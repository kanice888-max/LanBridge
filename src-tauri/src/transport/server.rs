use anyhow::Result;
use ed25519_dalek::Signature;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

use crate::core::model::{
    EntryKind, FileSnapshot, HashStatus, HistoryEntry, LogEntry, LogLevel, SyncBaseline,
};
use crate::core::path_safety::safe_join;
use crate::history::store::HistoryStore;
use crate::pairing::{DeviceIdentity, PublicIdentity};
use crate::state::{db, repository};
use crate::transport::connection::{self, auth_payload};
use crate::transport::protocol::{
    self, RemoteFileState, TRANSFER_PROGRESS_INTERVAL_BYTES, TRANSFER_V1_ACK_INTERVAL_BYTES,
    TRANSFER_V1_CHUNK_SIZE, TRANSFER_V2_ACK_INTERVAL_BYTES, TRANSFER_V2_CHUNK_SIZE,
};

const PRIMARY_NON_EMPTY_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Maps negotiated sync task IDs to the local root that should receive files.
#[derive(Clone, Default)]
pub struct TaskRootRegistry {
    roots: Arc<Mutex<HashMap<String, PathBuf>>>,
    trusted_peers: Arc<Mutex<HashMap<String, PublicIdentity>>>,
    incoming: Arc<Mutex<HashMap<String, IncomingTransfer>>>,
    persistence_path: Arc<Mutex<Option<PathBuf>>>,
    task_invite_persistence_path: Arc<Mutex<Option<PathBuf>>>,
    local_identity: Arc<Mutex<Option<PublicIdentity>>>,
    task_invite_inbox_root: Arc<Mutex<Option<PathBuf>>>,
    auto_accept_task_invites: Arc<Mutex<bool>>,
    task_invites: Arc<Mutex<HashMap<String, PendingTaskInvite>>>,
    state_db_path: Arc<Mutex<Option<PathBuf>>>,
}

/// Tracks an in-progress file reception. Not `Clone` — `file: std::fs::File` is not cloneable.
/// The handle is opened once at transfer start, reused for every chunk append, and flushed+dropped
/// before the final rename to avoid Windows file-lock errors.
#[derive(Debug)]
struct IncomingTransfer {
    transfer_id: String,
    partial_path: PathBuf,
    final_path: PathBuf,
    file_hash: String,
    total_bytes: u64,
    written_bytes: u64,
    hasher: blake3::Hasher,
    start_time: Instant,
    first_byte_time: Option<Instant>,
    next_progress_at: u64,
    next_ack_at: u64,
    ack_every_chunk: bool,
    protocol_version: &'static str,
    file: std::fs::File,
    timing: V2ReceiveTiming,
}

#[derive(Debug, Default)]
struct V2ReceiveTiming {
    payload_read_ms: u64,
    file_write_ms: u64,
    hash_ms: u64,
    flush_ms: u64,
    rename_ms: u64,
    ack_write_ms: u64,
    chunk_count: u64,
}

#[derive(Debug, Default)]
struct V2ServeTiming {
    read_ms: u64,
    hash_ms: u64,
    socket_write_ms: u64,
    chunk_socket_write_ms: u64,
    ack_wait_ms: u64,
    chunk_count: u64,
}

enum IncomingChunkAck {
    None,
    LegacyFileAck,
    Checkpoint(u64),
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn update_incoming_timing(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    update: impl FnOnce(&mut V2ReceiveTiming),
) {
    if let Ok(mut incoming) = task_roots.incoming.lock() {
        if let Some(transfer) = incoming.get_mut(&transfer_key(task_id, relative_path)) {
            update(&mut transfer.timing);
        }
    }
}

fn log_v2_receive_timing_summary(
    transfer_id: &str,
    task_id: &str,
    relative_path: &str,
    total_bytes: u64,
    elapsed_ms: u64,
    timing: &V2ReceiveTiming,
    success: bool,
    error: Option<&str>,
) {
    tracing::info!(
        transfer_timing_summary = true,
        transfer_id = %transfer_id,
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "receive",
        protocol = "v2_binary",
        success = success,
        error = error.unwrap_or(""),
        bytes_total = total_bytes,
        elapsed_ms = elapsed_ms,
        ack_interval_bytes = TRANSFER_V2_ACK_INTERVAL_BYTES,
        payload_read_ms = timing.payload_read_ms,
        file_write_ms = timing.file_write_ms,
        hash_ms = timing.hash_ms,
        flush_ms = timing.flush_ms,
        rename_ms = timing.rename_ms,
        ack_write_ms = timing.ack_write_ms,
        chunk_count = timing.chunk_count,
    );
}

fn log_v2_serve_timing_summary(
    transfer_id: &str,
    task_id: &str,
    relative_path: &str,
    total_bytes: u64,
    elapsed_ms: u64,
    timing: &V2ServeTiming,
    success: bool,
    error: Option<&str>,
) {
    let avg_chunk_write_ms = if timing.chunk_count > 0 {
        timing.chunk_socket_write_ms as f64 / timing.chunk_count as f64
    } else {
        0.0
    };
    tracing::info!(
        transfer_timing_summary = true,
        transfer_id = %transfer_id,
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "serve",
        protocol = "v2_binary",
        success = success,
        error = error.unwrap_or(""),
        bytes_total = total_bytes,
        elapsed_ms = elapsed_ms,
        ack_interval_bytes = TRANSFER_V2_ACK_INTERVAL_BYTES,
        read_ms = timing.read_ms,
        hash_ms = timing.hash_ms,
        socket_write_ms = timing.socket_write_ms,
        ack_wait_ms = timing.ack_wait_ms,
        chunk_count = timing.chunk_count,
        avg_chunk_write_ms = format_args!("{:.2}", avg_chunk_write_ms),
    );
}

async fn write_v2_stream_ack(
    writer: &mut (impl AsyncWrite + Unpin),
    task_roots: &TaskRootRegistry,
    ack: protocol::SyncMessage,
) -> Result<()> {
    let timing_key = match &ack {
        protocol::SyncMessage::FileStreamAckV2 {
            task_id,
            relative_path,
            ..
        } => Some((task_id.clone(), relative_path.clone())),
        _ => None,
    };
    let encoded = protocol::encode_message(&ack)?;
    let ack_start = Instant::now();
    writer.write_all(&encoded).await?;
    let ack_ms = elapsed_ms(ack_start);
    if let Some((task_id, relative_path)) = timing_key {
        update_incoming_timing(task_roots, &task_id, &relative_path, |timing| {
            timing.ack_write_ms += ack_ms;
        });
    }
    Ok(())
}

fn record_server_transfer_start(
    task_id: &str,
    relative_path: &str,
    direction: &str,
    total_bytes: u64,
    protocol_version: &str,
) -> String {
    let transfer_id = connection::new_transfer_id();
    connection::record_transfer_progress(connection::TransferProgress {
        transfer_id: transfer_id.clone(),
        task_id: task_id.to_string(),
        relative_path: relative_path.to_string(),
        direction: direction.to_string(),
        bytes_done: 0,
        bytes_total: total_bytes,
        wire_bytes: 0,
        mbps: 0.0,
        finished: false,
        protocol_version: protocol_version.to_string(),
        finished_at_unix_ms: None,
    });
    transfer_id
}

struct ServerTransferProgressGuard {
    transfer_id: String,
}

impl Drop for ServerTransferProgressGuard {
    fn drop(&mut self) {
        connection::finish_transfer_progress(&self.transfer_id);
    }
}

fn server_transfer_progress_guard(
    task_id: &str,
    relative_path: &str,
    direction: &str,
    total_bytes: u64,
    protocol_version: &str,
) -> ServerTransferProgressGuard {
    let transfer_id = record_server_transfer_start(
        task_id,
        relative_path,
        direction,
        total_bytes,
        protocol_version,
    );
    ServerTransferProgressGuard { transfer_id }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PendingTaskInvite {
    pub invite_id: String,
    pub task_id: String,
    pub task_name: String,
    pub requester_device_id: String,
    #[serde(default)]
    pub requester_public_key: Vec<u8>,
    #[serde(default)]
    pub requester_address: Option<String>,
    pub requester_path: Option<String>,
    pub proposed_role: String,
    pub status: String,
    pub local_path: Option<String>,
    pub error: Option<String>,
    pub created_unix_ms: i64,
}

impl TaskRootRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, task_id: impl Into<String>, root: impl AsRef<Path>) -> Result<()> {
        let root = root.as_ref();
        if !root.exists() {
            std::fs::create_dir_all(root)?;
        }

        let mut roots = self.roots.lock().unwrap();
        roots.insert(task_id.into(), root.to_path_buf());
        drop(roots);
        self.save_roots()?;
        Ok(())
    }

    pub fn unregister(&self, task_id: &str) -> Result<()> {
        let mut roots = self.roots.lock().unwrap();
        roots.remove(task_id);
        drop(roots);

        let prefix = format!("{}\n", task_id);
        let mut incoming = self.incoming.lock().unwrap();
        incoming.retain(|key, _| !key.starts_with(&prefix));
        drop(incoming);

        self.save_roots()?;
        Ok(())
    }

    pub fn retain_registered_roots(&self, task_ids: &HashSet<String>) -> Result<()> {
        let mut roots = self.roots.lock().unwrap();
        roots.retain(|task_id, _| task_ids.contains(task_id));
        drop(roots);
        self.save_roots()?;
        Ok(())
    }

    fn root_for(&self, task_id: &str) -> Option<PathBuf> {
        let roots = self.roots.lock().unwrap();
        roots.get(task_id).cloned()
    }

    pub fn cancel_incoming_transfer(&self, task_id: &str, relative_path: &str) -> Result<()> {
        let key = transfer_key(task_id, relative_path);
        let transfer = {
            let mut incoming = self.incoming.lock().unwrap();
            incoming.remove(&key)
        };
        if let Some(transfer) = transfer {
            let transfer_id = transfer.transfer_id.clone();
            drop(transfer.file);
            let _ = std::fs::remove_file(&transfer.partial_path);
            connection::finish_transfer_progress(&transfer_id);
        }
        Ok(())
    }

    pub fn register_trusted_peer(&self, identity: PublicIdentity) {
        let mut peers = self.trusted_peers.lock().unwrap();
        peers.insert(identity.device_id.clone(), identity);
    }

    fn trusted_peer(&self, device_id: &str) -> Option<PublicIdentity> {
        let peers = self.trusted_peers.lock().unwrap();
        peers.get(device_id).cloned()
    }

