use anyhow::Result;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::pairing::{DeviceIdentity, PublicIdentity};
use crate::transport::protocol::{decode_message, encode_message, RemoteFileState, SyncMessage};

/// Connection state for a paired peer.
#[derive(Debug, Clone)]
pub struct PeerConnection {
    pub device_id: String,
    pub address: String,
    pub connected: bool,
    pub last_seen_unix_ms: i64,
}

/// Connection manager for paired peers.
///
/// P0: Manual IP connection only.
/// P1: UDP discovery with manual IP fallback.
#[derive(Clone)]
pub struct ConnectionManager {
    peers: Arc<Mutex<HashMap<String, PeerConnection>>>,
    pinned_identities: Arc<Mutex<HashMap<String, PublicIdentity>>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            peers: Arc::new(Mutex::new(HashMap::new())),
            pinned_identities: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Pin a peer identity after successful pairing.
    pub fn pin_peer(&self, identity: PublicIdentity) {
        let mut pins = self.pinned_identities.lock().unwrap();
        pins.insert(identity.device_id.clone(), identity);
    }

    /// Check if a device ID is pinned.
    pub fn is_pinned(&self, device_id: &str) -> bool {
        let pins = self.pinned_identities.lock().unwrap();
        pins.contains_key(device_id)
    }

    /// Get a pinned peer identity.
    pub fn get_pinned(&self, device_id: &str) -> Option<PublicIdentity> {
        let pins = self.pinned_identities.lock().unwrap();
        pins.get(device_id).cloned()
    }

    /// Register a connected peer.
    pub fn register_connection(&self, conn: PeerConnection) {
        let mut peers = self.peers.lock().unwrap();
        peers.insert(conn.device_id.clone(), conn);
    }

    /// Mark a peer as disconnected.
    pub fn disconnect(&self, device_id: &str) {
        let mut peers = self.peers.lock().unwrap();
        if let Some(peer) = peers.get_mut(device_id) {
            peer.connected = false;
        }
    }

    /// Check if a peer is currently connected.
    pub fn is_connected(&self, device_id: &str) -> bool {
        let peers = self.peers.lock().unwrap();
        peers.get(device_id).map_or(false, |p| p.connected)
    }

    /// List all known peer connections.
    pub fn list_peers(&self) -> Vec<PeerConnection> {
        let peers = self.peers.lock().unwrap();
        peers.values().cloned().collect()
    }

    /// Get a known peer connection by device ID.
    pub fn get_peer(&self, device_id: &str) -> Option<PeerConnection> {
        let peers = self.peers.lock().unwrap();
        peers.get(device_id).cloned()
    }
}

/// Attempt to connect to a peer at the given address.
///
/// P0: Manual TCP connection. Returns the stream on success.
pub async fn connect_to_peer(address: &str, port: u16) -> Result<TcpStream> {
    let addr = format!("{}:{}", address, port);
    let stream = TcpStream::connect(&addr).await?;
    Ok(stream)
}

pub async fn ping_peer_address(address: &str, port: u16) -> Result<()> {
    let mut stream = connect_to_peer(address, port).await?;
    stream
        .write_all(&encode_message(&SyncMessage::Ping)?)
        .await?;
    match read_message(&mut stream).await? {
        SyncMessage::Pong => Ok(()),
        other => anyhow::bail!("unexpected ping response: {:?}", other),
    }
}

pub async fn request_peer_identity(address: &str, port: u16) -> Result<PublicIdentity> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut stream = connect_to_peer(address, port).await?;
        stream
            .write_all(&encode_message(&SyncMessage::IdentityRequest)?)
            .await?;

        match read_message(&mut stream).await? {
            SyncMessage::IdentityResponse {
                device_id,
                public_key,
            } if !device_id.is_empty() && !public_key.is_empty() => Ok(PublicIdentity {
                device_id,
                public_key,
            }),
            SyncMessage::AuthReject { reason } => anyhow::bail!(reason),
            other => anyhow::bail!("unexpected identity response: {:?}", other),
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("peer identity request timed out"))?
}

pub async fn send_message_to_peer(
    manager: &ConnectionManager,
    device_id: &str,
    message: SyncMessage,
) -> Result<SyncMessage> {
    let peer = manager
        .get_peer(device_id)
        .ok_or_else(|| anyhow::anyhow!("peer is not connected"))?;
    if !peer.connected {
        anyhow::bail!("peer is disconnected");
    }

    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect(&peer.address),
    )
    .await??;
    stream.write_all(&encode_message(&message)?).await?;
    read_message(&mut stream).await
}

