use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::core::model::{EntryKind, HashStatus};

/// Shared transfer constants used by both V1 and V2 protocols.
pub(crate) const TRANSFER_V1_CHUNK_SIZE: usize = 1024 * 1024;
pub(crate) const TRANSFER_V2_CHUNK_SIZE: usize = 4 * 1024 * 1024;
pub(crate) const TRANSFER_V1_ACK_INTERVAL_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const TRANSFER_V2_ACK_INTERVAL_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const TRANSFER_PROGRESS_INTERVAL_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const NEGOTIATION_TIMEOUT_SECS: u64 = 1;

fn default_remote_entry_kind() -> EntryKind {
    EntryKind::File
}

fn default_remote_hash_status() -> HashStatus {
    HashStatus::Unavailable
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteFileState {
    pub relative_path: String,
    #[serde(default = "default_remote_entry_kind")]
    pub kind: EntryKind,
    pub blake3_hash: Option<String>,
    #[serde(default = "default_remote_hash_status")]
    pub hash_status: HashStatus,
    pub size: i64,
    pub modified_unix_ms: i64,
}

/// Messages exchanged between paired devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMessage {
    /// Request the peer's public identity for manual IP pairing.
    IdentityRequest,

    /// Public identity response. This data is public and must still be pinned
    /// by pairing approval before authenticated operations are allowed.
    IdentityResponse {
        device_id: String,
        public_key: Vec<u8>,
    },

    /// Request pairing with verification code.
    PairRequest {
        device_id: String,
        public_key: Vec<u8>,
        nonce: Vec<u8>,
    },

    /// Confirm pairing (both sides show the same code).
    PairConfirm { device_id: String, code: String },

    /// Reject pairing.
    PairReject { device_id: String, reason: String },

    /// Start an authenticated connection as a paired device.
    AuthHello { device_id: String },

    /// Server nonce challenge for the claimed device.
    AuthChallenge { nonce: Vec<u8> },

    /// Signed challenge proof.
    AuthProof {
        device_id: String,
        signature: Vec<u8>,
    },

    /// Authentication accepted.
    AuthOk { device_id: String },

    /// Authentication rejected.
    AuthReject { reason: String },

    /// Register the local root for a task on the receiving peer.
    TaskRegister { task_id: String, root_path: String },

    /// Acknowledge task registration.
    TaskAck {
        task_id: String,
        success: bool,
        error: Option<String>,
    },

    /// Ask the peer to allocate a receiving folder for a new task.
    TaskInvite {
        invite_id: String,
        task_id: String,
        task_name: String,
        requester_port: u16,
        requester_path: Option<String>,
        proposed_role: String,
    },

    /// First-contact task invitation. This only records a pending invite; the
    /// requester is trusted only after the local user accepts it.
    TaskInviteProposal {
        invite_id: String,
        task_id: String,
        task_name: String,
        requester_device_id: String,
        requester_public_key: Vec<u8>,
        requester_port: u16,
        requester_path: Option<String>,
        proposed_role: String,
    },

    /// The peer recorded the invitation and is waiting for local approval.
    TaskInvitePending { invite_id: String, task_id: String },

    /// Ask for the current status of a previously sent task invitation.
    TaskInviteStatusRequest { invite_id: String },

    /// Return the current status of a task invitation.
    TaskInviteStatus {
        invite_id: String,
        task_id: String,
        status: String,
        remote_path: Option<String>,
        error: Option<String>,
    },

    /// Acknowledge task invitation and return the peer-selected root.
    TaskInviteAck {
        task_id: String,
        success: bool,
        remote_path: Option<String>,
        error: Option<String>,
    },

    /// Transfer a file.
    FileTransfer {
        task_id: String,
        relative_path: String,
        file_hash: String,
        total_bytes: u64,
        data: Vec<u8>,
    },

    /// Start a chunked file transfer.
    FileChunkStart {
        task_id: String,
        relative_path: String,
        file_hash: String,
        total_bytes: u64,
    },

    /// One chunk in a chunked file transfer.
    FileChunk {
        task_id: String,
        relative_path: String,
        offset: u64,
        data: Vec<u8>,
    },

    /// Finish a chunked file transfer and verify hash.
    FileChunkEnd {
        task_id: String,
        relative_path: String,
        #[serde(default)]
        file_hash: Option<String>,
    },

    /// Request a file from a peer task root.
    FileDownloadRequest {
        task_id: String,
        relative_path: String,
    },

    /// Create an empty directory on the receiving peer.
    DirectoryCreate {
        task_id: String,
        relative_path: String,
    },

    /// Request to delete a file (primary delete → secondary history).
    FileDelete {
        task_id: String,
        relative_path: String,
        #[serde(default)]
        expected_kind: Option<EntryKind>,
        #[serde(default)]
        expected_hash: Option<String>,
        #[serde(default)]
        expected_hash_status: Option<HashStatus>,
        #[serde(default)]
        expected_size: Option<i64>,
        #[serde(default)]
        expected_modified_unix_ms: Option<i64>,
        #[serde(default)]
        delete_batch_id: Option<String>,
    },

    /// Acknowledge successful file write.
    FileAck {
        task_id: String,
        relative_path: String,
        success: bool,
        error: Option<String>,
    },

    /// Checkpoint ACK during chunked transfer. Sent every N bytes instead of every chunk.
    FileChunkAck {
        task_id: String,
        relative_path: String,
        received_bytes: u64,
        success: bool,
        error: Option<String>,
    },

    /// Scan request from watcher.
    ScanRequest { task_id: String },

    /// Remote scan response.
    ScanResponse {
        task_id: String,
        files: Vec<RemoteFileState>,
        error: Option<String>,
    },

    /// Heartbeat / keepalive.
    Ping,

    /// Heartbeat response.
    Pong,

    /// Cancel an in-flight transfer and remove any receiver-side partial file.
    TransferCancel {
        task_id: String,
        relative_path: String,
        #[serde(default)]
        direction: Option<String>,
    },

    // ── V2 protocol negotiation ──
    /// Announce supported transfer protocol versions.
    TransferHello {
        supported_versions: Vec<u16>,
        preferred_version: u16,
    },

    /// Accept a transfer protocol version.
    TransferReady {
        selected_version: u16,
        max_chunk_size: u32,
        ack_interval_bytes: u64,
    },

    // ── V2 binary transfer messages (control frames, JSON only) ──
    /// Start a V2 file stream. Payload follows as raw binary.
    FileStreamStartV2 {
        task_id: String,
        relative_path: String,
        total_bytes: u64,
    },

    /// V2 binary chunk header. Raw payload of `bytes` length follows immediately after.
    FileChunkBinaryV2 {
        task_id: String,
        relative_path: String,
        offset: u64,
        bytes: u32,
        ack: bool,
    },

    /// Finish a V2 file stream with final hash.
    FileStreamEndV2 {
        task_id: String,
        relative_path: String,
        file_hash: String,
    },

    /// V2 checkpoint ACK.
    FileStreamAckV2 {
        task_id: String,
        relative_path: String,
        received_bytes: u64,
        success: bool,
        error: Option<String>,
    },

    /// Request a file download via V2 protocol.
    FileDownloadRequestV2 {
        task_id: String,
        relative_path: String,
    },
}