    fn ensure_trusted_peer_key_matches(&self, device_id: &str, public_key: &[u8]) -> Result<()> {
        if public_key.is_empty() {
            return Ok(());
        }
        if let Some(existing) = self.trusted_peer(device_id) {
            if existing.public_key != public_key {
                anyhow::bail!(
                    "trusted peer public key changed for device {}; reject and re-pair before accepting this invite",
                    device_id
                );
            }
        }
        Ok(())
    }

    pub fn set_local_identity(&self, identity: PublicIdentity) {
        let mut local_identity = self.local_identity.lock().unwrap();
        *local_identity = Some(identity);
    }

    fn local_identity(&self) -> Option<PublicIdentity> {
        let local_identity = self.local_identity.lock().unwrap();
        local_identity.clone()
    }

    pub fn set_task_invite_inbox_root(&self, root: impl AsRef<Path>) -> Result<()> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let mut task_invite_inbox_root = self.task_invite_inbox_root.lock().unwrap();
        *task_invite_inbox_root = Some(root);
        Ok(())
    }

    pub fn set_auto_accept_task_invites(&self, auto_accept: bool) {
        let mut current = self.auto_accept_task_invites.lock().unwrap();
        *current = auto_accept;
    }

    fn auto_accept_task_invites(&self) -> bool {
        *self.auto_accept_task_invites.lock().unwrap()
    }

    pub fn list_task_invites(&self) -> Vec<PendingTaskInvite> {
        let invites = self.task_invites.lock().unwrap();
        invites.values().cloned().collect()
    }

    pub fn accept_task_invite(
        &self,
        invite_id: &str,
        local_path: impl AsRef<Path>,
    ) -> Result<PendingTaskInvite> {
        let local_path = local_path.as_ref().to_path_buf();
        let invite_snapshot = {
            let invites = self.task_invites.lock().unwrap();
            invites
                .get(invite_id)
                .ok_or_else(|| anyhow::anyhow!("task invite not found"))?
                .clone()
        };
        validate_invite_local_path(&local_path, &invite_snapshot.proposed_role)?;
        self.ensure_trusted_peer_key_matches(
            &invite_snapshot.requester_device_id,
            &invite_snapshot.requester_public_key,
        )?;
        self.register(&invite_snapshot.task_id, &local_path)?;

        let mut invites = self.task_invites.lock().unwrap();
        let invite = invites
            .get_mut(invite_id)
            .ok_or_else(|| anyhow::anyhow!("task invite not found"))?;
        if !invite.requester_public_key.is_empty() {
            self.register_trusted_peer(PublicIdentity {
                device_id: invite.requester_device_id.clone(),
                public_key: invite.requester_public_key.clone(),
            });
        }
        invite.status = "Accepted".to_string();
        invite.local_path = Some(local_path.to_string_lossy().to_string());
        invite.error = None;
        let invite = invite.clone();
        drop(invites);
        self.save_task_invites()?;
        Ok(invite)
    }

    pub fn reject_task_invite(&self, invite_id: &str, reason: &str) -> Result<PendingTaskInvite> {
        let mut invites = self.task_invites.lock().unwrap();
        let invite = invites
            .get_mut(invite_id)
            .ok_or_else(|| anyhow::anyhow!("task invite not found"))?;
        invite.status = "Rejected".to_string();
        invite.error = Some(reason.to_string());
        let invite = invite.clone();
        drop(invites);
        self.save_task_invites()?;
        Ok(invite)
    }

    fn record_task_invite(
        &self,
        invite_id: String,
        task_id: String,
        task_name: String,
        requester_device_id: String,
        requester_public_key: Vec<u8>,
        requester_address: Option<String>,
        requester_path: Option<String>,
        proposed_role: String,
    ) -> Result<PendingTaskInvite> {
        self.ensure_trusted_peer_key_matches(&requester_device_id, &requester_public_key)?;
        let invite = PendingTaskInvite {
            invite_id: invite_id.clone(),
            task_id,
            task_name,
            requester_device_id,
            requester_public_key,
            requester_address,
            requester_path,
            proposed_role,
            status: "Pending".to_string(),
            local_path: None,
            error: None,
            created_unix_ms: now_ms(),
        };
        let mut invites = self.task_invites.lock().unwrap();
        invites.insert(invite_id, invite.clone());
        drop(invites);
        self.save_task_invites()?;
        Ok(invite)
    }

    fn task_invite_status(&self, invite_id: &str) -> Option<PendingTaskInvite> {
        let invites = self.task_invites.lock().unwrap();
        invites.get(invite_id).cloned()
    }

    fn invite_root(&self, task_id: &str, task_name: &str) -> Result<PathBuf> {
        let base = {
            let task_invite_inbox_root = self.task_invite_inbox_root.lock().unwrap();
            task_invite_inbox_root.clone()
        }
        .ok_or_else(|| anyhow::anyhow!("task invite inbox is not configured"))?;

        let safe_name = safe_folder_name(task_name);
        let suffix = task_id.chars().take(8).collect::<String>();
        Ok(base.join(format!("{}-{}", safe_name, suffix)))
    }

    pub fn set_persistence_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if path.exists() {
            let bytes = std::fs::read(&path)?;
            let persisted: HashMap<String, String> = serde_json::from_slice(&bytes)?;
            let mut roots = self.roots.lock().unwrap();
            for (task_id, root) in persisted {
                roots.insert(task_id, PathBuf::from(root));
            }
        }

        let mut persistence_path = self.persistence_path.lock().unwrap();
        *persistence_path = Some(path);
        drop(persistence_path);
        self.save_roots()
    }

    pub fn set_state_db_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut state_db_path = self.state_db_path.lock().unwrap();
        *state_db_path = Some(path);
        Ok(())
    }

    fn state_db_path(&self) -> Option<PathBuf> {
        self.state_db_path.lock().unwrap().clone()
    }

    pub fn set_task_invites_persistence_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if path.exists() {
            let bytes = std::fs::read(&path)?;
            let persisted: HashMap<String, PendingTaskInvite> = serde_json::from_slice(&bytes)?;
            let mut invites = self.task_invites.lock().unwrap();
            for (invite_id, invite) in persisted {
                invites.insert(invite_id, invite);
            }
        }

        let mut persistence_path = self.task_invite_persistence_path.lock().unwrap();
        *persistence_path = Some(path);
        drop(persistence_path);
        self.save_task_invites()
    }

    fn save_task_invites(&self) -> Result<()> {
        let path = {
            let persistence_path = self.task_invite_persistence_path.lock().unwrap();
            persistence_path.clone()
        };
        let Some(path) = path else {
            return Ok(());
        };

        let invites = self.task_invites.lock().unwrap();
        let bytes = serde_json::to_vec_pretty(&*invites)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    fn save_roots(&self) -> Result<()> {
        let path = {
            let persistence_path = self.persistence_path.lock().unwrap();
            persistence_path.clone()
        };
        let Some(path) = path else {
            return Ok(());
        };

        let roots = self.roots.lock().unwrap();
        let persisted = roots
            .iter()
            .map(|(task_id, root)| (task_id.clone(), root.to_string_lossy().to_string()))
            .collect::<HashMap<_, _>>();
        let bytes = serde_json::to_vec_pretty(&persisted)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

/// TCP listener for incoming peer connections.
pub struct SyncServer {
    local_addr: SocketAddr,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    task_roots: TaskRootRegistry,
}

impl SyncServer {
    pub fn start_in_background(port: u16) -> Result<Self> {
        let addr = format!("0.0.0.0:{}", port);
        let listener = std::net::TcpListener::bind(&addr)?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?;
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let task_roots = TaskRootRegistry::new();
        let accept_roots = task_roots.clone();

        std::thread::spawn(move || {
            let worker_threads = std::thread::available_parallelism()
                .map(|n| n.get().clamp(2, 4))
                .unwrap_or(2);
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(worker_threads)
                .enable_all()
                .build()
                .expect("failed to create sync server tokio runtime");

            rt.block_on(async move {
                let listener = match TcpListener::from_std(listener) {
                    Ok(listener) => listener,
                    Err(e) => {
                        tracing::error!("failed to create tokio tcp listener: {}", e);
                        return;
                    }
                };

                accept_loop(listener, local_addr, accept_roots, &mut shutdown_rx).await;
            });
        });

        Ok(Self {
            local_addr,
            shutdown_tx: Some(shutdown_tx),
            task_roots,
        })
    }

    /// Start listening on the given port (0 = OS-assigned).
    /// Returns the server with the actual bound address.
    pub async fn start(port: u16) -> Result<Self> {
        let addr = format!("0.0.0.0:{}", port);
        let listener = TcpListener::bind(&addr).await?;
        let local_addr = listener.local_addr()?;

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let task_roots = TaskRootRegistry::new();
        let accept_roots = task_roots.clone();

        tokio::spawn(async move {
            accept_loop(listener, local_addr, accept_roots, &mut shutdown_rx).await;
        });

        Ok(Self {
            local_addr,
            shutdown_tx: Some(shutdown_tx),
            task_roots,
        })
    }

    /// Get the port the server is listening on.
    pub fn port(&self) -> u16 {
        self.local_addr.port()
    }

    pub fn register_task_root(
        &self,
        task_id: impl Into<String>,
        root: impl AsRef<Path>,
    ) -> Result<()> {
        self.task_roots.register(task_id, root)
    }

    pub fn unregister_task_root(&self, task_id: &str) -> Result<()> {
        self.task_roots.unregister(task_id)
    }

    pub fn retain_registered_task_roots(&self, task_ids: &HashSet<String>) -> Result<()> {
        self.task_roots.retain_registered_roots(task_ids)
    }

    pub fn register_trusted_peer(&self, identity: PublicIdentity) {
        self.task_roots.register_trusted_peer(identity);
    }

    pub fn set_task_roots_persistence_path(&self, path: impl AsRef<Path>) -> Result<()> {
        self.task_roots.set_persistence_path(path)
    }

    pub fn set_state_db_path(&self, path: impl AsRef<Path>) -> Result<()> {
        self.task_roots.set_state_db_path(path)
    }

    pub fn set_task_invites_persistence_path(&self, path: impl AsRef<Path>) -> Result<()> {
        self.task_roots.set_task_invites_persistence_path(path)
    }

    pub fn set_local_identity(&self, identity: PublicIdentity) {
        self.task_roots.set_local_identity(identity);
    }

    pub fn set_task_invite_inbox_root(&self, root: impl AsRef<Path>) -> Result<()> {
        self.task_roots.set_task_invite_inbox_root(root)
    }

    pub fn set_auto_accept_task_invites(&self, auto_accept: bool) {
        self.task_roots.set_auto_accept_task_invites(auto_accept);
    }

    pub fn list_task_invites(&self) -> Vec<PendingTaskInvite> {
        self.task_roots.list_task_invites()
    }

    pub fn accept_task_invite(
        &self,
        invite_id: &str,
        local_path: impl AsRef<Path>,
    ) -> Result<PendingTaskInvite> {
        self.task_roots.accept_task_invite(invite_id, local_path)
    }

    pub fn reject_task_invite(&self, invite_id: &str, reason: &str) -> Result<PendingTaskInvite> {
        self.task_roots.reject_task_invite(invite_id, reason)
    }

    pub fn cancel_incoming_transfer(&self, task_id: &str, relative_path: &str) -> Result<()> {
        self.task_roots
            .cancel_incoming_transfer(task_id, relative_path)
    }

    #[allow(dead_code)]
    pub fn record_pending_task_invite_for_test(
        &self,
        invite_id: &str,
        task_id: &str,
        task_name: &str,
        requester_device_id: &str,
        requester_path: Option<String>,
        proposed_role: &str,
    ) -> Result<PendingTaskInvite> {
        self.task_roots.record_task_invite(
            invite_id.to_string(),
            task_id.to_string(),
            task_name.to_string(),
            requester_device_id.to_string(),
            Vec::new(),
            None,
            requester_path,
            proposed_role.to_string(),
        )
    }
}

impl Drop for SyncServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

async fn accept_loop(
    listener: TcpListener,
    local_addr: SocketAddr,
    task_roots: TaskRootRegistry,
    shutdown_rx: &mut tokio::sync::oneshot::Receiver<()>,
) {
    tracing::info!("sync server listening on {}", local_addr);
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, peer_addr)) => {
                        tracing::info!("incoming connection from {}", peer_addr);
                        let task_roots = task_roots.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, peer_addr, task_roots).await {
                                tracing::warn!("connection from {} error: {}", peer_addr, e);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("accept error: {}", e);
                    }
                }
            }
            _ = &mut *shutdown_rx => {
                tracing::info!("sync server shutting down");
                break;
            }
        }
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    task_roots: TaskRootRegistry,
) -> Result<()> {
    use crate::transport::protocol;
    let (mut reader, mut writer) = stream.into_split();
    let mut len_buf = [0u8; 4];
    let mut authenticated_device_id: Option<String> = None;
    let mut pending_auth: Option<(String, Vec<u8>)> = None;

    loop {
        // Read message length
        match reader.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(_) => {
                tracing::info!("peer {} disconnected", peer_addr);
                break;
            }
        }

        let msg_len = u32::from_be_bytes(len_buf) as usize;
        if msg_len > 10 * 1024 * 1024 {
            return Err(anyhow::anyhow!("message too large: {} bytes", msg_len));
        }

        let mut msg_buf = vec![0u8; msg_len];
        reader.read_exact(&mut msg_buf).await?;

        match protocol::decode_message(&msg_buf) {
            Ok(msg) => {
                tracing::debug!("received from {}: {:?}", peer_addr, msg);
                // Handle message types as needed
                match msg {
                    protocol::SyncMessage::IdentityRequest => {
                        let response = match task_roots.local_identity() {
                            Some(identity) => protocol::SyncMessage::IdentityResponse {
                                device_id: identity.device_id,
                                public_key: identity.public_key,
                            },
                            None => protocol::SyncMessage::AuthReject {
                                reason: "local identity is not available".to_string(),
                            },
                        };
                        writer
                            .write_all(&protocol::encode_message(&response)?)
                            .await?;
                    }
                    protocol::SyncMessage::AuthHello { device_id } => {
                        if task_roots.trusted_peer(&device_id).is_none() {
                            let reject = protocol::SyncMessage::AuthReject {
                                reason: "peer is not trusted".to_string(),
                            };
                            writer
                                .write_all(&protocol::encode_message(&reject)?)
                                .await?;
                            continue;
                        }

                        let mut nonce = vec![0u8; 32];
                        rand::thread_rng().fill_bytes(&mut nonce);
                        pending_auth = Some((device_id, nonce.clone()));
                        let challenge = protocol::SyncMessage::AuthChallenge { nonce };
                        writer
                            .write_all(&protocol::encode_message(&challenge)?)
                            .await?;
                    }
                    protocol::SyncMessage::AuthProof {
                        device_id,
                        signature,
                    } => {
                        let auth_result = verify_auth_proof(
                            &task_roots,
                            pending_auth.as_ref(),
                            &device_id,
                            &signature,
                        );
                        let response = match auth_result {
                            Ok(()) => {
                                authenticated_device_id = Some(device_id.clone());
                                pending_auth = None;
                                protocol::SyncMessage::AuthOk { device_id }
                            }
                            Err(e) => protocol::SyncMessage::AuthReject {
                                reason: e.to_string(),
                            },
                        };
                        writer
                            .write_all(&protocol::encode_message(&response)?)
                            .await?;
                    }
                    protocol::SyncMessage::TransferHello {
                        supported_versions, ..
                    } => {
                        let selected = if supported_versions.contains(&2) {
                            2
                        } else {
                            1
                        };
                        let response = protocol::SyncMessage::TransferReady {
                            selected_version: selected,
                            max_chunk_size: 4 * 1024 * 1024,
                            ack_interval_bytes: if selected == 2 {
                                TRANSFER_V2_ACK_INTERVAL_BYTES
                            } else {
                                TRANSFER_V1_ACK_INTERVAL_BYTES
                            },
                        };
                        writer
                            .write_all(&protocol::encode_message(&response)?)
                            .await?;
                    }
                    protocol::SyncMessage::Ping => {
                        let pong = protocol::encode_message(&protocol::SyncMessage::Pong)?;
                        writer.write_all(&pong).await?;
                    }
                    protocol::SyncMessage::TransferCancel {
                        task_id,
                        relative_path,
                        direction,
                    } => {
                        connection::cancel_active_transfer(
                            &task_id,
                            &relative_path,
                            direction.as_deref(),
                        );
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                if direction.as_deref() == Some("serve") {
                                    Ok(())
                                } else {
                                    task_roots.cancel_incoming_transfer(&task_id, &relative_path)
                                }
                            }) {
                                Ok(()) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: true,
                                    error: None,
                                },
                                Err(e) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: false,
                                    error: Some(e.to_string()),
                                },
                            };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::FileTransfer {
                        task_id,
                        relative_path,
                        file_hash,
                        total_bytes,
                        data,
                    } => {
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                write_incoming_file(
                                    &task_roots,
                                    &task_id,
                                    &relative_path,
                                    &file_hash,
                                    total_bytes,
                                    &data,
                                )
                            }) {
                                Ok(()) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: true,
                                    error: None,
                                },
                                Err(e) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: false,
                                    error: Some(e.to_string()),
                                },
                            };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::FileChunkStart {
                        task_id,
                        relative_path,
                        file_hash,
                        total_bytes,
                    } => {
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                start_incoming_chunked_file(
                                    &task_roots,
                                    &task_id,
                                    &relative_path,
                                    &file_hash,
                                    total_bytes,
                                )
                            }) {
                                Ok(()) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: true,
                                    error: None,
                                },
                                Err(e) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: false,
                                    error: Some(e.to_string()),
                                },
                            };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::FileChunk {
                        task_id,
                        relative_path,
                        offset,
                        data,
                    } => {
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                append_incoming_chunk(
                                    &task_roots,
                                    &task_id,
                                    &relative_path,
                                    offset,
                                    &data,
                                )
                            }) {
                                Ok(needs_ack) => match needs_ack {
                                    IncomingChunkAck::Checkpoint(received) => {
                                        Some(protocol::SyncMessage::FileChunkAck {
                                            task_id,
                                            relative_path,
                                            received_bytes: received,
                                            success: true,
                                            error: None,
                                        })
                                    }
                                    IncomingChunkAck::LegacyFileAck => {
                                        Some(protocol::SyncMessage::FileAck {
                                            task_id,
                                            relative_path,
                                            success: true,
                                            error: None,
                                        })
                                    }
                                    IncomingChunkAck::None => None,
                                },
                                Err(e) => Some(protocol::SyncMessage::FileChunkAck {
                                    task_id,
                                    relative_path,
                                    received_bytes: 0,
                                    success: false,
                                    error: Some(e.to_string()),
                                }),
                            };
                        if let Some(ack) = ack {
                            writer.write_all(&protocol::encode_message(&ack)?).await?;
                        }
                    }
                    protocol::SyncMessage::FileChunkEnd {
                        task_id,
                        relative_path,
                        file_hash,
                    } => {
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                finish_incoming_chunked_file(
                                    &task_roots,
                                    &task_id,
                                    &relative_path,
                                    file_hash.as_deref(),
                                )
                            }) {
                                Ok(()) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: true,
                                    error: None,
                                },
                                Err(e) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: false,
                                    error: Some(e.to_string()),
                                },
                            };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::FileStreamStartV2 {
                        task_id,
                        relative_path,
                        total_bytes,
                    } => {
                        if let Err(e) =
                            require_authenticated(&authenticated_device_id).and_then(|_| {
                                start_incoming_v2(
                                    &task_roots,
                                    &task_id,
                                    &relative_path,
                                    total_bytes,
                                )
                            })
                        {
                            let ack = protocol::SyncMessage::FileAck {
                                task_id,
                                relative_path,
                                success: false,
                                error: Some(e.to_string()),
                            };
                            writer.write_all(&protocol::encode_message(&ack)?).await?;
                        }
                    }
                    protocol::SyncMessage::FileChunkBinaryV2 {
                        task_id,
                        relative_path,
                        offset,
                        bytes,
                        ack: wants_ack,
                    } => {
                        let payload_start = Instant::now();
                        let payload =
                            match protocol::read_v2_payload(&mut reader, bytes as usize).await {
                                Ok(p) => {
                                    let payload_ms = elapsed_ms(payload_start);
                                    update_incoming_timing(
                                        &task_roots,
                                        &task_id,
                                        &relative_path,
                                        |timing| {
                                            timing.payload_read_ms += payload_ms;
                                        },
                                    );
                                    p
                                }
                                Err(e) => {
                                    let ack = protocol::SyncMessage::FileStreamAckV2 {
                                        task_id,
                                        relative_path,
                                        received_bytes: 0,
                                        success: false,
                                        error: Some(e.to_string()),
                                    };
                                    write_v2_stream_ack(&mut writer, &task_roots, ack).await?;
                                    continue;
                                }
                            };
                        let ack_result = match require_authenticated(&authenticated_device_id) {
                            Ok(()) => {
                                let task_roots_for_write = task_roots.clone();
                                let task_id_for_write = task_id.clone();
                                let relative_path_for_write = relative_path.clone();
                                tokio::task::spawn_blocking(move || {
                                    append_incoming_chunk(
                                        &task_roots_for_write,
                                        &task_id_for_write,
                                        &relative_path_for_write,
                                        offset,
                                        &payload,
                                    )
                                })
                                .await
                                .map_err(|e| anyhow::anyhow!("v2 receive worker failed: {}", e))?
                            }
                            Err(e) => Err(e),
                        };
                        match ack_result {
                            Ok(received) if wants_ack => {
                                let received_bytes = match received {
                                    IncomingChunkAck::Checkpoint(received) => received,
                                    IncomingChunkAck::LegacyFileAck | IncomingChunkAck::None => {
                                        offset + bytes as u64
                                    }
                                };
                                let ack = protocol::SyncMessage::FileStreamAckV2 {
                                    task_id,
                                    relative_path,
                                    received_bytes,
                                    success: true,
                                    error: None,
                                };
                                write_v2_stream_ack(&mut writer, &task_roots, ack).await?;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                let ack = protocol::SyncMessage::FileStreamAckV2 {
                                    task_id,
                                    relative_path,
                                    received_bytes: 0,
                                    success: false,
                                    error: Some(e.to_string()),
                                };
                                write_v2_stream_ack(&mut writer, &task_roots, ack).await?;
                            }
                        }
                    }
                    protocol::SyncMessage::FileStreamEndV2 {
                        task_id,
                        relative_path,
                        file_hash,
                    } => {
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                finish_incoming_v2(
                                    &task_roots,
                                    &task_id,
                                    &relative_path,
                                    &file_hash,
                                )
                            }) {
                                Ok(()) => protocol::SyncMessage::FileStreamAckV2 {
                                    task_id: task_id.clone(),
                                    relative_path: relative_path.clone(),
                                    received_bytes: 0,
                                    success: true,
                                    error: None,
                                },
                                Err(e) => protocol::SyncMessage::FileStreamAckV2 {
                                    task_id: task_id.clone(),
                                    relative_path: relative_path.clone(),
                                    received_bytes: 0,
                                    success: false,
                                    error: Some(e.to_string()),
                                },
                            };
                        write_v2_stream_ack(&mut writer, &task_roots, ack).await?;
                    }
                    protocol::SyncMessage::FileDownloadRequestV2 {
                        task_id,
                        relative_path,
                    } => match require_authenticated(&authenticated_device_id).map(|_| ()) {
                        Ok(()) => {
                            if let Err(e) = send_file_download_v2(
                                &task_roots,
                                &task_id,
                                &relative_path,
                                &mut reader,
                                &mut writer,
                            )
                            .await
                            {
                                let ack = protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: false,
                                    error: Some(e.to_string()),
                                };
                                writer.write_all(&protocol::encode_message(&ack)?).await?;
                            } else {
                                match tokio::time::timeout(
                                    Duration::from_secs(10),
                                    read_server_message(&mut reader),
                                )
                                .await
                                {
                                    Ok(Ok(protocol::SyncMessage::FileStreamAckV2 {
                                        success: true,
                                        ..
                                    })) => {}
                                    Ok(Ok(protocol::SyncMessage::FileStreamAckV2 {
                                        success: false,
                                        error,
                                        ..
                                    })) => tracing::warn!(
                                        "v2 download final ack error: {}",
                                        error.unwrap_or_else(|| "unknown".to_string())
                                    ),
                                    Ok(Ok(other)) => {
                                        tracing::warn!(
                                            "unexpected v2 download final ack: {:?}",
                                            other
                                        )
                                    }
                                    Ok(Err(e)) => {
                                        tracing::warn!("v2 download final ack read failed: {}", e)
                                    }
                                    Err(_) => tracing::warn!("v2 download final ack timed out"),
                                }
                            }
                        }
                        Err(e) => {
                            let ack = protocol::SyncMessage::FileAck {
                                task_id,
                                relative_path,
                                success: false,
                                error: Some(e.to_string()),
                            };
                            writer.write_all(&protocol::encode_message(&ack)?).await?;
                        }
                    },
                    protocol::SyncMessage::FileDownloadRequest {
                        task_id,
                        relative_path,
                    } => match require_authenticated(&authenticated_device_id).map(|_| ()) {
                        Ok(()) => {
                            if let Err(e) = send_file_download(
                                &task_roots,
                                &task_id,
                                &relative_path,
                                &mut writer,
                            )
                            .await
                            {
                                let ack = protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: false,
                                    error: Some(e.to_string()),
                                };
                                writer.write_all(&protocol::encode_message(&ack)?).await?;
                            }
                        }
                        Err(e) => {
                            let ack = protocol::SyncMessage::FileAck {
                                task_id,
                                relative_path,
                                success: false,
                                error: Some(e.to_string()),
                            };
                            writer.write_all(&protocol::encode_message(&ack)?).await?;
                        }
                    },
                    protocol::SyncMessage::DirectoryCreate {
                        task_id,
                        relative_path,
                    } => {
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                create_incoming_directory(&task_roots, &task_id, &relative_path)
                            }) {
                                Ok(()) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: true,
                                    error: None,
                                },
                                Err(e) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: false,
                                    error: Some(e.to_string()),
                                },
                            };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::FileDelete {
                        task_id,
                        relative_path,
                        expected_kind,
                        expected_hash,
                        expected_hash_status,
                        expected_size,
                        expected_modified_unix_ms,
                        delete_batch_id,
                    } => {
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                move_incoming_delete_to_history(
                                    &task_roots,
                                    &task_id,
                                    &relative_path,
                                    expected_kind,
                                    expected_hash.as_deref(),
                                    expected_hash_status,
                                    expected_size,
                                    expected_modified_unix_ms,
                                    delete_batch_id.as_deref(),
                                )
                            }) {
                                Ok(()) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: true,
                                    error: None,
                                },
                                Err(e) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: false,
                                    error: Some(e.to_string()),
                                },
                            };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::ConflictApply {
                        task_id,
                        relative_path,
                        staged_relative_path,
                        mode,
                    } => {
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                apply_incoming_conflict_file(
                                    &task_roots,
                                    &task_id,
                                    &relative_path,
                                    &staged_relative_path,
                                    &mode,
                                )
                            }) {
                                Ok(applied_path) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path: applied_path,
                                    success: true,
                                    error: None,
                                },
                                Err(e) => protocol::SyncMessage::FileAck {
                                    task_id,
                                    relative_path,
                                    success: false,
                                    error: Some(e.to_string()),
                                },
                            };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::TaskRegister { task_id, root_path } => {
                        let ack = match require_authenticated(&authenticated_device_id)
                            .and_then(|_| task_roots.register(&task_id, &root_path))
                        {
                            Ok(()) => protocol::SyncMessage::TaskAck {
                                task_id,
                                success: true,
                                error: None,
                            },
                            Err(e) => protocol::SyncMessage::TaskAck {
                                task_id,
                                success: false,
                                error: Some(e.to_string()),
                            },
                        };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::TaskInviteProposal {
                        invite_id,
                        task_id,
                        task_name,
                        requester_device_id,
                        requester_public_key,
                        requester_port,
                        requester_path,
                        proposed_role,
                    } => {
                        let response =
                            if requester_device_id.is_empty() || requester_public_key.is_empty() {
                                protocol::SyncMessage::TaskInviteAck {
                                    task_id,
                                    success: false,
                                    remote_path: None,
                                    error: Some(
                                        "task invite requester identity is missing".to_string(),
                                    ),
                                }
                            } else {
                                match task_roots.record_task_invite(
                                    invite_id,
                                    task_id.clone(),
                                    task_name,
                                    requester_device_id,
                                    requester_public_key,
                                    requester_address(peer_addr, requester_port),
                                    requester_path,
                                    proposed_role,
                                ) {
                                    Ok(invite) => protocol::SyncMessage::TaskInvitePending {
                                        invite_id: invite.invite_id,
                                        task_id: invite.task_id,
                                    },
                                    Err(e) => protocol::SyncMessage::TaskInviteAck {
                                        task_id,
                                        success: false,
                                        remote_path: None,
                                        error: Some(e.to_string()),
                                    },
                                }
                            };
                        writer
                            .write_all(&protocol::encode_message(&response)?)
                            .await?;
                    }
                    protocol::SyncMessage::TaskInvite {
                        invite_id,
                        task_id,
                        task_name,
                        requester_port,
                        requester_path,
                        proposed_role,
                    } => {
                        let error_task_id = task_id.clone();
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                let requester_device_id = authenticated_device_id
                                    .clone()
                                    .unwrap_or_else(|| "unknown".to_string());
                                if !task_roots.auto_accept_task_invites() {
                                    let invite = task_roots.record_task_invite(
                                        invite_id.clone(),
                                        task_id.clone(),
                                        task_name.clone(),
                                        requester_device_id,
                                        Vec::new(),
                                        requester_address(peer_addr, requester_port),
                                        requester_path.clone(),
                                        proposed_role.clone(),
                                    )?;
                                    return Ok(protocol::SyncMessage::TaskInvitePending {
                                        invite_id: invite.invite_id,
                                        task_id: invite.task_id,
                                    });
                                }
                                let root = task_roots.invite_root(&task_id, &task_name)?;
                                task_roots.register(&task_id, &root)?;
                                Ok(protocol::SyncMessage::TaskInviteAck {
                                    task_id,
                                    success: true,
                                    remote_path: Some(root.to_string_lossy().to_string()),
                                    error: None,
                                })
                            }) {
                                Ok(msg) => msg,
                                Err(e) => protocol::SyncMessage::TaskInviteAck {
                                    task_id: error_task_id,
                                    success: false,
                                    remote_path: None,
                                    error: Some(e.to_string()),
                                },
                            };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::TaskInviteStatusRequest { invite_id } => {
                        let response = match task_roots
                            .task_invite_status(&invite_id)
                            .ok_or_else(|| anyhow::anyhow!("task invite not found"))
                        {
                            Ok(invite) => protocol::SyncMessage::TaskInviteStatus {
                                invite_id: invite.invite_id,
                                task_id: invite.task_id,
                                status: invite.status,
                                remote_path: invite.local_path,
                                error: invite.error,
                            },
                            Err(e) => protocol::SyncMessage::TaskInviteStatus {
                                invite_id,
                                task_id: String::new(),
                                status: "Missing".to_string(),
                                remote_path: None,
                                error: Some(e.to_string()),
                            },
                        };
                        writer
                            .write_all(&protocol::encode_message(&response)?)
                            .await?;
                    }
                    protocol::SyncMessage::IdentityResponse { .. }
                    | protocol::SyncMessage::PairRequest { .. }
                    | protocol::SyncMessage::PairConfirm { .. }
                    | protocol::SyncMessage::PairReject { .. }
                    | protocol::SyncMessage::AuthChallenge { .. }
                    | protocol::SyncMessage::AuthOk { .. }
                    | protocol::SyncMessage::AuthReject { .. }
                    | protocol::SyncMessage::TaskAck { .. }
                    | protocol::SyncMessage::TaskInviteAck { .. }
                    | protocol::SyncMessage::TaskInvitePending { .. }
                    | protocol::SyncMessage::TaskInviteStatus { .. }
                    | protocol::SyncMessage::FileAck { .. }
                    | protocol::SyncMessage::FileChunkAck { .. }
                    | protocol::SyncMessage::FileStreamAckV2 { .. }
                    | protocol::SyncMessage::ScanResponse { .. }
                    | protocol::SyncMessage::Pong
                    | protocol::SyncMessage::TransferReady { .. } => {
                        tracing::info!("received control message from {}", peer_addr);
                    }
                    protocol::SyncMessage::ScanRequest { task_id } => {
                        let response = match require_authenticated(&authenticated_device_id)
                            .and_then(|_| scan_task_root(&task_roots, &task_id))
                        {
                            Ok(files) => protocol::SyncMessage::ScanResponse {
                                task_id,
                                files,
                                error: None,
                            },
                            Err(e) => protocol::SyncMessage::ScanResponse {
                                task_id,
                                files: Vec::new(),
                                error: Some(e.to_string()),
                            },
                        };
                        writer
                            .write_all(&protocol::encode_message(&response)?)
                            .await?;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to decode message from {}: {}", peer_addr, e);
            }
        }
    }

    Ok(())
}

