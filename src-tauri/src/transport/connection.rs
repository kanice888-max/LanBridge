use crate::pairing::{DeviceIdentity, PublicIdentity};
use crate::transport::protocol::{
    decode_message, encode_message, RemoteFileState, SyncMessage, NEGOTIATION_TIMEOUT_SECS,
    TRANSFER_PROGRESS_INTERVAL_BYTES, TRANSFER_V1_ACK_INTERVAL_BYTES, TRANSFER_V1_CHUNK_SIZE,
    TRANSFER_V2_ACK_INTERVAL_BYTES, TRANSFER_V2_CHUNK_SIZE,
};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
/// Global speed limit in bytes/sec. 0 = unlimited.
static GLOBAL_RATE_LIMIT_BPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Set the global transfer speed limit. Pass 0 to disable.
pub fn set_transfer_speed_limit(bytes_per_sec: u64) {
    GLOBAL_RATE_LIMIT_BPS.store(bytes_per_sec, std::sync::atomic::Ordering::Relaxed);
}
/// Get the current global transfer speed limit. 0 = unlimited.
pub fn get_transfer_speed_limit() -> u64 {
    GLOBAL_RATE_LIMIT_BPS.load(std::sync::atomic::Ordering::Relaxed)
}
// ─── Transfer Progress Tracking ───
/// Progress of an active file transfer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TransferProgress {
    pub transfer_id: String,
    pub task_id: String,
    pub relative_path: String,
    pub direction: String, // "upload" or "download"
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Total bytes sent over the wire (includes protocol overhead).
    pub wire_bytes: u64,
    pub mbps: f64,
    pub finished: bool,
    /// Protocol version in use: "v2_binary" or "v1_json".
    pub protocol_version: String,
    #[serde(skip_serializing)]
    pub finished_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FileTransferOutcome {
    pub blake3_hash: String,
    pub protocol: &'static str,
    pub elapsed_ms: u64,
}
static GLOBAL_TRANSFER_PROGRESS: std::sync::LazyLock<Mutex<HashMap<String, TransferProgress>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static CANCELLED_TRANSFERS: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));
static DEFERRED_TRANSFERS: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

const TRANSFER_DIRECTIONS: [&str; 4] = ["upload", "download", "receive", "serve"];

fn recover_lock<'a, T>(mutex: &'a Mutex<T>, name: &str) -> MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        tracing::error!(lock = name, "recovering poisoned transfer lock");
        poisoned.into_inner()
    })
}

pub fn new_transfer_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn transfer_key(task_id: &str, relative_path: &str, direction: &str) -> String {
    format!("{}\n{}\n{}", task_id, relative_path, direction)
}

fn transfer_matches(
    progress: &TransferProgress,
    task_id: &str,
    relative_path: &str,
    direction: Option<&str>,
) -> bool {
    progress.task_id == task_id
        && progress.relative_path == relative_path
        && direction.map_or(true, |direction| progress.direction == direction)
}

struct TransferProgressGuard {
    transfer_id: String,
}

impl TransferProgressGuard {
    fn new(transfer_id: impl Into<String>) -> Self {
        Self {
            transfer_id: transfer_id.into(),
        }
    }
}

impl Drop for TransferProgressGuard {
    fn drop(&mut self) {
        finish_transfer_progress(&self.transfer_id);
    }
}

/// Record a transfer progress update.
pub fn record_transfer_progress(progress: TransferProgress) {
    if let Ok(mut map) = GLOBAL_TRANSFER_PROGRESS.lock() {
        map.insert(progress.transfer_id.clone(), progress);
    }
}
/// Mark a transfer as finished (removes it from active tracking).
pub fn finish_transfer_progress(transfer_id: &str) {
    if let Ok(mut map) = GLOBAL_TRANSFER_PROGRESS.lock() {
        if let Some(progress) = map.get_mut(transfer_id) {
            progress.finished = true;
            progress.bytes_done = progress.bytes_total;
            progress.finished_at_unix_ms = Some(now_ms_i64());
        }
    }
}

pub fn finish_transfer_progress_for_path(
    task_id: &str,
    relative_path: &str,
    direction: Option<&str>,
) {
    if let Ok(mut map) = GLOBAL_TRANSFER_PROGRESS.lock() {
        map.retain(|_, progress| !transfer_matches(progress, task_id, relative_path, direction));
    }
}
/// Get all active transfer progress entries.
pub fn get_transfer_progress() -> Vec<TransferProgress> {
    let now = now_ms_i64();
    GLOBAL_TRANSFER_PROGRESS
        .lock()
        .map(|mut map| {
            map.retain(|_, progress| {
                progress
                    .finished_at_unix_ms
                    .map_or(true, |finished_at| now - finished_at <= 2_000)
            });
            map.values().cloned().collect()
        })
        .unwrap_or_default()
}

pub fn has_active_transfers() -> bool {
    GLOBAL_TRANSFER_PROGRESS
        .lock()
        .map(|map| map.values().any(|progress| !progress.finished))
        .unwrap_or(false)
}

pub fn cancel_transfer(task_id: &str, relative_path: &str, direction: &str) {
    cancel_active_transfer(task_id, relative_path, Some(direction));
    defer_transfer(task_id, relative_path, direction);
}

pub fn cancel_active_transfer(task_id: &str, relative_path: &str, direction: Option<&str>) {
    if let Ok(mut cancelled) = CANCELLED_TRANSFERS.lock() {
        if let Some(direction) = direction {
            cancelled.insert(transfer_key(task_id, relative_path, direction));
        } else {
            for direction in TRANSFER_DIRECTIONS {
                cancelled.insert(transfer_key(task_id, relative_path, direction));
            }
        }
    }
    finish_transfer_progress_for_path(task_id, relative_path, direction);
}

pub fn defer_transfer(task_id: &str, relative_path: &str, direction: &str) {
    if let Ok(mut deferred) = DEFERRED_TRANSFERS.lock() {
        deferred.insert(transfer_key(task_id, relative_path, direction));
    }
}

pub fn resume_deferred_transfer(task_id: &str, relative_path: &str, direction: Option<&str>) {
    if let Ok(mut deferred) = DEFERRED_TRANSFERS.lock() {
        if let Some(direction) = direction {
            deferred.remove(&transfer_key(task_id, relative_path, direction));
        } else {
            for direction in TRANSFER_DIRECTIONS {
                deferred.remove(&transfer_key(task_id, relative_path, direction));
            }
        }
    }
    clear_transfer_cancel(task_id, relative_path, direction);
}