pub async fn send_authenticated_message_to_peer(
    manager: &ConnectionManager,
    local_identity: &DeviceIdentity,
    device_id: &str,
    message: SyncMessage,
) -> Result<SyncMessage> {
    let peer = manager
        .get_peer(device_id)
        .ok_or_else(|| anyhow::anyhow!("peer is not connected"))?;
    if !peer.connected {
        anyhow::bail!("peer is disconnected");
    }

    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect(&peer.address),
    )
    .await??;

    let local_device_id = local_identity.public().device_id;
    stream
        .write_all(&encode_message(&SyncMessage::AuthHello {
            device_id: local_device_id.clone(),
        })?)
        .await?;

    let challenge = match read_message(&mut stream).await? {
        SyncMessage::AuthChallenge { nonce } => nonce,
        SyncMessage::AuthReject { reason } => anyhow::bail!("authentication rejected: {}", reason),
        other => anyhow::bail!("unexpected auth response: {:?}", other),
    };

    let signature = local_identity
        .sign(&auth_payload(&local_device_id, &challenge))
        .to_bytes()
        .to_vec();
    stream
        .write_all(&encode_message(&SyncMessage::AuthProof {
            device_id: local_device_id,
            signature,
        })?)
        .await?;

    match read_message(&mut stream).await? {
        SyncMessage::AuthOk { .. } => {}
        SyncMessage::AuthReject { reason } => anyhow::bail!("authentication rejected: {}", reason),
        other => anyhow::bail!("unexpected auth proof response: {:?}", other),
    }

    stream.write_all(&encode_message(&message)?).await?;
    read_message(&mut stream).await
}

pub async fn send_authenticated_file_to_peer(
    manager: &ConnectionManager,
    local_identity: &DeviceIdentity,
    device_id: &str,
    task_id: impl Into<String>,
    relative_path: impl Into<String>,
    file_path: &Path,
) -> Result<()> {
    const CHUNK_SIZE: usize = 64 * 1024;

    let task_id = task_id.into();
    let relative_path = relative_path.into();
    let metadata = std::fs::metadata(file_path)?;
    let total_bytes = metadata.len();
    let file_hash = crate::core::scanner::hash_file(file_path)?;
    let mut stream = open_authenticated_stream(manager, local_identity, device_id).await?;

    send_and_expect_file_ack(
        &mut stream,
        SyncMessage::FileChunkStart {
            task_id: task_id.clone(),
            relative_path: relative_path.clone(),
            file_hash,
            total_bytes,
        },
    )
    .await?;

    let mut file = std::fs::File::open(file_path)?;
    let mut offset = 0u64;
    loop {
        let mut buf = vec![0u8; CHUNK_SIZE];
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        buf.truncate(read);
        send_and_expect_file_ack(
            &mut stream,
            SyncMessage::FileChunk {
                task_id: task_id.clone(),
                relative_path: relative_path.clone(),
                offset,
                data: buf,
            },
        )
        .await?;
        offset += read as u64;
    }

    send_and_expect_file_ack(
        &mut stream,
        SyncMessage::FileChunkEnd {
            task_id,
            relative_path,
        },
    )
    .await
}

pub async fn request_authenticated_scan(
    manager: &ConnectionManager,
    local_identity: &DeviceIdentity,
    device_id: &str,
    task_id: impl Into<String>,
) -> Result<Vec<RemoteFileState>> {
    let task_id = task_id.into();
    match send_authenticated_message_to_peer(
        manager,
        local_identity,
        device_id,
        SyncMessage::ScanRequest {
            task_id: task_id.clone(),
        },
    )
    .await?
    {
        SyncMessage::ScanResponse {
            error: None, files, ..
        } => Ok(files),
        SyncMessage::ScanResponse {
            error: Some(error), ..
        } => anyhow::bail!(error),
        other => anyhow::bail!("unexpected scan response: {:?}", other),
    }
}

pub async fn request_authenticated_file_from_peer(
    manager: &ConnectionManager,
    local_identity: &DeviceIdentity,
    device_id: &str,
    task_id: impl Into<String>,
    relative_path: impl Into<String>,
    target_path: &Path,
) -> Result<()> {
    let task_id = task_id.into();
    let relative_path = relative_path.into();
    let mut stream = open_authenticated_stream(manager, local_identity, device_id).await?;
    stream
        .write_all(&encode_message(&SyncMessage::FileDownloadRequest {
            task_id: task_id.clone(),
            relative_path: relative_path.clone(),
        })?)
        .await?;

    let (expected_hash, total_bytes) = match read_message(&mut stream).await? {
        SyncMessage::FileChunkStart {
            task_id: ack_task,
            relative_path: ack_path,
            file_hash,
            total_bytes,
        } if ack_task == task_id && ack_path == relative_path => (file_hash, total_bytes),
        SyncMessage::FileAck { error, .. } => {
            anyhow::bail!(error.unwrap_or_else(|| "peer rejected file download".to_string()))
        }
        other => anyhow::bail!("unexpected download response: {:?}", other),
    };

    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial_path = partial_path(target_path);
    let mut file = std::fs::File::create(&partial_path)?;
    let mut hasher = blake3::Hasher::new();
    let mut written = 0u64;

    loop {
        match read_message(&mut stream).await? {
            SyncMessage::FileChunk {
                task_id: chunk_task,
                relative_path: chunk_path,
                offset,
                data,
            } if chunk_task == task_id && chunk_path == relative_path => {
                if offset != written {
                    let _ = std::fs::remove_file(&partial_path);
                    anyhow::bail!("unexpected download chunk offset");
                }
                hasher.update(&data);
                file.write_all(&data)?;
                written += data.len() as u64;
                if written > total_bytes {
                    let _ = std::fs::remove_file(&partial_path);
                    anyhow::bail!("download exceeded expected size");
                }
            }
            SyncMessage::FileChunkEnd {
                task_id: end_task,
                relative_path: end_path,
            } if end_task == task_id && end_path == relative_path => break,
            SyncMessage::FileAck { error, .. } => {
                let _ = std::fs::remove_file(&partial_path);
                anyhow::bail!(error.unwrap_or_else(|| "peer rejected file download".to_string()));
            }
            other => {
                let _ = std::fs::remove_file(&partial_path);
                anyhow::bail!("unexpected download message: {:?}", other);
            }
        }
    }

    file.flush()?;
    drop(file);
    if written != total_bytes {
        let _ = std::fs::remove_file(&partial_path);
        anyhow::bail!("download size mismatch");
    }
    let actual_hash = hasher.finalize().to_hex().to_string();
    if actual_hash != expected_hash {
        let _ = std::fs::remove_file(&partial_path);
        anyhow::bail!("download hash mismatch");
    }
    std::fs::rename(partial_path, target_path)?;
    Ok(())
}