fn require_authenticated(authenticated_device_id: &Option<String>) -> Result<()> {
    if authenticated_device_id.is_some() {
        Ok(())
    } else {
        anyhow::bail!("peer is not authenticated")
    }
}

fn verify_auth_proof(
    task_roots: &TaskRootRegistry,
    pending_auth: Option<&(String, Vec<u8>)>,
    device_id: &str,
    signature: &[u8],
) -> Result<()> {
    let Some((pending_device_id, nonce)) = pending_auth else {
        anyhow::bail!("missing auth challenge");
    };
    if pending_device_id != device_id {
        anyhow::bail!("auth device mismatch");
    }
    let peer = task_roots
        .trusted_peer(device_id)
        .ok_or_else(|| anyhow::anyhow!("peer is not trusted"))?;
    let signature = Signature::from_slice(signature)?;
    DeviceIdentity::verify(
        &peer.public_key,
        &auth_payload(device_id, nonce),
        &signature,
    )
}

fn write_incoming_file(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    file_hash: &str,
    total_bytes: u64,
    data: &[u8],
) -> Result<()> {
    if data.len() as u64 != total_bytes {
        anyhow::bail!("file size mismatch");
    }
    ensure_transfer_not_deferred(task_id, relative_path, "receive")?;
    let actual_hash = blake3::hash(data).to_hex().to_string();
    if actual_hash != file_hash {
        anyhow::bail!("file hash mismatch");
    }

    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let dest = safe_join(&root, relative_path)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let partial = partial_path(&dest);
    std::fs::write(&partial, data)?;
    std::fs::rename(&partial, &dest)?;
    record_received_file(task_roots, task_id, relative_path, &dest, file_hash)?;
    Ok(())
}