/// Encode a V2 binary chunk frame: 4-byte JSON length + JSON header + raw payload.
#[cfg(test)]
pub fn encode_v2_chunk(header: &SyncMessage, payload: &[u8]) -> anyhow::Result<Vec<u8>> {
    let json = serde_json::to_vec(header)?;
    let len = json.len() as u32;
    let mut buf = Vec::with_capacity(4 + json.len() + payload.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&json);
    buf.extend_from_slice(payload);
    Ok(buf)
}

/// Write a V2 binary chunk without copying the payload into an intermediate frame buffer.
pub(crate) async fn write_v2_chunk(
    writer: &mut (impl AsyncWrite + Unpin),
    header: &SyncMessage,
    payload: &[u8],
) -> anyhow::Result<()> {
    let json = serde_json::to_vec(header)?;
    let len = json.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&json).await?;
    writer.write_all(payload).await?;
    Ok(())
}

/// Read exactly `n` raw bytes after a V2 binary chunk header has been decoded.
pub(crate) async fn read_v2_payload(
    reader: &mut (impl tokio::io::AsyncReadExt + Unpin),
    n: usize,
) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Wire format: length-prefixed JSON.
pub fn encode_message(msg: &SyncMessage) -> anyhow::Result<Vec<u8>> {
    let json = serde_json::to_vec(msg)?;
    let len = json.len() as u32;
    let mut buf = Vec::with_capacity(4 + json.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&json);
    Ok(buf)
}

