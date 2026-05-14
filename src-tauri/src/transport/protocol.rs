use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteFileState {
    pub relative_path: String,
    pub blake3_hash: Option<String>,
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
    },

    /// Request a file from a peer task root.
    FileDownloadRequest {
        task_id: String,
        relative_path: String,
    },

    /// Request to delete a file (primary delete → secondary history).
    FileDelete {
        task_id: String,
        relative_path: String,
    },

    /// Acknowledge successful file write.
    FileAck {
        task_id: String,
        relative_path: String,
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