fn transfer_key(task_id: &str, relative_path: &str) -> String {
    format!("{}\n{}", task_id, relative_path)
}

fn ensure_transfer_not_deferred(task_id: &str, relative_path: &str, direction: &str) -> Result<()> {
    if connection::is_transfer_deferred(task_id, relative_path, direction) {
        anyhow::bail!("transfer deferred by user");
    }
    Ok(())
}

fn start_incoming_chunked_file(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    file_hash: &str,
    total_bytes: u64,
) -> Result<()> {
    ensure_transfer_not_deferred(task_id, relative_path, "receive")?;
    connection::clear_transfer_cancel(task_id, relative_path, Some("receive"));
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let final_path = safe_join(&root, relative_path)?;
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial_path = partial_path(&final_path);
    let file = std::fs::File::create(&partial_path)?;

    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "receive",
        bytes_total = total_bytes,
        "incoming chunked file start"
    );
    let transfer_id =
        record_server_transfer_start(task_id, relative_path, "receive", total_bytes, "v1_json");

    let transfer = IncomingTransfer {
        transfer_id,
        partial_path,
        final_path,
        file_hash: file_hash.to_string(),
        total_bytes,
        written_bytes: 0,
        hasher: blake3::Hasher::new(),
        start_time: Instant::now(),
        first_byte_time: None,
        next_progress_at: TRANSFER_PROGRESS_INTERVAL_BYTES,
        next_ack_at: TRANSFER_V1_ACK_INTERVAL_BYTES,
        ack_every_chunk: !file_hash.is_empty(),
        protocol_version: "v1_json",
        file,
        timing: V2ReceiveTiming::default(),
    };
    let mut incoming = task_roots.incoming.lock().unwrap();
    incoming.insert(transfer_key(task_id, relative_path), transfer);
    Ok(())
}