pub fn decode_message(data: &[u8]) -> anyhow::Result<SyncMessage> {
    let msg: SyncMessage = serde_json::from_slice(data)?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(msg: &SyncMessage) -> SyncMessage {
        let encoded = encode_message(msg).unwrap();
        let len = u32::from_be_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]) as usize;
        assert_eq!(
            encoded.len(),
            4 + len,
            "length prefix should match payload size"
        );
        decode_message(&encoded[4..]).unwrap()
    }

    #[test]
    fn round_trip_ping_pong() {
        assert!(matches!(round_trip(&SyncMessage::Ping), SyncMessage::Ping));
        assert!(matches!(round_trip(&SyncMessage::Pong), SyncMessage::Pong));
    }

    #[test]
    fn round_trip_identity_response() {
        let msg = SyncMessage::IdentityResponse {
            device_id: "abc123".to_string(),
            public_key: vec![1, 2, 3, 4],
        };
        let decoded = round_trip(&msg);
        match decoded {
            SyncMessage::IdentityResponse {
                device_id,
                public_key,
            } => {
                assert_eq!(device_id, "abc123");
                assert_eq!(public_key, vec![1, 2, 3, 4]);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn round_trip_scan_request_and_response() {
        let req = SyncMessage::ScanRequest {
            task_id: "task-1".to_string(),
        };
        match round_trip(&req) {
            SyncMessage::ScanRequest { task_id } => assert_eq!(task_id, "task-1"),
            other => panic!("unexpected: {:?}", other),
        }

        let resp = SyncMessage::ScanResponse {
            task_id: "task-1".to_string(),
            files: vec![RemoteFileState {
                relative_path: "a.txt".to_string(),
                kind: crate::core::model::EntryKind::File,
                blake3_hash: Some("deadbeef".to_string()),
                hash_status: HashStatus::Verified,
                size: 42,
                modified_unix_ms: 1000,
            }],
            error: None,
        };
        match round_trip(&resp) {
            SyncMessage::ScanResponse {
                task_id,
                files,
                error,
            } => {
                assert_eq!(task_id, "task-1");
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].relative_path, "a.txt");
                assert_eq!(files[0].blake3_hash, Some("deadbeef".to_string()));
                assert!(error.is_none());
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn round_trip_file_chunk_start() {
        let msg = SyncMessage::FileChunkStart {
            task_id: "t1".to_string(),
            relative_path: "big.bin".to_string(),
            file_hash: "abc".to_string(),
            total_bytes: 1_048_576,
        };
        match round_trip(&msg) {
            SyncMessage::FileChunkStart {
                task_id,
                relative_path,
                file_hash,
                total_bytes,
            } => {
                assert_eq!(task_id, "t1");
                assert_eq!(relative_path, "big.bin");
                assert_eq!(file_hash, "abc");
                assert_eq!(total_bytes, 1_048_576);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn round_trip_file_chunk_end_optional_hash() {
        let with_hash = SyncMessage::FileChunkEnd {
            task_id: "t".to_string(),
            relative_path: "f".to_string(),
            file_hash: Some("hash".to_string()),
        };
        match round_trip(&with_hash) {
            SyncMessage::FileChunkEnd { file_hash, .. } => {
                assert_eq!(file_hash, Some("hash".to_string()));
            }
            other => panic!("unexpected: {:?}", other),
        }

        let without_hash = SyncMessage::FileChunkEnd {
            task_id: "t".to_string(),
            relative_path: "f".to_string(),
            file_hash: None,
        };
        match round_trip(&without_hash) {
            SyncMessage::FileChunkEnd { file_hash, .. } => {
                assert_eq!(file_hash, None);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn round_trip_task_invite_with_optional_fields() {
        let msg = SyncMessage::TaskInvite {
            invite_id: "inv-1".to_string(),
            task_id: "task-1".to_string(),
            task_name: "docs".to_string(),
            requester_port: 9527,
            requester_path: Some("/home/docs".to_string()),
            proposed_role: "Secondary".to_string(),
        };
        match round_trip(&msg) {
            SyncMessage::TaskInvite {
                invite_id,
                task_id,
                proposed_role,
                ..
            } => {
                assert_eq!(invite_id, "inv-1");
                assert_eq!(task_id, "task-1");
                assert_eq!(proposed_role, "Secondary");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn round_trip_file_ack_success_and_failure() {
        let ok = SyncMessage::FileAck {
            task_id: "t".to_string(),
            relative_path: "p".to_string(),
            success: true,
            error: None,
        };
        match round_trip(&ok) {
            SyncMessage::FileAck { success, error, .. } => {
                assert!(success);
                assert!(error.is_none());
            }
            other => panic!("unexpected: {:?}", other),
        }

        let fail = SyncMessage::FileAck {
            task_id: "t".to_string(),
            relative_path: "p".to_string(),
            success: false,
            error: Some("disk full".to_string()),
        };
        match round_trip(&fail) {
            SyncMessage::FileAck { success, error, .. } => {
                assert!(!success);
                assert_eq!(error, Some("disk full".to_string()));
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn encode_v2_chunk_produces_correct_format() {
        let header = SyncMessage::FileChunkBinaryV2 {
            task_id: "t1".to_string(),
            relative_path: "a.bin".to_string(),
            offset: 1024,
            bytes: 8,
            ack: true,
        };
        let payload = b"DEADBEEF";
        let encoded = encode_v2_chunk(&header, payload).unwrap();

        // First 4 bytes: JSON header length (big-endian)
        let json_len =
            u32::from_be_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]) as usize;
        assert!(json_len > 0);
        assert_eq!(encoded.len(), 4 + json_len + payload.len());

        // Verify payload follows the JSON header
        assert_eq!(&encoded[4 + json_len..], payload);
    }

    #[test]
    fn round_trip_v2_control_frames() {
        let messages = [
            SyncMessage::TransferHello {
                supported_versions: vec![2, 1],
                preferred_version: 2,
            },
            SyncMessage::TransferReady {
                selected_version: 2,
                max_chunk_size: 4 * 1024 * 1024,
                ack_interval_bytes: TRANSFER_V2_ACK_INTERVAL_BYTES,
            },
            SyncMessage::FileStreamStartV2 {
                task_id: "task".to_string(),
                relative_path: "file.bin".to_string(),
                total_bytes: 123,
            },
            SyncMessage::FileStreamEndV2 {
                task_id: "task".to_string(),
                relative_path: "file.bin".to_string(),
                file_hash: "abc".to_string(),
            },
            SyncMessage::FileStreamAckV2 {
                task_id: "task".to_string(),
                relative_path: "file.bin".to_string(),
                received_bytes: 123,
                success: true,
                error: None,
            },
            SyncMessage::FileDownloadRequestV2 {
                task_id: "task".to_string(),
                relative_path: "file.bin".to_string(),
            },
        ];

        for message in messages {
            match round_trip(&message) {
                round_tripped if format!("{round_tripped:?}") == format!("{message:?}") => {}
                other => panic!("unexpected V2 control frame round trip: {:?}", other),
            }
        }
    }

    #[test]
    fn remote_file_state_default_kind_is_file() {
        let json = r#"{"relative_path":"x","size":0,"modified_unix_ms":0}"#;
        let state: RemoteFileState = serde_json::from_str(json).unwrap();
        assert_eq!(state.relative_path, "x");
        assert!(matches!(state.kind, crate::core::model::EntryKind::File));
        assert!(state.blake3_hash.is_none());
        assert_eq!(state.hash_status, HashStatus::Unavailable);
    }

    #[test]
    fn round_trip_auth_messages() {
        let hello = SyncMessage::AuthHello {
            device_id: "dev-1".to_string(),
        };
        match round_trip(&hello) {
            SyncMessage::AuthHello { device_id } => assert_eq!(device_id, "dev-1"),
            other => panic!("unexpected: {:?}", other),
        }

        let challenge = SyncMessage::AuthChallenge {
            nonce: vec![0u8; 32],
        };
        match round_trip(&challenge) {
            SyncMessage::AuthChallenge { nonce } => assert_eq!(nonce.len(), 32),
            other => panic!("unexpected: {:?}", other),
        }

        let reject = SyncMessage::AuthReject {
            reason: "not trusted".to_string(),
        };
        match round_trip(&reject) {
            SyncMessage::AuthReject { reason } => assert_eq!(reason, "not trusted"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn round_trip_directory_create_and_file_delete() {
        let dir = SyncMessage::DirectoryCreate {
            task_id: "t".to_string(),
            relative_path: "docs/".to_string(),
        };
        match round_trip(&dir) {
            SyncMessage::DirectoryCreate {
                task_id,
                relative_path,
            } => {
                assert_eq!(task_id, "t");
                assert_eq!(relative_path, "docs/");
            }
            other => panic!("unexpected: {:?}", other),
        }

        let del = SyncMessage::FileDelete {
            task_id: "t".to_string(),
            relative_path: "old.txt".to_string(),
            expected_kind: None,
            expected_hash: None,
            expected_hash_status: None,
            expected_size: None,
            expected_modified_unix_ms: None,
            delete_batch_id: None,
        };
        match round_trip(&del) {
            SyncMessage::FileDelete {
                task_id,
                relative_path,
                expected_kind,
                expected_hash,
                expected_hash_status,
                expected_size,
                expected_modified_unix_ms,
                delete_batch_id,
            } => {
                assert_eq!(task_id, "t");
                assert_eq!(relative_path, "old.txt");
                assert_eq!(expected_kind, None);
                assert_eq!(expected_hash, None);
                assert_eq!(expected_hash_status, None);
                assert_eq!(expected_size, None);
                assert_eq!(expected_modified_unix_ms, None);
                assert_eq!(delete_batch_id, None);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }
}
