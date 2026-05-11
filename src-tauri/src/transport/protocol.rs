use serde::{Deserialize, Serialize};

/// Messages exchanged between paired devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMessage {
    /// Request pairing with verification code.
    PairRequest {
        device_id: String,
        public_key: Vec<u8>,
        nonce: Vec<u8>,
    },

    /// Confirm pairing (both sides show the same code).
    PairConfirm {
        device_id: String,
        code: String,
    },

    /// Reject pairing.
    PairReject {
        device_id: String,
        reason: String,
    },

    /// Transfer a file.
    FileTransfer {
        task_id: String,
        relative_path: String,
        file_hash: String,
        total_bytes: u64,
        data: Vec<u8>,
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
    ScanRequest {
        task_id: String,
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