/// Append a chunk to an incoming transfer.
/// Returns `Some(written_bytes)` when the receiver should send a checkpoint ACK,
/// `None` when the sender should keep streaming without waiting.
fn append_incoming_chunk(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    offset: u64,
    data: &[u8],
) -> Result<IncomingChunkAck> {
    if connection::is_transfer_cancelled(task_id, relative_path, "receive") {
        let _ = task_roots.cancel_incoming_transfer(task_id, relative_path);
        anyhow::bail!("transfer cancelled");
    }

    let key = transfer_key(task_id, relative_path);
    let mut incoming = task_roots.incoming.lock().unwrap();
    let mismatch_partial_path = {
        let transfer = incoming
            .get_mut(&key)
            .ok_or_else(|| anyhow::anyhow!("chunked transfer not started"))?;
        if transfer.written_bytes != offset {
            Some(transfer.partial_path.clone())
        } else {
            None
        }
    };
    if let Some(partial_path) = mismatch_partial_path {
        incoming.remove(&key);
        drop(incoming);
        let _ = std::fs::remove_file(partial_path);
        anyhow::bail!("unexpected chunk offset");
    }
    let transfer = incoming
        .get_mut(&key)
        .ok_or_else(|| anyhow::anyhow!("chunked transfer not started"))?;

    if transfer.first_byte_time.is_none() {
        transfer.first_byte_time = Some(Instant::now());
    }

    use std::io::Write;
    let write_start = Instant::now();
    transfer.file.write_all(data)?;
    transfer.timing.file_write_ms += elapsed_ms(write_start);
    let hash_start = Instant::now();
    transfer.hasher.update(data);
    transfer.timing.hash_ms += elapsed_ms(hash_start);
    transfer.timing.chunk_count += 1;
    transfer.written_bytes += data.len() as u64;
    if transfer.written_bytes > transfer.total_bytes {
        anyhow::bail!("chunked transfer exceeded expected size");
    }

    if transfer.ack_every_chunk {
        return Ok(IncomingChunkAck::LegacyFileAck);
    }

    let should_ack = transfer.written_bytes >= transfer.next_ack_at
        || transfer.written_bytes >= transfer.total_bytes;
    if should_ack {
        transfer.next_ack_at += if transfer.protocol_version == "v2_binary" {
            TRANSFER_V2_ACK_INTERVAL_BYTES
        } else {
            TRANSFER_V1_ACK_INTERVAL_BYTES
        };
    }

    if transfer.written_bytes >= transfer.next_progress_at {
        let started_at = transfer.first_byte_time.unwrap_or(transfer.start_time);
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        let mbps = if elapsed_ms > 0 {
            (transfer.written_bytes as f64 / (1024.0 * 1024.0)) / (elapsed_ms as f64 / 1000.0)
        } else {
            0.0
        };
        tracing::info!(
            task_id = %task_id,
            relative_path = %relative_path,
            direction = "receive",
            bytes_total = transfer.total_bytes,
            bytes_done = transfer.written_bytes,
            elapsed_ms = elapsed_ms,
            mbps = format_args!("{:.1}", mbps),
            protocol_version = transfer.protocol_version,
            ack_interval_bytes = if transfer.protocol_version == "v2_binary" {
                TRANSFER_V2_ACK_INTERVAL_BYTES
            } else {
                TRANSFER_V1_ACK_INTERVAL_BYTES
            },
            "transfer progress"
        );
        connection::record_throttled(
            &transfer.transfer_id,
            task_id,
            relative_path,
            "receive",
            transfer.written_bytes,
            transfer.total_bytes,
            transfer.written_bytes,
            mbps,
            transfer.protocol_version,
        );
        transfer.next_progress_at += TRANSFER_PROGRESS_INTERVAL_BYTES;
    }

    if should_ack {
        Ok(IncomingChunkAck::Checkpoint(transfer.written_bytes))
    } else {
        Ok(IncomingChunkAck::None)
    }
}

fn finish_incoming_chunked_file(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    end_hash: Option<&str>,
) -> Result<()> {
    use std::io::Write;
    let key = transfer_key(task_id, relative_path);
    let mut transfer = {
        let mut incoming = task_roots.incoming.lock().unwrap();
        incoming
            .remove(&key)
            .ok_or_else(|| anyhow::anyhow!("chunked transfer not started"))?
    };
    if transfer.written_bytes != transfer.total_bytes {
        connection::finish_transfer_progress(&transfer.transfer_id);
        anyhow::bail!("chunked transfer size mismatch");
    }
    let expected_hash = end_hash
        .filter(|h| !h.is_empty())
        .unwrap_or(&transfer.file_hash);
    let actual_hash = transfer.hasher.finalize().to_hex().to_string();
    if actual_hash != expected_hash {
        let _ = std::fs::remove_file(&transfer.partial_path);
        connection::finish_transfer_progress(&transfer.transfer_id);
        anyhow::bail!("file hash mismatch");
    }
    transfer.file.flush()?;
    drop(transfer.file);
    std::fs::rename(&transfer.partial_path, &transfer.final_path)?;

    let total_elapsed_ms = transfer.start_time.elapsed().as_millis() as u64;
    let total_mbps = if total_elapsed_ms > 0 {
        (transfer.total_bytes as f64 / (1024.0 * 1024.0)) / (total_elapsed_ms as f64 / 1000.0)
    } else {
        0.0
    };
    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "receive",
        bytes_total = transfer.total_bytes,
        bytes_done = transfer.written_bytes,
        elapsed_ms = total_elapsed_ms,
        mbps = format_args!("{:.1}", total_mbps),
        protocol_version = transfer.protocol_version,
        "transfer complete"
    );

    let effective_hash = expected_hash.to_string();
    record_received_file(
        task_roots,
        task_id,
        relative_path,
        &transfer.final_path,
        &effective_hash,
    )?;
    connection::finish_transfer_progress(&transfer.transfer_id);
    Ok(())
}