pub fn list_deferred_transfers() -> Vec<(String, String, String)> {
    DEFERRED_TRANSFERS
        .lock()
        .map(|deferred| {
            deferred
                .iter()
                .filter_map(|key| {
                    let mut parts = key.splitn(3, '\n');
                    Some((
                        parts.next()?.to_string(),
                        parts.next()?.to_string(),
                        parts.next()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn clear_transfer_cancel(task_id: &str, relative_path: &str, direction: Option<&str>) {
    if let Ok(mut cancelled) = CANCELLED_TRANSFERS.lock() {
        if let Some(direction) = direction {
            cancelled.remove(&transfer_key(task_id, relative_path, direction));
        } else {
            for direction in TRANSFER_DIRECTIONS {
                cancelled.remove(&transfer_key(task_id, relative_path, direction));
            }
        }
    }
}

pub(crate) fn is_transfer_cancelled(task_id: &str, relative_path: &str, direction: &str) -> bool {
    CANCELLED_TRANSFERS
        .lock()
        .map(|cancelled| cancelled.contains(&transfer_key(task_id, relative_path, direction)))
        .unwrap_or(false)
}

pub(crate) fn ensure_transfer_not_cancelled(
    task_id: &str,
    relative_path: &str,
    direction: &str,
) -> Result<()> {
    if is_transfer_cancelled(task_id, relative_path, direction) {
        anyhow::bail!("transfer cancelled");
    }
    if is_transfer_deferred(task_id, relative_path, direction) {
        anyhow::bail!("transfer deferred by user");
    }
    Ok(())
}

pub(crate) fn is_transfer_deferred(task_id: &str, relative_path: &str, direction: &str) -> bool {
    DEFERRED_TRANSFERS
        .lock()
        .map(|deferred| deferred.contains(&transfer_key(task_id, relative_path, direction)))
        .unwrap_or(false)
}

fn now_ms_i64() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
/// Cache of negotiated protocol versions per peer device_id.
/// Maps device_id -> protocol_version (2 or 1). Cleared on errors.
static PEER_PROTOCOL: std::sync::LazyLock<Mutex<HashMap<String, u16>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn get_cached_protocol(device_id: &str) -> Option<u16> {
    PEER_PROTOCOL.lock().ok()?.get(device_id).copied()
}

pub fn set_cached_protocol(device_id: &str, version: u16) {
    if let Ok(mut map) = PEER_PROTOCOL.lock() {
        map.insert(device_id.to_string(), version);
    }
}

pub fn clear_cached_protocol(device_id: &str) {
    if let Ok(mut map) = PEER_PROTOCOL.lock() {
        map.remove(device_id);
    }
}

/// Check whether V1 fallback is allowed.
/// When `LANBRIDGE_FORCE_V2` is set to `1`, the app will refuse to fall back to V1
/// and return an error instead. This is useful for debugging to confirm whether V2
/// is actually being used.
pub fn force_v2_enabled() -> bool {
    std::env::var("LANBRIDGE_FORCE_V2").as_deref() == Ok("1")
}

/// Record a transfer progress update with 500ms throttle per file.
/// Avoids overwhelming the global state from high-frequency chunk loops.
pub(crate) fn record_throttled(
    transfer_id: &str,
    task_id: &str,
    relative_path: &str,
    direction: &str,
    bytes_done: u64,
    bytes_total: u64,
    wire_bytes: u64,
    mbps: f64,
    protocol_version: &str,
) {
    use std::sync::OnceLock;
    use std::time::Instant;
    static LAST_UI: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let last_ui = LAST_UI.get_or_init(|| Mutex::new(HashMap::new()));
    let key = transfer_id.to_string();
    if let Ok(mut last) = last_ui.lock() {
        if let Some(t) = last.get(&key) {
            if t.elapsed() < Duration::from_millis(500) {
                return;
            }
        }
        last.insert(key.clone(), Instant::now());
    }
    record_transfer_progress(TransferProgress {
        transfer_id: transfer_id.to_string(),
        task_id: task_id.to_string(),
        relative_path: relative_path.to_string(),
        direction: direction.to_string(),
        bytes_done,
        bytes_total,
        wire_bytes,
        mbps,
        finished: false,
        protocol_version: protocol_version.to_string(),
        finished_at_unix_ms: None,
    });
}
/// Sleep if needed to respect the global speed limit for `bytes` just sent.
async fn throttle(bytes: u64) {
    let limit = GLOBAL_RATE_LIMIT_BPS.load(std::sync::atomic::Ordering::Relaxed);
    if limit == 0 {
        return;
    }
    let delay = Duration::from_secs_f64(bytes as f64 / limit as f64);
    if delay > Duration::ZERO {
        tokio::time::sleep(delay).await;
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[derive(Debug, Default)]
struct V2TransferTiming {
    read_ms: u64,
    hash_ms: u64,
    socket_write_ms: u64,
    chunk_socket_write_ms: u64,
    ack_wait_ms: u64,
    throttle_ms: u64,
    chunk_count: u64,
}

impl V2TransferTiming {
    fn log_summary(
        &self,
        transfer_id: &str,
        task_id: &str,
        relative_path: &str,
        direction: &str,
        bytes_total: u64,
        elapsed_ms: u64,
        ack_interval_bytes: u64,
        success: bool,
        error: Option<&str>,
    ) {
        let avg_chunk_write_ms = if self.chunk_count > 0 {
            self.chunk_socket_write_ms as f64 / self.chunk_count as f64
        } else {
            0.0
        };
        tracing::info!(
            transfer_timing_summary = true,
            transfer_id = %transfer_id,
            task_id = %task_id,
            relative_path = %relative_path,
            direction = direction,
            protocol = "v2_binary",
            success = success,
            error = error.unwrap_or(""),
            bytes_total = bytes_total,
            elapsed_ms = elapsed_ms,
            ack_interval_bytes = ack_interval_bytes,
            read_ms = self.read_ms,
            hash_ms = self.hash_ms,
            socket_write_ms = self.socket_write_ms,
            ack_wait_ms = self.ack_wait_ms,
            throttle_ms = self.throttle_ms,
            chunk_count = self.chunk_count,
            avg_chunk_write_ms = format_args!("{:.2}", avg_chunk_write_ms),
        );
    }
}

struct V2ReadChunk {
    offset: u64,
    data: Vec<u8>,
    read_ms: u64,
}

type V2ReadChunkResult = std::result::Result<V2ReadChunk, String>;

fn spawn_v2_file_reader(
    task_id: String,
    relative_path: String,
    direction: String,
    file_path: PathBuf,
) -> mpsc::Receiver<V2ReadChunkResult> {
    let (tx, rx) = mpsc::channel(3);
    tokio::task::spawn_blocking(move || {
        let mut file = match std::fs::File::open(&file_path) {
            Ok(file) => file,
            Err(e) => {
                let _ = tx.blocking_send(Err(e.to_string()));
                return;
            }
        };
        let mut offset = 0u64;
        let mut buf = vec![0u8; TRANSFER_V2_CHUNK_SIZE];
        loop {
            if is_transfer_cancelled(&task_id, &relative_path, &direction) {
                let _ = tx.blocking_send(Err("transfer cancelled".to_string()));
                return;
            }
            let read_start = Instant::now();
            let read = match file.read(&mut buf) {
                Ok(read) => read,
                Err(e) => {
                    let _ = tx.blocking_send(Err(e.to_string()));
                    return;
                }
            };
            let read_ms = elapsed_ms(read_start);
            if read == 0 {
                return;
            }
            let chunk = V2ReadChunk {
                offset,
                data: buf[..read].to_vec(),
                read_ms,
            };
            offset += read as u64;
            if tx.blocking_send(Ok(chunk)).is_err() {
                return;
            }
        }
    });
    rx
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceFileState {
    pub(crate) len: u64,
    modified: Option<SystemTime>,
}
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
        let mut pins = recover_lock(&self.pinned_identities, "pinned_identities");
        pins.insert(identity.device_id.clone(), identity);
    }
    /// Check if a device ID is pinned.
    pub fn is_pinned(&self, device_id: &str) -> bool {
        let pins = recover_lock(&self.pinned_identities, "pinned_identities");
        pins.contains_key(device_id)
    }
    /// Get a pinned peer identity.
    pub fn get_pinned(&self, device_id: &str) -> Option<PublicIdentity> {
        let pins = recover_lock(&self.pinned_identities, "pinned_identities");
        pins.get(device_id).cloned()
    }
    /// Register a connected peer.
    pub fn register_connection(&self, conn: PeerConnection) {
        let mut peers = recover_lock(&self.peers, "peers");
        peers.insert(conn.device_id.clone(), conn);
    }
    /// Mark a peer as disconnected.
    pub fn disconnect(&self, device_id: &str) {
        let mut peers = recover_lock(&self.peers, "peers");
        if let Some(peer) = peers.get_mut(device_id) {
            peer.connected = false;
        }
    }
    /// Check if a peer is currently connected.
    pub fn is_connected(&self, device_id: &str) -> bool {
        let peers = recover_lock(&self.peers, "peers");
        peers.get(device_id).map_or(false, |p| p.connected)
    }
    /// List all known peer connections.
    pub fn list_peers(&self) -> Vec<PeerConnection> {
        let peers = recover_lock(&self.peers, "peers");
        peers.values().cloned().collect()
    }
    /// Get a known peer connection by device ID.
    pub fn get_peer(&self, device_id: &str) -> Option<PeerConnection> {
        let peers = recover_lock(&self.peers, "peers");
        peers.get(device_id).cloned()
    }

    pub fn mark_connected(&self, device_id: &str) {
        let mut peers = recover_lock(&self.peers, "peers");
        if let Some(peer) = peers.get_mut(device_id) {
            peer.connected = true;
            peer.last_seen_unix_ms = now_ms();
        }
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

pub async fn ping_known_peer(manager: &ConnectionManager, device_id: &str) -> Result<()> {
    let peer = manager
        .get_peer(device_id)
        .ok_or_else(|| anyhow::anyhow!("peer is not connected"))?;
    let (address, port) = peer
        .address
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("invalid peer address"))?;
    let port = port.parse::<u16>()?;
    match ping_peer_address(address, port).await {
        Ok(()) => {
            manager.mark_connected(device_id);
            Ok(())
        }
        Err(error) => {
            manager.disconnect(device_id);
            Err(error)
        }
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
    let mut stream = open_authenticated_stream(manager, local_identity, device_id).await?;
    stream.write_all(&encode_message(&message)?).await?;
    read_message(&mut stream).await
}
/// Try to negotiate V2 transfer protocol on an open authenticated stream.
/// Returns `true` if V2 was negotiated, `false` if the peer explicitly selected V1.
async fn try_negotiate_v2(stream: &mut TcpStream) -> Result<bool> {
    use std::time::Duration;
    use tokio::time::timeout;
    stream
        .write_all(&encode_message(&SyncMessage::TransferHello {
            supported_versions: vec![2, 1],
            preferred_version: 2,
        })?)
        .await?;
    match timeout(
        Duration::from_secs(NEGOTIATION_TIMEOUT_SECS),
        read_message(stream),
    )
    .await
    {
        Ok(Ok(SyncMessage::TransferReady {
            selected_version: 2,
            ..
        })) => Ok(true),
        Ok(Ok(SyncMessage::TransferReady {
            selected_version, ..
        })) if selected_version == 1 => Ok(false),
        Ok(Ok(other)) => anyhow::bail!("unexpected negotiation response: {:?}", other),
        Ok(Err(error)) => Err(error),
        Err(_) => anyhow::bail!(
            "V2 negotiation timed out after {}s",
            NEGOTIATION_TIMEOUT_SECS
        ),
    }
}
/// Send a file using V2 binary protocol.
async fn send_file_v2(
    stream: &mut TcpStream,
    transfer_id: &str,
    task_id: &str,
    relative_path: &str,
    file_path: &Path,
    total_bytes: u64,
    before_hash: &SourceFileState,
) -> Result<FileTransferOutcome> {
    let transfer_start = Instant::now();
    let mut timing = V2TransferTiming::default();
    let result = send_file_v2_inner(
        stream,
        transfer_id,
        task_id,
        relative_path,
        file_path,
        total_bytes,
        before_hash,
        &mut timing,
        transfer_start,
    )
    .await;
    let elapsed_ms = match &result {
        Ok(outcome) => outcome.elapsed_ms,
        Err(_) => elapsed_ms(transfer_start),
    };
    let error = result.as_ref().err().map(|e| e.to_string());
    timing.log_summary(
        transfer_id,
        task_id,
        relative_path,
        "upload",
        total_bytes,
        elapsed_ms,
        TRANSFER_V2_ACK_INTERVAL_BYTES,
        result.is_ok(),
        error.as_deref(),
    );
    result
}

async fn send_file_v2_inner(
    stream: &mut TcpStream,
    transfer_id: &str,
    task_id: &str,
    relative_path: &str,
    file_path: &Path,
    total_bytes: u64,
    before_hash: &SourceFileState,
    timing: &mut V2TransferTiming,
    first_byte: Instant,
) -> Result<FileTransferOutcome> {
    let write_start = Instant::now();
    stream
        .write_all(&encode_message(&SyncMessage::FileStreamStartV2 {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
            total_bytes,
        })?)
        .await?;
    timing.socket_write_ms += elapsed_ms(write_start);
    let mut hasher = blake3::Hasher::new();
    let mut offset = 0u64;
    let mut next_ack_at = TRANSFER_V2_ACK_INTERVAL_BYTES;
    let mut next_progress_at = TRANSFER_PROGRESS_INTERVAL_BYTES;
    let mut reader = spawn_v2_file_reader(
        task_id.to_string(),
        relative_path.to_string(),
        "upload".to_string(),
        file_path.to_path_buf(),
    );
    while let Some(read_result) = reader.recv().await {
        if is_transfer_cancelled(task_id, relative_path, "upload") {
            let _ = stream
                .write_all(&encode_message(&SyncMessage::TransferCancel {
                    task_id: task_id.to_string(),
                    relative_path: relative_path.to_string(),
                    direction: Some("receive".to_string()),
                })?)
                .await;
            anyhow::bail!("transfer cancelled");
        }
        let read_chunk = read_result.map_err(anyhow::Error::msg)?;
        if read_chunk.offset != offset {
            anyhow::bail!("unexpected v2 read offset");
        }
        timing.read_ms += read_chunk.read_ms;
        let read = read_chunk.data.len();
        let hash_start = Instant::now();
        hasher.update(&read_chunk.data);
        timing.hash_ms += elapsed_ms(hash_start);
        let need_ack = offset + read as u64 >= next_ack_at || offset + read as u64 >= total_bytes;
        let header = SyncMessage::FileChunkBinaryV2 {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
            offset,
            bytes: read as u32,
            ack: need_ack,
        };
        let write_start = Instant::now();
        crate::transport::protocol::write_v2_chunk(stream, &header, &read_chunk.data).await?;
        let write_ms = elapsed_ms(write_start);
        timing.socket_write_ms += write_ms;
        timing.chunk_socket_write_ms += write_ms;
        timing.chunk_count += 1;
        offset += read as u64;
        let throttle_start = Instant::now();
        throttle(read as u64).await;
        timing.throttle_ms += elapsed_ms(throttle_start);
        if need_ack {
            let ack_start = Instant::now();
            match read_message(stream).await? {
                SyncMessage::FileStreamAckV2 {
                    success: true,
                    received_bytes,
                    ..
                } => {
                    if received_bytes < offset {
                        anyhow::bail!("v2 checkpoint ack behind stream offset");
                    }
                }
                SyncMessage::FileStreamAckV2 {
                    success: false,
                    error,
                    ..
                } => {
                    anyhow::bail!(error.unwrap_or_else(|| "peer rejected v2 chunk".to_string()))
                }
                other => anyhow::bail!("unexpected v2 ack response: {:?}", other),
            }
            timing.ack_wait_ms += elapsed_ms(ack_start);
            next_ack_at += TRANSFER_V2_ACK_INTERVAL_BYTES;
        }
        if offset >= next_progress_at {
            let elapsed_ms = first_byte.elapsed().as_millis() as u64;
            let mbps = if elapsed_ms > 0 {
                (offset as f64 / (1024.0 * 1024.0)) / (elapsed_ms as f64 / 1000.0)
            } else {
                0.0
            };
            tracing::info!(
                task_id = %task_id,
                relative_path = %relative_path,
                direction = "upload",
                bytes_total = total_bytes,
                bytes_done = offset,
                elapsed_ms = elapsed_ms,
                mbps = format_args!("{:.1}", mbps),
                protocol_version = "v2",
                ack_interval_bytes = TRANSFER_V2_ACK_INTERVAL_BYTES,
                "transfer progress"
            );
            record_throttled(
                transfer_id,
                &task_id,
                &relative_path,
                "upload",
                offset,
                total_bytes,
                offset,
                mbps,
                "v2_binary",
            );
            next_progress_at += TRANSFER_PROGRESS_INTERVAL_BYTES;
        }
    }
    ensure_source_file_unchanged(file_path, before_hash)?;
    if offset != total_bytes {
        anyhow::bail!("v2 stream size mismatch before end");
    }
    let file_hash = hasher.finalize().to_hex().to_string();
    let write_start = Instant::now();
    stream
        .write_all(&encode_message(&SyncMessage::FileStreamEndV2 {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
            file_hash: file_hash.clone(),
        })?)
        .await?;
    timing.socket_write_ms += elapsed_ms(write_start);
    let total_elapsed_ms = first_byte.elapsed().as_millis() as u64;
    let ack_start = Instant::now();
    match read_message(stream).await? {
        SyncMessage::FileStreamAckV2 { success: true, .. } => {
            timing.ack_wait_ms += elapsed_ms(ack_start);
            Ok(FileTransferOutcome {
                blake3_hash: file_hash,
                protocol: "v2_binary",
                elapsed_ms: total_elapsed_ms,
            })
        }
        SyncMessage::FileStreamAckV2 {
            success: false,
            error,
            ..
        } => anyhow::bail!(error.unwrap_or_else(|| "v2 stream end rejected".to_string())),
        other => anyhow::bail!("unexpected v2 end ack: {:?}", other),
    }
}
pub async fn send_authenticated_file_to_peer(
    manager: &ConnectionManager,
    local_identity: &DeviceIdentity,
    device_id: &str,
    task_id: impl Into<String>,
    relative_path: impl Into<String>,
    file_path: &Path,
) -> Result<FileTransferOutcome> {
    let task_id = task_id.into();
    let relative_path = relative_path.into();
    clear_transfer_cancel(&task_id, &relative_path, Some("upload"));
    let total_start = Instant::now();
    let before_hash = source_file_state(file_path)?;
    let total_bytes = before_hash.len;
    let transfer_id = new_transfer_id();
    let _progress_guard = TransferProgressGuard::new(transfer_id.clone());
    record_transfer_progress(TransferProgress {
        transfer_id: transfer_id.clone(),
        task_id: task_id.clone(),
        relative_path: relative_path.clone(),
        direction: "upload".to_string(),
        bytes_done: 0,
        bytes_total: total_bytes,
        mbps: 0.0,
        wire_bytes: 0,
        protocol_version: String::new(),
        finished: false,
        finished_at_unix_ms: None,
    });
    let stream_start = Instant::now();
    let mut stream = open_authenticated_stream(manager, local_identity, device_id).await?;
    let stream_elapsed_ms = stream_start.elapsed().as_millis() as u64;
    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "upload",
        bytes_total = total_bytes,
        stream_open_ms = stream_elapsed_ms,
        "authenticated stream open"
    );
    // Check cached protocol version - skip negotiation on subsequent transfers.
    let mut cache_v1 = false;
    let mut reopen_for_v1 = false;
    let use_v2 = match get_cached_protocol(device_id) {
        Some(2) => {
            tracing::debug!(
                selected_protocol = "v2_binary",
                peer = %device_id,
                "using cached V2"
            );
            true
        }
        Some(1) => false,
        _ => match try_negotiate_v2(&mut stream).await {
            Ok(true) => true,
            Ok(false) => {
                if force_v2_enabled() {
                    anyhow::bail!("LANBRIDGE_FORCE_V2: peer selected V1 for {}", device_id);
                }
                cache_v1 = true;
                tracing::info!(
                    selected_protocol = "v1_json",
                    fallback_reason = "peer_v2_unsupported",
                    "peer does not support V2, using V1"
                );
                false
            }
            Err(e) => {
                if force_v2_enabled() {
                    anyhow::bail!(
                        "LANBRIDGE_FORCE_V2: V2 negotiation failed for {}: {}",
                        device_id,
                        e
                    );
                }
                tracing::info!(
                    selected_protocol = "v1_json",
                    fallback_reason = format_args!("negotiation_error: {}", e),
                    "V2 negotiation failed, reopening stream for V1"
                );
                reopen_for_v1 = true;
                false
            }
        },
    };
    if use_v2 {
        set_cached_protocol(device_id, 2);
        tracing::info!(selected_protocol = "v2_binary", "using V2");
        let result = send_file_v2(
            &mut stream,
            &transfer_id,
            &task_id,
            &relative_path,
            file_path,
            total_bytes,
            &before_hash,
        )
        .await;
        if result.is_err() {
            clear_cached_protocol(device_id);
        }
        let total_elapsed_ms = total_start.elapsed().as_millis() as u64;
        let total_mbps = if total_elapsed_ms > 0 {
            (total_bytes as f64 / (1024.0 * 1024.0)) / (total_elapsed_ms as f64 / 1000.0)
        } else {
            0.0
        };
        match &result {
            Ok(outcome) => {
                tracing::info!(
                    task_id = %task_id,
                    relative_path = %relative_path,
                    direction = "upload",
                    bytes_total = total_bytes,
                    elapsed_ms = total_elapsed_ms,
                    mbps = format_args!("{:.1}", total_mbps),
                    protocol_version = "v2",
                    "transfer complete"
                );
                tracing::info!(
                    transfer_summary = true,
                    file_path = %relative_path,
                    file_size = total_bytes,
                    protocol = "v2_binary",
                    direction = "upload",
                    elapsed_ms = total_elapsed_ms,
                    avg_mbps = format_args!("{:.1}", total_mbps),
                );
                tracing::debug!(
                    task_id = %task_id,
                    relative_path = %relative_path,
                    transfer_hash = %outcome.blake3_hash,
                    protocol = outcome.protocol,
                    elapsed_ms = outcome.elapsed_ms,
                    "transfer outcome hash ready"
                );
            }
            Err(e) => tracing::warn!(
                task_id = %task_id,
                relative_path = %relative_path,
                error = %e,
                "v2 transfer failed"
            ),
        }
        return result;
    }
    if cache_v1 {
        set_cached_protocol(device_id, 1);
    }
    if reopen_for_v1 {
        stream = open_authenticated_stream(manager, local_identity, device_id).await?;
    }
    // Use checkpoint ACKs only when the peer negotiated/cached the newer V1 behavior.
    // Negotiation failure means the peer may be an older V1-only build that ACKs every chunk.
    let use_v1_checkpoint_acks = !reopen_for_v1;
    let legacy_file_hash = if use_v1_checkpoint_acks {
        None
    } else {
        let hash_start = Instant::now();
        let file_hash = crate::core::scanner::hash_file(file_path)?;
        ensure_source_file_unchanged(file_path, &before_hash)?;
        tracing::info!(
            task_id = %task_id,
            relative_path = %relative_path,
            direction = "upload",
            bytes_total = total_bytes,
            elapsed_ms = hash_start.elapsed().as_millis() as u64,
            protocol_version = "v1",
            "file hash complete"
        );
        Some(file_hash)
    };
    send_and_expect_file_ack(
        &mut stream,
        SyncMessage::FileChunkStart {
            task_id: task_id.clone(),
            relative_path: relative_path.clone(),
            file_hash: legacy_file_hash.clone().unwrap_or_default(),
            total_bytes,
        },
    )
    .await?;
    let first_byte = Instant::now();
    let mut file = std::fs::File::open(file_path)?;
    let mut hasher = blake3::Hasher::new();
    let mut offset = 0u64;
    let mut next_ack_at = TRANSFER_V1_ACK_INTERVAL_BYTES;
    let mut next_progress_at = TRANSFER_PROGRESS_INTERVAL_BYTES;
    let mut buf = vec![0u8; TRANSFER_V1_CHUNK_SIZE];
    loop {
        if is_transfer_cancelled(&task_id, &relative_path, "upload") {
            let _ = stream
                .write_all(&encode_message(&SyncMessage::TransferCancel {
                    task_id: task_id.clone(),
                    relative_path: relative_path.clone(),
                    direction: Some("receive".to_string()),
                })?)
                .await;
            anyhow::bail!("transfer cancelled");
        }
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        let chunk = &buf[..read];
        hasher.update(chunk);
        let message = SyncMessage::FileChunk {
            task_id: task_id.clone(),
            relative_path: relative_path.clone(),
            offset,
            data: chunk.to_vec(),
        };
        if use_v1_checkpoint_acks {
            stream.write_all(&encode_message(&message)?).await?;
        } else {
            send_and_expect_file_ack(&mut stream, message).await?;
        }
        offset += read as u64;
        throttle(read as u64).await;
        if use_v1_checkpoint_acks && (offset >= next_ack_at || offset >= total_bytes) {
            match read_message(&mut stream).await? {
                SyncMessage::FileChunkAck {
                    success: true,
                    received_bytes,
                    ..
                } if received_bytes == offset => {}
                SyncMessage::FileChunkAck {
                    success: true,
                    received_bytes,
                    ..
                } => anyhow::bail!(
                    "unexpected v1 checkpoint ack offset: expected {}, got {}",
                    offset,
                    received_bytes
                ),
                SyncMessage::FileChunkAck { error, .. } => {
                    anyhow::bail!(error.unwrap_or_else(|| "peer rejected v1 chunk".to_string()))
                }
                SyncMessage::FileAck { success: true, .. } => {}
                SyncMessage::FileAck { error, .. } => {
                    anyhow::bail!(error.unwrap_or_else(|| "peer rejected v1 chunk".to_string()))
                }
                other => anyhow::bail!("unexpected v1 checkpoint response: {:?}", other),
            }
            while next_ack_at <= offset {
                next_ack_at += TRANSFER_V1_ACK_INTERVAL_BYTES;
            }
        }
        if offset >= next_progress_at {
            let elapsed_ms = first_byte.elapsed().as_millis() as u64;
            let mbps = if elapsed_ms > 0 {
                (offset as f64 / (1024.0 * 1024.0)) / (elapsed_ms as f64 / 1000.0)
            } else {
                0.0
            };
            tracing::info!(
                task_id = %task_id,
                relative_path = %relative_path,
                direction = "upload",
                bytes_total = total_bytes,
                bytes_done = offset,
                elapsed_ms = elapsed_ms,
                mbps = format_args!("{:.1}", mbps),
                protocol_version = "v1",
                chunk_size = TRANSFER_V1_CHUNK_SIZE,
                ack_interval_bytes = TRANSFER_V1_ACK_INTERVAL_BYTES,
                "transfer progress"
            );
            record_throttled(
                &transfer_id,
                &task_id,
                &relative_path,
                "upload",
                offset,
                total_bytes,
                offset,
                mbps,
                "v1_json",
            );
            next_progress_at += TRANSFER_PROGRESS_INTERVAL_BYTES;
        }
    }
    ensure_source_file_unchanged(file_path, &before_hash)?;
    let streamed_hash = hasher.finalize().to_hex().to_string();
    if let Some(file_hash) = legacy_file_hash.as_ref() {
        if streamed_hash != *file_hash {
            anyhow::bail!("source file hash changed while streaming");
        }
    }
    send_and_expect_file_ack(
        &mut stream,
        SyncMessage::FileChunkEnd {
            task_id: task_id.clone(),
            relative_path: relative_path.clone(),
            file_hash: if use_v1_checkpoint_acks {
                Some(streamed_hash.clone())
            } else {
                None
            },
        },
    )
    .await?;
    let total_elapsed_ms = total_start.elapsed().as_millis() as u64;
    let total_mbps = if total_elapsed_ms > 0 {
        (total_bytes as f64 / (1024.0 * 1024.0)) / (total_elapsed_ms as f64 / 1000.0)
    } else {
        0.0
    };
    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "upload",
        bytes_total = total_bytes,
        bytes_done = offset,
        elapsed_ms = total_elapsed_ms,
        mbps = format_args!("{:.1}", total_mbps),
        protocol_version = "v1",
        "transfer complete"
    );
    tracing::info!(
        transfer_summary = true,
        file_path = %relative_path,
        file_size = total_bytes,
        protocol = "v1_json",
        direction = "upload",
        elapsed_ms = total_elapsed_ms,
        avg_mbps = format_args!("{:.1}", total_mbps),
    );
    Ok(FileTransferOutcome {
        blake3_hash: streamed_hash,
        protocol: "v1_json",
        elapsed_ms: total_elapsed_ms,
    })
}
pub(crate) fn source_file_state(file_path: &Path) -> Result<SourceFileState> {
    let metadata = std::fs::metadata(file_path)?;
    if !metadata.is_file() {
        anyhow::bail!("source path is not a file");
    }
    Ok(SourceFileState {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}
pub(crate) fn ensure_source_file_unchanged(
    file_path: &Path,
    expected: &SourceFileState,
) -> Result<()> {
    let current = source_file_state(file_path)?;
    if &current != expected {
        anyhow::bail!("source file changed while preparing transfer");
    }
    Ok(())
}

pub(crate) async fn wait_for_source_file_stability(file_path: &Path) -> Result<SourceFileState> {
    const RECENT_WRITE_WINDOW: Duration = Duration::from_millis(1500);
    const SAMPLE_DELAY: Duration = Duration::from_millis(700);

    let first = source_file_state(file_path)?;
    let recent_write = first
        .modified
        .and_then(|modified| modified.elapsed().ok())
        .map_or(true, |age| age < RECENT_WRITE_WINDOW);
    if !recent_write {
        return Ok(first);
    }

    tokio::time::sleep(SAMPLE_DELAY).await;
    let second = source_file_state(file_path)?;
    if first != second {
        anyhow::bail!("source file is still changing; retry later");
    }
    Ok(second)
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
) -> Result<FileTransferOutcome> {
    let task_id = task_id.into();
    let relative_path = relative_path.into();
    clear_transfer_cancel(&task_id, &relative_path, Some("download"));
    let total_start = Instant::now();
    let stream_start = Instant::now();
    let mut stream = open_authenticated_stream(manager, local_identity, device_id).await?;
    let stream_elapsed_ms = stream_start.elapsed().as_millis() as u64;
    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "download",
        stream_open_ms = stream_elapsed_ms,
        "authenticated stream open"
    );
    let mut cache_v1 = false;
    let mut reopen_for_v1 = false;
    let use_v2 = match get_cached_protocol(device_id) {
        Some(2) => {
            tracing::debug!(
                selected_protocol = "v2_binary",
                peer = %device_id,
                "using cached V2"
            );
            true
        }
        Some(1) => false,
        _ => match try_negotiate_v2(&mut stream).await {
            Ok(true) => true,
            Ok(false) => {
                if force_v2_enabled() {
                    anyhow::bail!("LANBRIDGE_FORCE_V2: peer selected V1 for {}", device_id);
                }
                cache_v1 = true;
                tracing::info!(
                    selected_protocol = "v1_json",
                    fallback_reason = "peer_v2_unsupported",
                    "peer does not support V2, using V1"
                );
                false
            }
            Err(e) => {
                if force_v2_enabled() {
                    anyhow::bail!(
                        "LANBRIDGE_FORCE_V2: V2 negotiation failed for {}: {}",
                        device_id,
                        e
                    );
                }
                tracing::info!(
                    selected_protocol = "v1_json",
                    fallback_reason = format_args!("negotiation_error: {}", e),
                    "V2 negotiation failed, reopening stream for V1"
                );
                reopen_for_v1 = true;
                false
            }
        },
    };
    if use_v2 {
        set_cached_protocol(device_id, 2);
        tracing::info!(selected_protocol = "v2_binary", "using V2");
        let result = request_file_v2(
            &mut stream,
            &task_id,
            &relative_path,
            target_path,
            total_start,
        )
        .await;
        if result.is_err() {
            clear_cached_protocol(device_id);
        }
        return result;
    }
    if cache_v1 {
        set_cached_protocol(device_id, 1);
    }
    if reopen_for_v1 {
        stream = open_authenticated_stream(manager, local_identity, device_id).await?;
    }
    // Use an authenticated stream for V1.
    stream
        .write_all(&encode_message(&SyncMessage::FileDownloadRequest {
            task_id: task_id.clone(),
            relative_path: relative_path.clone(),
        })?)
        .await?;
    let first_byte = Instant::now();
    let (mut expected_hash, total_bytes) = match read_message(&mut stream).await? {
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
    let transfer_id = new_transfer_id();
    let _progress_guard = TransferProgressGuard::new(transfer_id.clone());
    record_transfer_progress(TransferProgress {
        transfer_id: transfer_id.clone(),
        task_id: task_id.clone(),
        relative_path: relative_path.clone(),
        direction: "download".to_string(),
        bytes_done: 0,
        bytes_total: total_bytes,
        mbps: 0.0,
        wire_bytes: 0,
        protocol_version: String::new(),
        finished: false,
        finished_at_unix_ms: None,
    });
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial_path = partial_path(target_path);
    let mut file = std::fs::File::create(&partial_path)?;
    let mut hasher = blake3::Hasher::new();
    let mut written = 0u64;
    let mut next_progress_at = TRANSFER_PROGRESS_INTERVAL_BYTES;
    loop {
        match read_message(&mut stream).await? {
            SyncMessage::FileChunk {
                task_id: chunk_task,
                relative_path: chunk_path,
                offset,
                data,
            } if chunk_task == task_id && chunk_path == relative_path => {
                if is_transfer_cancelled(&task_id, &relative_path, "download") {
                    let _ = std::fs::remove_file(&partial_path);
                    let _ = stream
                        .write_all(&encode_message(&SyncMessage::TransferCancel {
                            task_id: task_id.clone(),
                            relative_path: relative_path.clone(),
                            direction: Some("serve".to_string()),
                        })?)
                        .await;
                    anyhow::bail!("transfer cancelled");
                }
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
                if written >= next_progress_at {
                    let elapsed_ms = first_byte.elapsed().as_millis() as u64;
                    let mbps = if elapsed_ms > 0 {
                        (written as f64 / (1024.0 * 1024.0)) / (elapsed_ms as f64 / 1000.0)
                    } else {
                        0.0
                    };
                    tracing::info!(
                        task_id = %task_id,
                        relative_path = %relative_path,
                        direction = "download",
                        bytes_total = total_bytes,
                        bytes_done = written,
                        elapsed_ms = elapsed_ms,
                        mbps = format_args!("{:.1}", mbps),
                        protocol_version = "v1",
                        "transfer progress"
                    );
                    record_throttled(
                        &transfer_id,
                        &task_id,
                        &relative_path,
                        "download",
                        written,
                        total_bytes,
                        written,
                        mbps,
                        "v1_json",
                    );
                    next_progress_at += TRANSFER_PROGRESS_INTERVAL_BYTES;
                }
            }
            SyncMessage::FileChunkEnd {
                task_id: end_task,
                relative_path: end_path,
                file_hash: end_hash,
            } if end_task == task_id && end_path == relative_path => {
                if let Some(ref hash) = end_hash {
                    if !hash.is_empty() {
                        expected_hash = hash.clone();
                    }
                }
                break;
            }
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
    let total_elapsed_ms = total_start.elapsed().as_millis() as u64;
    let total_mbps = if total_elapsed_ms > 0 {
        (total_bytes as f64 / (1024.0 * 1024.0)) / (total_elapsed_ms as f64 / 1000.0)
    } else {
        0.0
    };
    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "download",
        bytes_total = total_bytes,
        bytes_done = written,
        elapsed_ms = total_elapsed_ms,
        mbps = format_args!("{:.1}", total_mbps),
        protocol_version = "v1",
        "transfer complete"
    );
    tracing::info!(
        transfer_summary = true,
        file_path = %relative_path,
        file_size = total_bytes,
        protocol = "v1_json",
        direction = "download",
        elapsed_ms = total_elapsed_ms,
        avg_mbps = format_args!("{:.1}", total_mbps),
    );
    Ok(FileTransferOutcome {
        blake3_hash: actual_hash,
        protocol: "v1_json",
        elapsed_ms: total_elapsed_ms,
    })
}
async fn request_file_v2(
    stream: &mut TcpStream,
    task_id: &str,
    relative_path: &str,
    target_path: &Path,
    total_start: Instant,
) -> Result<FileTransferOutcome> {
    stream
        .write_all(&encode_message(&SyncMessage::FileDownloadRequestV2 {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
        })?)
        .await?;
    let first_byte = Instant::now();
    let total_bytes = match read_message(stream).await? {
        SyncMessage::FileStreamStartV2 {
            task_id: ack_task,
            relative_path: ack_path,
            total_bytes,
        } if ack_task == task_id && ack_path == relative_path => total_bytes,
        SyncMessage::FileAck { error, .. } => {
            anyhow::bail!(error.unwrap_or_else(|| "peer rejected v2 file download".to_string()))
        }
        other => anyhow::bail!("unexpected v2 download response: {:?}", other),
    };
    let transfer_id = new_transfer_id();
    let _progress_guard = TransferProgressGuard::new(transfer_id.clone());
    record_transfer_progress(TransferProgress {
        transfer_id: transfer_id.clone(),
        task_id: task_id.to_string(),
        relative_path: relative_path.to_string(),
        direction: "download".to_string(),
        bytes_done: 0,
        bytes_total: total_bytes,
        mbps: 0.0,
        wire_bytes: 0,
        protocol_version: "v2_binary".to_string(),
        finished: false,
        finished_at_unix_ms: None,
    });
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial_path = partial_path(target_path);
    let mut file = std::fs::File::create(&partial_path)?;
    let mut hasher = blake3::Hasher::new();
    let mut written = 0u64;
    let mut next_progress_at = TRANSFER_PROGRESS_INTERVAL_BYTES;
    let expected_hash = loop {
        match read_message(stream).await? {
            SyncMessage::FileChunkBinaryV2 {
                task_id: chunk_task,
                relative_path: chunk_path,
                offset,
                bytes,
                ack,
            } if chunk_task == task_id && chunk_path == relative_path => {
                if is_transfer_cancelled(task_id, relative_path, "download") {
                    let _ = std::fs::remove_file(&partial_path);
                    let _ = stream
                        .write_all(&encode_message(&SyncMessage::TransferCancel {
                            task_id: task_id.to_string(),
                            relative_path: relative_path.to_string(),
                            direction: Some("serve".to_string()),
                        })?)
                        .await;
                    anyhow::bail!("transfer cancelled");
                }
                if offset != written {
                    let _ = std::fs::remove_file(&partial_path);
                    send_v2_download_ack(
                        stream,
                        task_id,
                        relative_path,
                        written,
                        false,
                        Some("unexpected download chunk offset".to_string()),
                    )
                    .await?;
                    anyhow::bail!("unexpected download chunk offset");
                }
                let data =
                    crate::transport::protocol::read_v2_payload(stream, bytes as usize).await?;
                hasher.update(&data);
                file.write_all(&data)?;
                written += bytes as u64;
                if written > total_bytes {
                    let _ = std::fs::remove_file(&partial_path);
                    send_v2_download_ack(
                        stream,
                        task_id,
                        relative_path,
                        written,
                        false,
                        Some("download exceeded expected size".to_string()),
                    )
                    .await?;
                    anyhow::bail!("download exceeded expected size");
                }
                if ack {
                    send_v2_download_ack(stream, task_id, relative_path, written, true, None)
                        .await?;
                }
                if written >= next_progress_at {
                    let elapsed_ms = first_byte.elapsed().as_millis() as u64;
                    let mbps = if elapsed_ms > 0 {
                        (written as f64 / (1024.0 * 1024.0)) / (elapsed_ms as f64 / 1000.0)
                    } else {
                        0.0
                    };
                    tracing::info!(
                        task_id = %task_id,
                        relative_path = %relative_path,
                        direction = "download",
                        bytes_total = total_bytes,
                        bytes_done = written,
                        elapsed_ms = elapsed_ms,
                        mbps = format_args!("{:.1}", mbps),
                        protocol_version = "v2",
                        ack_interval_bytes = TRANSFER_V2_ACK_INTERVAL_BYTES,
                        "transfer progress"
                    );
                    record_throttled(
                        &transfer_id,
                        &task_id,
                        &relative_path,
                        "download",
                        written,
                        total_bytes,
                        written,
                        mbps,
                        "v2_binary",
                    );
                    next_progress_at += TRANSFER_PROGRESS_INTERVAL_BYTES;
                }
            }
            SyncMessage::FileStreamEndV2 {
                task_id: end_task,
                relative_path: end_path,
                file_hash,
            } if end_task == task_id && end_path == relative_path => break file_hash,
            SyncMessage::FileAck { error, .. } => {
                let _ = std::fs::remove_file(&partial_path);
                anyhow::bail!(error.unwrap_or_else(|| "peer rejected v2 file download".to_string()));
            }
            other => {
                let _ = std::fs::remove_file(&partial_path);
                anyhow::bail!("unexpected v2 download message: {:?}", other);
            }
        }
    };
    file.flush()?;
    drop(file);
    if written != total_bytes {
        let _ = std::fs::remove_file(&partial_path);
        send_v2_download_ack(
            stream,
            task_id,
            relative_path,
            written,
            false,
            Some("download size mismatch".to_string()),
        )
        .await?;
        anyhow::bail!("download size mismatch");
    }
    let actual_hash = hasher.finalize().to_hex().to_string();
    if actual_hash != expected_hash {
        let _ = std::fs::remove_file(&partial_path);
        send_v2_download_ack(
            stream,
            task_id,
            relative_path,
            written,
            false,
            Some("download hash mismatch".to_string()),
        )
        .await?;
        anyhow::bail!("download hash mismatch");
    }
    std::fs::rename(partial_path, target_path)?;
    send_v2_download_ack(stream, task_id, relative_path, written, true, None).await?;
    let total_elapsed_ms = total_start.elapsed().as_millis() as u64;
    let total_mbps = if total_elapsed_ms > 0 {
        (total_bytes as f64 / (1024.0 * 1024.0)) / (total_elapsed_ms as f64 / 1000.0)
    } else {
        0.0
    };
    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "download",
        bytes_total = total_bytes,
        bytes_done = written,
        elapsed_ms = total_elapsed_ms,
        mbps = format_args!("{:.1}", total_mbps),
        protocol_version = "v2",
        "transfer complete"
    );
    tracing::info!(
        transfer_summary = true,
        file_path = %relative_path,
        file_size = total_bytes,
        protocol = "v2_binary",
        direction = "download",
        elapsed_ms = total_elapsed_ms,
        avg_mbps = format_args!("{:.1}", total_mbps),
    );
    Ok(FileTransferOutcome {
        blake3_hash: actual_hash,
        protocol: "v2_binary",
        elapsed_ms: total_elapsed_ms,
    })
}
async fn send_v2_download_ack(
    stream: &mut TcpStream,
    task_id: &str,
    relative_path: &str,
    received_bytes: u64,
    success: bool,
    error: Option<String>,
) -> Result<()> {
    stream
        .write_all(&encode_message(&SyncMessage::FileStreamAckV2 {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
            received_bytes,
            success,
            error,
        })?)
        .await?;
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
    tracing::debug!(
        device_id = %device_id,
        target = %peer.address,
        "connecting to peer for authenticated stream"
    );
    let mut stream = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect(&peer.address),
    )
    .await
    {
        Ok(Ok(stream)) => {
            manager.mark_connected(device_id);
            stream
        }
        Ok(Err(e)) => {
            manager.disconnect(device_id);
            anyhow::bail!(
                "failed to connect to peer {} at {}: {}",
                device_id,
                peer.address,
                e
            )
        }
        Err(_) => {
            manager.disconnect(device_id);
            anyhow::bail!(
                "timed out connecting to peer {} at {} after 5s",
                device_id,
                peer.address
            )
        }
    };
    authenticate_stream(&mut stream, local_identity).await?;
    Ok(stream)
}
async fn authenticate_stream(
    stream: &mut TcpStream,
    local_identity: &DeviceIdentity,
) -> Result<()> {
    let local_device_id = local_identity.public().device_id;
    stream
        .write_all(&encode_message(&SyncMessage::AuthHello {
            device_id: local_device_id.clone(),
        })?)
        .await?;
    let challenge = match read_message(stream).await? {
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
    match read_message(stream).await? {
        SyncMessage::AuthOk { .. } => Ok(()),
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
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    #[test]
    fn source_stability_detects_file_changes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("growing.txt");
        std::fs::write(&path, "first").unwrap();
        let before = source_file_state(&path).unwrap();
        std::fs::write(&path, "first plus more").unwrap();
        assert!(ensure_source_file_unchanged(&path, &before).is_err());
    }

    #[test]
    fn progress_entries_include_direction_in_key() {
        let task_id = uuid::Uuid::new_v4().to_string();
        let relative_path = "same.zip";
        finish_transfer_progress_for_path(&task_id, relative_path, None);
        assert!(!has_active_transfers());
        let upload_id = new_transfer_id();
        let download_id = new_transfer_id();

        record_transfer_progress(TransferProgress {
            transfer_id: upload_id.clone(),
            task_id: task_id.clone(),
            relative_path: relative_path.to_string(),
            direction: "upload".to_string(),
            bytes_done: 10,
            bytes_total: 100,
            wire_bytes: 10,
            mbps: 1.0,
            finished: false,
            protocol_version: "v2_binary".to_string(),
            finished_at_unix_ms: None,
        });
        record_transfer_progress(TransferProgress {
            transfer_id: download_id.clone(),
            task_id: task_id.clone(),
            relative_path: relative_path.to_string(),
            direction: "download".to_string(),
            bytes_done: 20,
            bytes_total: 100,
            wire_bytes: 20,
            mbps: 2.0,
            finished: false,
            protocol_version: "v2_binary".to_string(),
            finished_at_unix_ms: None,
        });

        let entries = get_transfer_progress()
            .into_iter()
            .filter(|entry| entry.task_id == task_id && entry.relative_path == relative_path)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry.direction == "upload"));
        assert!(entries.iter().any(|entry| entry.direction == "download"));
        assert!(has_active_transfers());

        finish_transfer_progress(&upload_id);
        let entries = get_transfer_progress()
            .into_iter()
            .filter(|entry| entry.task_id == task_id && entry.relative_path == relative_path)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|entry| entry.transfer_id == upload_id && entry.finished));
        assert!(entries
            .iter()
            .any(|entry| entry.transfer_id == download_id && !entry.finished));

        finish_transfer_progress_for_path(&task_id, relative_path, None);
        assert!(!has_active_transfers());
    }

    #[test]
    fn deferred_and_cancelled_transfers_are_direction_specific() {
        let task_id = uuid::Uuid::new_v4().to_string();
        let relative_path = "same.zip";
        resume_deferred_transfer(&task_id, relative_path, None);
        clear_transfer_cancel(&task_id, relative_path, None);

        defer_transfer(&task_id, relative_path, "upload");
        assert!(is_transfer_deferred(&task_id, relative_path, "upload"));
        assert!(!is_transfer_deferred(&task_id, relative_path, "download"));

        cancel_active_transfer(&task_id, relative_path, Some("receive"));
        assert!(is_transfer_cancelled(&task_id, relative_path, "receive"));
        assert!(!is_transfer_cancelled(&task_id, relative_path, "upload"));

        resume_deferred_transfer(&task_id, relative_path, Some("upload"));
        clear_transfer_cancel(&task_id, relative_path, Some("receive"));
        assert!(!is_transfer_deferred(&task_id, relative_path, "upload"));
        assert!(!is_transfer_cancelled(&task_id, relative_path, "receive"));
    }
}
pub fn pin_connected_peer(
    manager: &ConnectionManager,
    address: &str,
    port: u16,
    peer: Option<PublicIdentity>,
) -> Result<String> {
    let identity = peer.ok_or_else(|| anyhow::anyhow!("peer identity is required"))?;
    if identity.device_id.is_empty() || identity.public_key.is_empty() {
        anyhow::bail!("peer identity is incomplete");
    }
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
    Ok(device_id)
}