pub fn auth_payload(device_id: &str, nonce: &[u8]) -> Vec<u8> {
    let mut payload = b"lanbridge-auth-v1:".to_vec();
    payload.extend_from_slice(device_id.as_bytes());
    payload.push(b':');
    payload.extend_from_slice(nonce);
    payload
}

async fn open_authenticated_stream(
    manager: &ConnectionManager,
    local_identity: &DeviceIdentity,
    device_id: &str,
) -> Result<TcpStream> {
    let peer = manager
        .get_peer(device_id)
        .ok_or_else(|| anyhow::anyhow!("peer is not connected"))?;
    if !peer.connected {
        anyhow::bail!("peer is disconnected");
    }

    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect(&peer.address),
    )
    .await??;

    let local_device_id = local_identity.public().device_id;
    stream
        .write_all(&encode_message(&SyncMessage::AuthHello {
            device_id: local_device_id.clone(),
        })?)
        .await?;

    let challenge = match read_message(&mut stream).await? {
        SyncMessage::AuthChallenge { nonce } => nonce,
        SyncMessage::AuthReject { reason } => anyhow::bail!("authentication rejected: {}", reason),
        other => anyhow::bail!("unexpected auth response: {:?}", other),
    };

    let signature = local_identity
        .sign(&auth_payload(&local_device_id, &challenge))
        .to_bytes()
        .to_vec();
    stream
        .write_all(&encode_message(&SyncMessage::AuthProof {
            device_id: local_device_id,
            signature,
        })?)
        .await?;

    match read_message(&mut stream).await? {
        SyncMessage::AuthOk { .. } => Ok(stream),
        SyncMessage::AuthReject { reason } => anyhow::bail!("authentication rejected: {}", reason),
        other => anyhow::bail!("unexpected auth proof response: {:?}", other),
    }
}

async fn send_and_expect_file_ack(stream: &mut TcpStream, msg: SyncMessage) -> Result<()> {
    stream.write_all(&encode_message(&msg)?).await?;
    match read_message(stream).await? {
        SyncMessage::FileAck { success: true, .. } => Ok(()),
        SyncMessage::FileAck { error, .. } => {
            anyhow::bail!(error.unwrap_or_else(|| "peer rejected file operation".to_string()))
        }
        other => anyhow::bail!("unexpected peer response: {:?}", other),
    }
}

async fn read_message(stream: &mut TcpStream) -> Result<SyncMessage> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let msg_len = u32::from_be_bytes(len_buf) as usize;
    if msg_len > 10 * 1024 * 1024 {
        anyhow::bail!("message too large: {} bytes", msg_len);
    }

    let mut msg_buf = vec![0u8; msg_len];
    stream.read_exact(&mut msg_buf).await?;
    decode_message(&msg_buf)
}

fn partial_path(target_path: &Path) -> std::path::PathBuf {
    let file_name = target_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download");
    target_path.with_file_name(format!("{}.lanbridge-partial", file_name))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn temporary_device_id(address: &str, port: u16) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(address.as_bytes());
    hasher.update(port.to_be_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

pub fn pin_connected_peer(
    manager: &ConnectionManager,
    address: &str,
    port: u16,
    peer: Option<PublicIdentity>,
) -> String {
    let identity = peer.unwrap_or_else(|| PublicIdentity {
        device_id: temporary_device_id(address, port),
        public_key: vec![0u8; 32],
    });
    let device_id = identity.device_id.clone();

    if !manager.is_pinned(&device_id) {
        manager.pin_peer(identity);
    }

    manager.register_connection(PeerConnection {
        device_id: device_id.clone(),
        address: format!("{}:{}", address, port),
        connected: true,
        last_seen_unix_ms: now_ms(),
    });

    device_id
}