fn start_incoming_v2(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    total_bytes: u64,
) -> Result<()> {
    ensure_transfer_not_deferred(task_id, relative_path, "receive")?;
    connection::clear_transfer_cancel(task_id, relative_path, Some("receive"));
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let final_path = safe_join(&root, relative_path)?;
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial_path = partial_path(&final_path);
    let file = std::fs::File::create(&partial_path)?;

    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "receive",
        bytes_total = total_bytes,
        protocol_version = "v2",
        "incoming v2 file stream start"
    );
    let transfer_id =
        record_server_transfer_start(task_id, relative_path, "receive", total_bytes, "v2_binary");

    let transfer = IncomingTransfer {
        transfer_id,
        partial_path,
        final_path,
        file_hash: String::new(),
        total_bytes,
        written_bytes: 0,
        hasher: blake3::Hasher::new(),
        start_time: Instant::now(),
        first_byte_time: None,
        next_progress_at: TRANSFER_PROGRESS_INTERVAL_BYTES,
        next_ack_at: TRANSFER_V2_ACK_INTERVAL_BYTES,
        ack_every_chunk: false,
        protocol_version: "v2_binary",
        file,
        timing: V2ReceiveTiming::default(),
    };
    let mut incoming = task_roots.incoming.lock().unwrap();
    incoming.insert(transfer_key(task_id, relative_path), transfer);
    Ok(())
}

fn finish_incoming_v2(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    file_hash: &str,
) -> Result<()> {
    use std::io::Write;
    let key = transfer_key(task_id, relative_path);
    let mut transfer = {
        let mut incoming = task_roots.incoming.lock().unwrap();
        incoming
            .remove(&key)
            .ok_or_else(|| anyhow::anyhow!("v2 stream not started"))?
    };
    if transfer.written_bytes != transfer.total_bytes {
        connection::finish_transfer_progress(&transfer.transfer_id);
        log_v2_receive_timing_summary(
            &transfer.transfer_id,
            task_id,
            relative_path,
            transfer.total_bytes,
            elapsed_ms(transfer.start_time),
            &transfer.timing,
            false,
            Some("v2 stream size mismatch"),
        );
        anyhow::bail!("v2 stream size mismatch");
    }
    let hash_start = Instant::now();
    let actual_hash = transfer.hasher.finalize().to_hex().to_string();
    transfer.timing.hash_ms += elapsed_ms(hash_start);
    if actual_hash != file_hash {
        let _ = std::fs::remove_file(&transfer.partial_path);
        connection::finish_transfer_progress(&transfer.transfer_id);
        log_v2_receive_timing_summary(
            &transfer.transfer_id,
            task_id,
            relative_path,
            transfer.total_bytes,
            elapsed_ms(transfer.start_time),
            &transfer.timing,
            false,
            Some("v2 file hash mismatch"),
        );
        anyhow::bail!("v2 file hash mismatch");
    }
    let flush_start = Instant::now();
    if let Err(e) = transfer.file.flush() {
        log_v2_receive_timing_summary(
            &transfer.transfer_id,
            task_id,
            relative_path,
            transfer.total_bytes,
            elapsed_ms(transfer.start_time),
            &transfer.timing,
            false,
            Some("v2 flush failed"),
        );
        return Err(e.into());
    }
    transfer.timing.flush_ms += elapsed_ms(flush_start);
    drop(transfer.file);
    let rename_start = Instant::now();
    if let Err(e) = std::fs::rename(&transfer.partial_path, &transfer.final_path) {
        log_v2_receive_timing_summary(
            &transfer.transfer_id,
            task_id,
            relative_path,
            transfer.total_bytes,
            elapsed_ms(transfer.start_time),
            &transfer.timing,
            false,
            Some("v2 rename failed"),
        );
        return Err(e.into());
    }
    transfer.timing.rename_ms += elapsed_ms(rename_start);

    let total_elapsed_ms = transfer.start_time.elapsed().as_millis() as u64;
    let total_mbps = if total_elapsed_ms > 0 {
        (transfer.total_bytes as f64 / (1024.0 * 1024.0)) / (total_elapsed_ms as f64 / 1000.0)
    } else {
        0.0
    };
    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "receive",
        bytes_total = transfer.total_bytes,
        bytes_done = transfer.written_bytes,
        elapsed_ms = total_elapsed_ms,
        mbps = format_args!("{:.1}", total_mbps),
        protocol_version = transfer.protocol_version,
        "transfer complete"
    );
    log_v2_receive_timing_summary(
        &transfer.transfer_id,
        task_id,
        relative_path,
        transfer.total_bytes,
        total_elapsed_ms,
        &transfer.timing,
        true,
        None,
    );

    record_received_file(
        task_roots,
        task_id,
        relative_path,
        &transfer.final_path,
        file_hash,
    )?;
    connection::finish_transfer_progress(&transfer.transfer_id);
    Ok(())
}

async fn send_file_download_v2(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    reader: &mut tokio::net::tcp::OwnedReadHalf,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
) -> Result<()> {
    let transfer_start = Instant::now();
    let mut timing = V2ServeTiming::default();
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let source = safe_join(&root, relative_path)?;
    if !source.is_file() {
        anyhow::bail!("requested file not found");
    }
    ensure_transfer_not_deferred(task_id, relative_path, "serve")?;
    connection::clear_transfer_cancel(task_id, relative_path, Some("serve"));

    let before_hash = crate::transport::connection::source_file_state(&source)?;
    let total_bytes = before_hash.len;
    let _progress_guard =
        server_transfer_progress_guard(task_id, relative_path, "serve", total_bytes, "v2_binary");
    let result = send_file_download_v2_inner(
        task_id,
        relative_path,
        reader,
        writer,
        &source,
        &before_hash,
        total_bytes,
        &_progress_guard.transfer_id,
        &mut timing,
        transfer_start,
    )
    .await;
    let elapsed_ms = elapsed_ms(transfer_start);
    let error = result.as_ref().err().map(|e| e.to_string());
    log_v2_serve_timing_summary(
        &_progress_guard.transfer_id,
        task_id,
        relative_path,
        total_bytes,
        elapsed_ms,
        &timing,
        result.is_ok(),
        error.as_deref(),
    );
    result
}

async fn send_file_download_v2_inner(
    task_id: &str,
    relative_path: &str,
    reader: &mut tokio::net::tcp::OwnedReadHalf,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    source: &Path,
    before_hash: &crate::transport::connection::SourceFileState,
    total_bytes: u64,
    transfer_id: &str,
    timing: &mut V2ServeTiming,
    first_byte: Instant,
) -> Result<()> {
    use crate::transport::protocol::{encode_message, write_v2_chunk, SyncMessage};
    let write_start = Instant::now();
    writer
        .write_all(&encode_message(&SyncMessage::FileStreamStartV2 {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
            total_bytes,
        })?)
        .await?;
    timing.socket_write_ms += elapsed_ms(write_start);

    let mut file = std::fs::File::open(&source)?;
    let mut hasher = blake3::Hasher::new();
    let mut offset = 0u64;
    let mut next_ack_at = TRANSFER_V2_ACK_INTERVAL_BYTES;
    let mut next_progress_at = TRANSFER_PROGRESS_INTERVAL_BYTES;
    let mut buf = vec![0u8; TRANSFER_V2_CHUNK_SIZE];
    loop {
        connection::ensure_transfer_not_cancelled(task_id, relative_path, "serve")?;
        let read_start = Instant::now();
        let read = file.read(&mut buf)?;
        timing.read_ms += elapsed_ms(read_start);
        if read == 0 {
            break;
        }
        let chunk = &buf[..read];
        let hash_start = Instant::now();
        hasher.update(chunk);
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
        write_v2_chunk(writer, &header, chunk).await?;
        let write_ms = elapsed_ms(write_start);
        timing.socket_write_ms += write_ms;
        timing.chunk_socket_write_ms += write_ms;
        timing.chunk_count += 1;
        offset += read as u64;
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
                direction = "serve",
                bytes_total = total_bytes,
                bytes_done = offset,
                elapsed_ms = elapsed_ms,
                mbps = format_args!("{:.1}", mbps),
                protocol_version = "v2",
                ack_interval_bytes = TRANSFER_V2_ACK_INTERVAL_BYTES,
                "transfer progress"
            );
            connection::record_throttled(
                transfer_id,
                task_id,
                relative_path,
                "serve",
                offset,
                total_bytes,
                offset,
                mbps,
                "v2_binary",
            );
            next_progress_at += TRANSFER_PROGRESS_INTERVAL_BYTES;
        }
        if need_ack {
            let ack_start = Instant::now();
            match tokio::time::timeout(Duration::from_secs(10), read_server_message(reader)).await {
                Ok(Ok(SyncMessage::FileStreamAckV2 { success: true, .. })) => {}
                Ok(Ok(SyncMessage::FileStreamAckV2 {
                    success: false,
                    error,
                    ..
                })) => {
                    anyhow::bail!(
                        error.unwrap_or_else(|| "peer rejected v2 download chunk".to_string())
                    )
                }
                Ok(Ok(other)) => anyhow::bail!("unexpected v2 download ack: {:?}", other),
                Ok(Err(e)) => anyhow::bail!("v2 download ack read failed: {}", e),
                Err(_) => anyhow::bail!("v2 download ack timed out"),
            }
            timing.ack_wait_ms += elapsed_ms(ack_start);
            next_ack_at += TRANSFER_V2_ACK_INTERVAL_BYTES;
        }
    }

    crate::transport::connection::ensure_source_file_unchanged(&source, &before_hash)?;
    let file_hash = hasher.finalize().to_hex().to_string();
    let write_start = Instant::now();
    writer
        .write_all(&encode_message(&SyncMessage::FileStreamEndV2 {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
            file_hash,
        })?)
        .await?;
    timing.socket_write_ms += elapsed_ms(write_start);
    Ok(())
}

async fn send_file_download(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
) -> Result<()> {
    use crate::transport::protocol::{encode_message, SyncMessage};

    let total_start = Instant::now();
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let source = safe_join(&root, relative_path)?;
    if !source.is_file() {
        anyhow::bail!("requested file not found");
    }
    ensure_transfer_not_deferred(task_id, relative_path, "serve")?;
    connection::clear_transfer_cancel(task_id, relative_path, Some("serve"));

    let before_hash = crate::transport::connection::source_file_state(&source)?;
    let total_bytes = before_hash.len;
    let file_hash = crate::core::scanner::hash_file(&source)?;
    crate::transport::connection::ensure_source_file_unchanged(&source, &before_hash)?;
    let _progress_guard =
        server_transfer_progress_guard(task_id, relative_path, "serve", total_bytes, "v1_json");

    writer
        .write_all(&encode_message(&SyncMessage::FileChunkStart {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
            file_hash: file_hash.clone(),
            total_bytes,
        })?)
        .await?;

    let first_byte = Instant::now();
    let mut file = std::fs::File::open(&source)?;
    let mut hasher = blake3::Hasher::new();
    let mut offset = 0u64;
    let mut next_progress_at = TRANSFER_PROGRESS_INTERVAL_BYTES;
    let mut buf = vec![0u8; TRANSFER_V1_CHUNK_SIZE];
    loop {
        connection::ensure_transfer_not_cancelled(task_id, relative_path, "serve")?;
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        let chunk = &buf[..read];
        hasher.update(chunk);
        writer
            .write_all(&encode_message(&SyncMessage::FileChunk {
                task_id: task_id.to_string(),
                relative_path: relative_path.to_string(),
                offset,
                data: chunk.to_vec(),
            })?)
            .await?;
        offset += read as u64;
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
                direction = "serve",
                bytes_total = total_bytes,
                bytes_done = offset,
                elapsed_ms = elapsed_ms,
                mbps = format_args!("{:.1}", mbps),
                protocol_version = "v1",
                "transfer progress"
            );
            connection::record_throttled(
                &_progress_guard.transfer_id,
                task_id,
                relative_path,
                "serve",
                offset,
                total_bytes,
                offset,
                mbps,
                "v1_json",
            );
            next_progress_at += TRANSFER_PROGRESS_INTERVAL_BYTES;
        }
    }

    crate::transport::connection::ensure_source_file_unchanged(&source, &before_hash)?;
    let streamed_hash = hasher.finalize().to_hex().to_string();
    if streamed_hash != file_hash {
        anyhow::bail!("source file hash changed while streaming");
    }
    writer
        .write_all(&encode_message(&SyncMessage::FileChunkEnd {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
            file_hash: None,
        })?)
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
        direction = "serve",
        bytes_total = total_bytes,
        bytes_done = offset,
        elapsed_ms = total_elapsed_ms,
        mbps = format_args!("{:.1}", total_mbps),
        protocol_version = "v1",
        "transfer complete"
    );
    Ok(())
}

fn scan_task_root(task_roots: &TaskRootRegistry, task_id: &str) -> Result<Vec<RemoteFileState>> {
    let scan_start = Instant::now();
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    let mut file_count = 0usize;
    let mut dir_count = 0usize;
    let mut hashed_files = 0usize;
    let mut skipped_large_files = 0usize;
    let mut large_cache_hits = 0usize;

    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.depth() == 0 {
            continue;
        }
        let path = entry.path();
        if crate::core::transient::path_has_protocol_ignored_component(path) {
            continue;
        }
        let is_file = entry.file_type().is_file();
        let is_dir = entry.file_type().is_dir();
        if !is_file && !is_dir {
            continue;
        }
        let relative_path = path
            .strip_prefix(&root)?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = entry.metadata()?;
        let modified_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default();
        let (blake3_hash, hash_status) = if is_file {
            let size = metadata.len() as i64;
            let hash_state = remote_scan_hash_state(
                task_roots,
                task_id,
                &relative_path,
                path,
                size,
                modified_unix_ms,
            )?;
            match hash_state.1 {
                HashStatus::Verified => {
                    if size > crate::core::scanner::EAGER_HASH_LIMIT {
                        large_cache_hits += 1;
                    } else {
                        hashed_files += 1;
                    }
                }
                HashStatus::UnverifiedLargeFile => skipped_large_files += 1,
                HashStatus::Unavailable => {}
            }
            hash_state
        } else {
            (None, HashStatus::Unavailable)
        };
        if is_file {
            file_count += 1;
        } else {
            dir_count += 1;
        }
        files.push(RemoteFileState {
            relative_path,
            kind: if is_dir {
                EntryKind::Directory
            } else {
                EntryKind::File
            },
            blake3_hash,
            hash_status,
            size: if is_file { metadata.len() as i64 } else { 0 },
            modified_unix_ms,
        });
    }
    tracing::info!(
        remote_scan_summary = true,
        task_id,
        entries = files.len(),
        files = file_count,
        dirs = dir_count,
        hashed_files,
        skipped_large_files,
        cache_hits = large_cache_hits,
        elapsed_ms = scan_start.elapsed().as_millis() as u64,
    );
    Ok(files)
}

async fn read_server_message(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
) -> Result<crate::transport::protocol::SyncMessage> {
    use crate::transport::protocol;

    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let msg_len = u32::from_be_bytes(len_buf) as usize;
    if msg_len > 10 * 1024 * 1024 {
        anyhow::bail!("message too large: {} bytes", msg_len);
    }
    let mut msg_buf = vec![0u8; msg_len];
    reader.read_exact(&mut msg_buf).await?;
    Ok(protocol::decode_message(&msg_buf)?)
}

fn remote_scan_hash_state(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    path: &Path,
    size: i64,
    modified_unix_ms: i64,
) -> Result<(Option<String>, HashStatus)> {
    if size > crate::core::scanner::EAGER_HASH_LIMIT {
        if let Some(hash) =
            cached_verified_hash(task_roots, task_id, relative_path, size, modified_unix_ms)
        {
            return Ok((Some(hash), HashStatus::Verified));
        }
        return Ok((None, HashStatus::UnverifiedLargeFile));
    }
    Ok((
        Some(crate::core::scanner::hash_file(path)?),
        HashStatus::Verified,
    ))
}

fn cached_verified_hash(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    size: i64,
    modified_unix_ms: i64,
) -> Option<String> {
    let db_path = task_roots.state_db_path()?;
    let task_id = Uuid::parse_str(task_id).ok()?;
    let conn = db::open_db(&db_path).ok()?;
    db::migrate(&conn).ok()?;
    let snapshot = repository::FileSnapshotRepository::new(&conn)
        .get(&task_id, relative_path)
        .ok()
        .flatten()?;
    if snapshot.kind == EntryKind::File
        && snapshot.hash_status == HashStatus::Verified
        && snapshot.size == size
        && snapshot.modified_unix_ms == modified_unix_ms
    {
        snapshot.blake3_hash
    } else {
        None
    }
}

fn create_incoming_directory(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
) -> Result<()> {
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let target = safe_join(&root, relative_path)?;
    if target.exists() && !target.is_dir() {
        anyhow::bail!("target path exists and is not a directory");
    }
    std::fs::create_dir_all(&target)?;
    record_received_directory(task_roots, task_id, relative_path, &target)?;
    Ok(())
}

fn move_incoming_delete_to_history(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    expected_kind: Option<EntryKind>,
    expected_hash: Option<&str>,
    expected_hash_status: Option<HashStatus>,
    expected_size: Option<i64>,
    expected_modified_unix_ms: Option<i64>,
    delete_batch_id: Option<&str>,
) -> Result<()> {
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let target = safe_join(&root, relative_path)?;
    if !target.exists() {
        record_received_delete_missing(task_roots, task_id, relative_path)?;
        return Ok(());
    }
    validate_delete_expectation(
        &target,
        expected_kind,
        expected_hash,
        expected_hash_status,
        expected_size,
        expected_modified_unix_ms,
    )?;

    let history = HistoryStore::new(&root);
    history.check_storage_blocked(now_ms())?;

    let now = now_ms();
    let batch_id = delete_batch_id
        .map(|batch| batch.to_string())
        .unwrap_or_else(|| now.to_string());
    let mut entry = history.move_to_trash_in_batch(&target, relative_path, now, &batch_id)?;
    entry.task_id = Uuid::parse_str(task_id)?;
    record_received_delete(task_roots, task_id, relative_path, &entry)?;
    Ok(())
}

fn apply_incoming_conflict_file(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    staged_relative_path: &str,
    mode: &str,
) -> Result<String> {
    if mode != "overwrite" && mode != "keep_both" {
        anyhow::bail!("unsupported conflict mode");
    }
    if !staged_relative_path.starts_with(".lanbridge-temp/") {
        anyhow::bail!("invalid conflict staging path");
    }
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let staged = safe_join(&root, staged_relative_path)?;
    if !staged.is_file() {
        anyhow::bail!("staged conflict file missing");
    }

    let now = now_ms();
    let history = HistoryStore::new(&root);
    let task_uuid = Uuid::parse_str(task_id)?;
    let applied_relative_path = if mode == "overwrite" {
        let target = safe_join(&root, relative_path)?;
        if target.exists() {
            history.check_storage_blocked(now)?;
            let mut entry = history.move_to_overwritten(&target, relative_path, now)?;
            entry.task_id = task_uuid;
            if let Some(db_path) = task_roots.state_db_path() {
                let conn = db::open_db(&db_path)?;
                db::migrate(&conn)?;
                repository::HistoryRepository::new(&conn).insert(&entry)?;
            }
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&staged, &target)?;
        relative_path.to_string()
    } else {
        let conflict_relative_path =
            crate::core::conflict::conflict_filename(relative_path, "Secondary", now, |name| {
                root.join(name).exists()
            });
        let target = safe_join(&root, &conflict_relative_path)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&staged, &target)?;
        conflict_relative_path
    };

    let applied_path = safe_join(&root, &applied_relative_path)?;
    let applied_hash = crate::core::scanner::hash_file(&applied_path)?;
    record_received_file(
        task_roots,
        task_id,
        &applied_relative_path,
        &applied_path,
        &applied_hash,
    )?;
    cleanup_conflict_staging(task_roots, task_id, staged_relative_path);
    Ok(applied_relative_path)
}

fn cleanup_conflict_staging(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    staged_relative_path: &str,
) {
    let Some(db_path) = task_roots.state_db_path() else {
        return;
    };
    let Ok(task_uuid) = Uuid::parse_str(task_id) else {
        return;
    };
    let Ok(conn) = db::open_db(&db_path) else {
        return;
    };
    if db::migrate(&conn).is_err() {
        return;
    }
    let _ = write_db_with_retry(|| {
        repository::FileSnapshotRepository::new(&conn)
            .remove_tree(&task_uuid, staged_relative_path)?;
        repository::SyncBaselineRepository::new(&conn)
            .remove_tree(&task_uuid, staged_relative_path)?;
        Ok(())
    });
}

fn validate_delete_expectation(
    target: &Path,
    expected_kind: Option<EntryKind>,
    expected_hash: Option<&str>,
    expected_hash_status: Option<HashStatus>,
    expected_size: Option<i64>,
    expected_modified_unix_ms: Option<i64>,
) -> Result<()> {
    let Some(expected_kind) = expected_kind else {
        return Ok(());
    };
    let metadata = std::fs::metadata(target)?;
    let current_kind = if metadata.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::File
    };
    if current_kind != expected_kind {
        anyhow::bail!("remote content no longer matches delete baseline");
    }
    if expected_kind == EntryKind::Directory {
        return Ok(());
    }

    let hash_status = expected_hash_status.unwrap_or(HashStatus::Unavailable);
    if hash_status == HashStatus::Verified {
        let expected_hash = expected_hash
            .filter(|hash| !hash.is_empty())
            .ok_or_else(|| anyhow::anyhow!("delete baseline missing expected hash"))?;
        let actual_hash = crate::core::scanner::hash_file(target)?;
        if actual_hash == expected_hash {
            return Ok(());
        }
        anyhow::bail!("remote content no longer matches delete baseline");
    }

    let current_size = metadata.len() as i64;
    let current_modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default();
    if expected_size == Some(current_size) && expected_modified_unix_ms == Some(current_modified) {
        Ok(())
    } else {
        anyhow::bail!("remote content no longer matches delete baseline")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scan_task_root_skips_lanbridge_transient_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("ready.txt"), "done").unwrap();
        std::fs::write(dir.path().join("ready.txt.lanbridge-partial"), "incomplete").unwrap();
        std::fs::write(
            dir.path()
                .join("ready.txt.lanbridge-partial.lanbridge-partial"),
            "loop",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".lanbridge-temp")).unwrap();
        std::fs::write(
            dir.path().join(".lanbridge-temp").join("staged.txt"),
            "staged",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".lanbridge-history")).unwrap();
        std::fs::write(
            dir.path().join(".lanbridge-history").join("trashed.txt"),
            "old",
        )
        .unwrap();

        let registry = TaskRootRegistry::new();
        registry.register("task", dir.path()).unwrap();
        let files = scan_task_root(&registry, "task").unwrap();
        let paths: Vec<String> = files.into_iter().map(|file| file.relative_path).collect();

        assert_eq!(paths, vec!["ready.txt".to_string()]);
    }

    #[test]
    fn remote_scan_large_file_without_cache_does_not_hash() {
        let registry = TaskRootRegistry::new();
        let missing_path = Path::new("/tmp/lanbridge-missing-large-file.bin");

        let (hash, status) = remote_scan_hash_state(
            &registry,
            "task",
            "large.bin",
            missing_path,
            crate::core::scanner::EAGER_HASH_LIMIT + 1,
            1,
        )
        .unwrap();

        assert_eq!(hash, None);
        assert_eq!(status, HashStatus::UnverifiedLargeFile);
    }

    #[test]
    fn delete_expectation_rejects_changed_file_hash() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("changed.bin");
        std::fs::write(&file, b"changed").unwrap();

        let result = validate_delete_expectation(
            &file,
            Some(EntryKind::File),
            Some(&blake3::hash(b"baseline").to_hex().to_string()),
            Some(HashStatus::Verified),
            Some(7),
            Some(1),
        );

        assert!(result
            .unwrap_err()
            .to_string()
            .contains("remote content no longer matches delete baseline"));
    }
}

fn record_received_file(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    path: &Path,
    file_hash: &str,
) -> Result<()> {
    let Some(db_path) = task_roots.state_db_path() else {
        return Ok(());
    };
    let task_id = Uuid::parse_str(task_id)?;
    let conn = db::open_db(&db_path)?;
    db::migrate(&conn)?;
    let metadata = std::fs::metadata(path)?;
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_else(now_ms);
    let snapshot = FileSnapshot {
        task_id,
        relative_path: relative_path.to_string(),
        kind: EntryKind::File,
        size: metadata.len() as i64,
        modified_unix_ms,
        blake3_hash: Some(file_hash.to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };
    write_db_with_retry(|| {
        repository::FileSnapshotRepository::new(&conn).upsert(&snapshot)?;
        repository::SyncBaselineRepository::new(&conn).upsert(&SyncBaseline {
            task_id,
            relative_path: relative_path.to_string(),
            primary_hash: Some(file_hash.to_string()),
            primary_hash_status: HashStatus::Verified,
            primary_size: metadata.len() as i64,
            secondary_size: metadata.len() as i64,
            primary_modified_unix_ms: modified_unix_ms,
            secondary_hash: Some(file_hash.to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: modified_unix_ms,
            last_synced_unix_ms: now_ms(),
        })?;
        repository::LogRepository::new(&conn).insert(&LogEntry {
            id: None,
            level: LogLevel::Info,
            task_id: Some(task_id),
            relative_path: Some(relative_path.to_string()),
            message: "received file from peer".to_string(),
            created_unix_ms: now_ms(),
        })?;
        Ok(())
    })?;
    Ok(())
}

fn record_received_directory(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    path: &Path,
) -> Result<()> {
    let Some(db_path) = task_roots.state_db_path() else {
        return Ok(());
    };
    let task_id = Uuid::parse_str(task_id)?;
    let conn = db::open_db(&db_path)?;
    db::migrate(&conn)?;
    let modified_unix_ms = std::fs::metadata(path)?
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_else(now_ms);
    let snapshot = FileSnapshot {
        task_id,
        relative_path: relative_path.to_string(),
        kind: EntryKind::Directory,
        size: 0,
        modified_unix_ms,
        blake3_hash: None,
        hash_status: HashStatus::Unavailable,
        deleted: false,
        is_symlink: false,
    };
    write_db_with_retry(|| {
        repository::FileSnapshotRepository::new(&conn).upsert(&snapshot)?;
        repository::SyncBaselineRepository::new(&conn).upsert(&SyncBaseline {
            task_id,
            relative_path: relative_path.to_string(),
            primary_hash: None,
            primary_hash_status: HashStatus::Unavailable,
            primary_size: 0,
            secondary_size: 0,
            primary_modified_unix_ms: modified_unix_ms,
            secondary_hash: None,
            secondary_hash_status: HashStatus::Unavailable,
            secondary_modified_unix_ms: modified_unix_ms,
            last_synced_unix_ms: now_ms(),
        })?;
        repository::LogRepository::new(&conn).insert(&LogEntry {
            id: None,
            level: LogLevel::Info,
            task_id: Some(task_id),
            relative_path: Some(relative_path.to_string()),
            message: "received directory from peer".to_string(),
            created_unix_ms: now_ms(),
        })?;
        Ok(())
    })?;
    Ok(())
}

fn record_received_delete(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    history_entry: &HistoryEntry,
) -> Result<()> {
    let Some(db_path) = task_roots.state_db_path() else {
        return Ok(());
    };
    let task_id = Uuid::parse_str(task_id)?;
    let conn = db::open_db(&db_path)?;
    db::migrate(&conn)?;
    write_db_with_retry(|| {
        repository::FileSnapshotRepository::new(&conn).remove_tree(&task_id, relative_path)?;
        repository::SyncBaselineRepository::new(&conn).remove_tree(&task_id, relative_path)?;
        repository::HistoryRepository::new(&conn).insert(history_entry)?;
        repository::LogRepository::new(&conn).insert(&LogEntry {
            id: None,
            level: LogLevel::Info,
            task_id: Some(task_id),
            relative_path: Some(relative_path.to_string()),
            message: "received delete from peer".to_string(),
            created_unix_ms: now_ms(),
        })?;
        Ok(())
    })?;
    Ok(())
}

fn record_received_delete_missing(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
) -> Result<()> {
    let Some(db_path) = task_roots.state_db_path() else {
        return Ok(());
    };
    let task_id = Uuid::parse_str(task_id)?;
    let conn = db::open_db(&db_path)?;
    db::migrate(&conn)?;
    write_db_with_retry(|| {
        repository::FileSnapshotRepository::new(&conn).remove_tree(&task_id, relative_path)?;
        repository::SyncBaselineRepository::new(&conn).remove_tree(&task_id, relative_path)?;
        repository::LogRepository::new(&conn).insert(&LogEntry {
            id: None,
            level: LogLevel::Info,
            task_id: Some(task_id),
            relative_path: Some(relative_path.to_string()),
            message: "received idempotent delete from peer".to_string(),
            created_unix_ms: now_ms(),
        })?;
        Ok(())
    })?;
    Ok(())
}

fn write_db_with_retry<F>(mut write: F) -> Result<()>
where
    F: FnMut() -> Result<()>,
{
    for attempt in 0..3 {
        match write() {
            Ok(()) => return Ok(()),
            Err(error) if attempt < 2 && is_sqlite_busy(&error) => {
                std::thread::sleep(Duration::from_millis(100 * (attempt + 1) as u64));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn is_sqlite_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(inner, _))
                if matches!(
                    inner.code,
                    rusqlite::ErrorCode::DatabaseBusy
                        | rusqlite::ErrorCode::DatabaseLocked
                )
        )
    })
}

fn requester_address(peer_addr: SocketAddr, requester_port: u16) -> Option<String> {
    if requester_port == 0 {
        None
    } else {
        Some(format!("{}:{}", peer_addr.ip(), requester_port))
    }
}

fn validate_invite_local_path(path: &Path, proposed_role: &str) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("invite local path must exist");
    }
    if !path.is_dir() {
        anyhow::bail!("invite local path must be a directory");
    }

    let allow_non_empty = proposed_role == "Primary";
    let mut total_size = 0u64;
    let mut has_content = false;
    let mut walker = walkdir::WalkDir::new(path)
        .min_depth(1)
        .follow_links(false)
        .into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry?;
        let file_type = entry.file_type();
        let is_dir = file_type.is_dir();
        let name = entry.file_name().to_string_lossy().to_string();
        if crate::core::transient::is_common_ignored_entry_name(&name, is_dir) {
            if is_dir {
                walker.skip_current_dir();
            }
            continue;
        }
        has_content = true;
        if !allow_non_empty {
            anyhow::bail!(
                "invite local path must be empty; found non-ignored entry '{}'",
                name
            );
        }
        if file_type.is_file() {
            total_size = total_size.saturating_add(entry.metadata().map(|m| m.len()).unwrap_or(0));
        }
        if total_size > PRIMARY_NON_EMPTY_LIMIT_BYTES {
            anyhow::bail!("invite local path exceeds primary folder size limit");
        }
    }

    if allow_non_empty && has_content && total_size > PRIMARY_NON_EMPTY_LIMIT_BYTES {
        anyhow::bail!("invite local path exceeds primary folder size limit");
    }
    Ok(())
}

fn partial_path(dest: &Path) -> PathBuf {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".lanbridge-partial");
    PathBuf::from(tmp)
}

fn safe_folder_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "sync-task".to_string()
    } else {
        out.chars().take(48).collect()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
