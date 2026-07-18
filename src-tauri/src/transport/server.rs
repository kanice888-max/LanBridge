use anyhow::Result;
use ed25519_dalek::Signature;
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
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
use crate::core::path_safety::{
    create_safe_parent_dirs, ensure_safe_for_mutation, safe_join, MutationGuard, TaskRootHandle,
};
use crate::history::store::HistoryStore;
use crate::pairing::{DeviceIdentity, PublicIdentity};
use crate::state::{db, repository};
use crate::transport::connection::{self, auth_payload};
use crate::transport::protocol::{
    self, RemoteFileState, TRANSFER_PROGRESS_INTERVAL_BYTES, TRANSFER_V1_ACK_INTERVAL_BYTES,
    TRANSFER_V1_CHUNK_SIZE, TRANSFER_V2_ACK_INTERVAL_BYTES, TRANSFER_V2_CHUNK_SIZE,
};

const PRIMARY_NON_EMPTY_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceivedChangeKind {
    File,
    Directory,
    Delete,
    ConflictApply,
}

impl ReceivedChangeKind {
    pub const fn event_reason(self) -> &'static str {
        match self {
            Self::File => "received_file",
            Self::Directory => "received_directory",
            Self::Delete => "received_delete",
            Self::ConflictApply => "conflict_apply",
        }
    }
}

type ReceiveCommitNotifier = Arc<dyn Fn(String, ReceivedChangeKind) + Send + Sync>;
type TransferActivityNotifier = Arc<dyn Fn(String) + Send + Sync>;

/// Maps negotiated sync task IDs to the local root that should receive files.
#[derive(Clone, Default)]
pub struct TaskRootRegistry {
    roots: Arc<Mutex<HashMap<String, PathBuf>>>,
    authorized_peers: Arc<Mutex<HashMap<String, String>>>,
    trusted_peers: Arc<Mutex<HashMap<String, PublicIdentity>>>,
    incoming: Arc<Mutex<HashMap<String, IncomingTransfer>>>,
    incoming_leases: Arc<Mutex<HashMap<String, String>>>,
    persistence_path: Arc<Mutex<Option<PathBuf>>>,
    task_invite_persistence_path: Arc<Mutex<Option<PathBuf>>>,
    local_identity: Arc<Mutex<Option<PublicIdentity>>>,
    task_invite_inbox_root: Arc<Mutex<Option<PathBuf>>>,
    auto_accept_task_invites: Arc<Mutex<bool>>,
    task_invites: Arc<Mutex<HashMap<String, PendingTaskInvite>>>,
    state_db_path: Arc<Mutex<Option<PathBuf>>>,
    peer_requested_disconnects: Arc<Mutex<HashMap<String, Option<u64>>>>,
    local_disconnected_peers: Arc<Mutex<HashSet<String>>>,
    receive_commit_notifier: Arc<Mutex<Option<ReceiveCommitNotifier>>>,
    transfer_activity_notifier: Arc<Mutex<Option<TransferActivityNotifier>>>,
}

/// Tracks an in-progress file reception. Not `Clone` — `file: std::fs::File` is not cloneable.
/// The handle is opened once at transfer start, reused for every chunk append, and flushed+dropped
/// before the final rename to avoid Windows file-lock errors.
#[derive(Debug)]
struct IncomingTransfer {
    connection_id: String,
    task_id: String,
    relative_path: String,
    transfer_id: String,
    partial_path: PathBuf,
    final_path: PathBuf,
    mutation_guard: MutationGuard,
    file_hash: String,
    expected_target_hash: Option<String>,
    total_bytes: u64,
    written_bytes: u64,
    hasher: blake3::Hasher,
    start_time: Instant,
    first_byte_time: Option<Instant>,
    next_progress_at: u64,
    next_ack_at: u64,
    ack_every_chunk: bool,
    protocol_version: &'static str,
    file: Option<std::fs::File>,
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

#[derive(Debug)]
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
        if let Some(transfer) = incoming
            .values_mut()
            .find(|transfer| transfer.task_id == task_id && transfer.relative_path == relative_path)
        {
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
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    direction: &str,
    total_bytes: u64,
    protocol_version: &str,
) -> String {
    record_task_transfer_activity(task_roots, task_id);
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

struct IncomingConnectionCleanup {
    task_roots: TaskRootRegistry,
    connection_id: String,
}

impl Drop for IncomingConnectionCleanup {
    fn drop(&mut self) {
        self.task_roots
            .cancel_incoming_for_connection(&self.connection_id);
    }
}

fn server_transfer_progress_guard(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    direction: &str,
    total_bytes: u64,
    protocol_version: &str,
) -> ServerTransferProgressGuard {
    let transfer_id = record_server_transfer_start(
        task_roots,
        task_id,
        relative_path,
        direction,
        total_bytes,
        protocol_version,
    );
    ServerTransferProgressGuard { transfer_id }
}

fn record_task_transfer_activity(task_roots: &TaskRootRegistry, task_id: &str) {
    let Some(db_path) = task_roots.state_db_path() else {
        return;
    };
    let Ok(task_id) = Uuid::parse_str(task_id) else {
        tracing::warn!(task_id = %task_id, "ignored transfer activity for invalid task id");
        return;
    };
    let result = (|| -> Result<()> {
        let conn = db::open_db(&db_path)?;
        db::migrate(&conn)?;
        repository::SyncTaskRepository::new(&conn).mark_transfer_activity(&task_id, now_ms())?;
        Ok(())
    })();
    match result {
        Ok(()) => task_roots.notify_transfer_activity(&task_id.to_string()),
        Err(error) => {
            tracing::warn!(task_id = %task_id, error = %error, "failed to record transfer activity")
        }
    }
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

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PersistedTaskRoot {
    root: String,
    #[serde(default)]
    peer_device_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PersistedTaskRootEntry {
    Current(PersistedTaskRoot),
    Legacy(String),
}

impl TaskRootRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_receive_commit_notifier(
        &self,
        notifier: impl Fn(String, ReceivedChangeKind) + Send + Sync + 'static,
    ) {
        *self.receive_commit_notifier.lock().unwrap() = Some(Arc::new(notifier));
    }

    pub fn set_transfer_activity_notifier(
        &self,
        notifier: impl Fn(String) + Send + Sync + 'static,
    ) {
        *self.transfer_activity_notifier.lock().unwrap() = Some(Arc::new(notifier));
    }

    fn notify_receive_commit(&self, task_id: &str, kind: ReceivedChangeKind) {
        let notifier = self.receive_commit_notifier.lock().unwrap().clone();
        if let Some(notifier) = notifier {
            notifier(task_id.to_string(), kind);
        }
    }

    fn notify_transfer_activity(&self, task_id: &str) {
        let notifier = self.transfer_activity_notifier.lock().unwrap().clone();
        if let Some(notifier) = notifier {
            notifier(task_id.to_string());
        }
    }

    pub fn register(&self, task_id: impl Into<String>, root: impl AsRef<Path>) -> Result<()> {
        self.register_inner(task_id, root, None)
    }

    pub fn register_for_peer(
        &self,
        task_id: impl Into<String>,
        root: impl AsRef<Path>,
        peer_device_id: impl Into<String>,
    ) -> Result<()> {
        self.register_inner(task_id, root, Some(peer_device_id.into()))
    }

    fn register_inner(
        &self,
        task_id: impl Into<String>,
        root: impl AsRef<Path>,
        peer_device_id: Option<String>,
    ) -> Result<()> {
        let task_id = task_id.into();
        let root = root.as_ref();
        if !root.exists() {
            std::fs::create_dir_all(root)?;
        }

        let mut roots = self.roots.lock().unwrap();
        roots.insert(task_id.clone(), root.to_path_buf());
        drop(roots);
        let mut authorized_peers = self.authorized_peers.lock().unwrap();
        match peer_device_id {
            Some(peer_device_id) => {
                authorized_peers.insert(task_id, peer_device_id);
            }
            None => {
                authorized_peers.remove(&task_id);
            }
        }
        drop(authorized_peers);
        self.save_roots()?;
        Ok(())
    }

    pub fn unregister(&self, task_id: &str) -> Result<()> {
        let mut roots = self.roots.lock().unwrap();
        roots.remove(task_id);
        drop(roots);

        let mut authorized_peers = self.authorized_peers.lock().unwrap();
        authorized_peers.remove(task_id);
        drop(authorized_peers);

        self.cancel_incoming_for_task(task_id);

        self.save_roots()?;
        Ok(())
    }

    pub fn retain_registered_roots(&self, task_ids: &HashSet<String>) -> Result<()> {
        let mut roots = self.roots.lock().unwrap();
        roots.retain(|task_id, _| task_ids.contains(task_id));
        drop(roots);
        let mut authorized_peers = self.authorized_peers.lock().unwrap();
        authorized_peers.retain(|task_id, _| task_ids.contains(task_id));
        drop(authorized_peers);
        let removed_task_ids = {
            let incoming = self.incoming.lock().unwrap();
            incoming
                .values()
                .filter(|transfer| !task_ids.contains(&transfer.task_id))
                .map(|transfer| transfer.task_id.clone())
                .collect::<HashSet<_>>()
        };
        for task_id in removed_task_ids {
            self.cancel_incoming_for_task(&task_id);
        }
        self.save_roots()?;
        Ok(())
    }

    fn root_for(&self, task_id: &str) -> Option<PathBuf> {
        let roots = self.roots.lock().unwrap();
        roots.get(task_id).cloned()
    }

    fn authorize_task_access(&self, task_id: &str, device_id: &str) -> Result<()> {
        if self.root_for(task_id).is_none() {
            anyhow::bail!("task root not registered");
        }
        let authorized_peers = self.authorized_peers.lock().unwrap();
        match authorized_peers.get(task_id) {
            Some(authorized_peer) if authorized_peer == device_id => Ok(()),
            Some(_) => anyhow::bail!("peer is not authorized for task"),
            None => anyhow::bail!("task peer authorization is missing"),
        }
    }

    fn validate_task_registration(
        &self,
        task_id: &str,
        requested_root: &Path,
        peer_device_id: &str,
    ) -> Result<()> {
        let approved_root = self
            .root_for(task_id)
            .ok_or_else(|| anyhow::anyhow!("TaskNotApproved"))?;
        let authorized_peers = self.authorized_peers.lock().unwrap();
        match authorized_peers.get(task_id) {
            Some(authorized_peer) if authorized_peer == peer_device_id => {}
            Some(_) => anyhow::bail!("TaskPeerMismatch"),
            None => anyhow::bail!("TaskNotApproved"),
        }
        drop(authorized_peers);

        let approved_root = approved_root
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("approved task root is unavailable: {e}"))?;
        let requested_root = requested_root
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("TaskRootMismatch: {e}"))?;
        if approved_root != requested_root {
            anyhow::bail!("TaskRootMismatch");
        }
        Ok(())
    }

    pub fn cancel_incoming_transfer(&self, task_id: &str, relative_path: &str) -> Result<()> {
        let transfers = {
            let mut incoming = self.incoming.lock().unwrap();
            let keys = incoming
                .iter()
                .filter(|(_, transfer)| {
                    transfer.task_id == task_id && transfer.relative_path == relative_path
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| incoming.remove(&key))
                .collect::<Vec<_>>()
        };
        for transfer in transfers {
            cleanup_incoming_transfer(self, transfer);
        }
        Ok(())
    }

    fn cancel_incoming_transfer_for_connection(
        &self,
        connection_id: &str,
        task_id: &str,
        relative_path: &str,
    ) -> Result<()> {
        let transfer = {
            let mut incoming = self.incoming.lock().unwrap();
            incoming.remove(&transfer_key(connection_id, task_id, relative_path))
        }
        .ok_or_else(|| anyhow::anyhow!("transfer is not owned by this connection"))?;
        cleanup_incoming_transfer(self, transfer);
        Ok(())
    }

    fn cancel_incoming_for_task(&self, task_id: &str) {
        let transfers = {
            let mut incoming = self.incoming.lock().unwrap();
            let keys = incoming
                .iter()
                .filter(|(_, transfer)| transfer.task_id == task_id)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| incoming.remove(&key))
                .collect::<Vec<_>>()
        };
        for transfer in transfers {
            cleanup_incoming_transfer(self, transfer);
        }
    }

    pub fn cancel_incoming_for_connection(&self, connection_id: &str) {
        let transfers = {
            let mut incoming = self.incoming.lock().unwrap();
            let keys = incoming
                .iter()
                .filter(|(_, transfer)| transfer.connection_id == connection_id)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| incoming.remove(&key))
                .collect::<Vec<_>>()
        };
        for transfer in transfers {
            cleanup_incoming_transfer(self, transfer);
        }
    }

    fn cancel_all_incoming(&self) {
        let transfers = {
            let mut incoming = self.incoming.lock().unwrap();
            incoming
                .drain()
                .map(|(_, transfer)| transfer)
                .collect::<Vec<_>>()
        };
        for transfer in transfers {
            cleanup_incoming_transfer(self, transfer);
        }
    }

    fn acquire_incoming_lease(
        &self,
        connection_id: &str,
        task_id: &str,
        relative_path: &str,
    ) -> Result<()> {
        let lease_key = transfer_target_key(task_id, relative_path);
        let mut leases = self.incoming_leases.lock().unwrap();
        if leases.contains_key(&lease_key) {
            anyhow::bail!("TransferAlreadyInProgress");
        }
        leases.insert(lease_key, connection_id.to_string());
        Ok(())
    }

    fn release_incoming_lease(&self, transfer: &IncomingTransfer) {
        self.release_incoming_lease_key(
            &transfer.connection_id,
            &transfer.task_id,
            &transfer.relative_path,
        );
    }

    fn release_incoming_lease_key(&self, connection_id: &str, task_id: &str, relative_path: &str) {
        let lease_key = transfer_target_key(task_id, relative_path);
        let mut leases = self.incoming_leases.lock().unwrap();
        if leases.get(&lease_key).map(String::as_str) == Some(connection_id) {
            leases.remove(&lease_key);
        }
    }

    pub fn register_trusted_peer(&self, identity: PublicIdentity) {
        let mut peers = self.trusted_peers.lock().unwrap();
        peers.insert(identity.device_id.clone(), identity);
    }

    fn trusted_peer(&self, device_id: &str) -> Option<PublicIdentity> {
        let peers = self.trusted_peers.lock().unwrap();
        peers.get(device_id).cloned()
    }

    pub fn set_local_peer_disconnected(&self, device_id: &str, disconnected: bool) {
        let mut peers = self.local_disconnected_peers.lock().unwrap();
        if disconnected {
            peers.insert(device_id.to_string());
        } else {
            peers.remove(device_id);
        }
    }

    pub fn local_peer_disconnected(&self, device_id: &str) -> bool {
        self.local_disconnected_peers
            .lock()
            .unwrap()
            .contains(device_id)
    }

    pub fn load_peer_requested_disconnect(
        &self,
        device_id: &str,
        disconnected: bool,
        revision: Option<u64>,
    ) {
        let mut peers = self.peer_requested_disconnects.lock().unwrap();
        if disconnected {
            peers.insert(device_id.to_string(), revision);
        } else {
            peers.remove(device_id);
        }
    }

    pub fn apply_peer_requested_disconnect(
        &self,
        device_id: &str,
        disconnected: bool,
        revision: Option<u64>,
    ) -> Result<bool> {
        if let Some(revision) = revision {
            if let Some(db_path) = self.state_db_path() {
                let conn = db::open_db(&db_path)?;
                db::migrate(&conn)?;
                let applied = repository::PeerConnectionStateRepository::new(&conn)
                    .apply_remote_disconnected(device_id, disconnected, revision, now_ms())?;
                if !applied {
                    crate::diagnostics::record_operation(
                        "peer_connection_state_stale_ignored",
                        format!(
                            "peer_device_id={device_id} disconnected={disconnected} revision={revision}"
                        ),
                    );
                    return Ok(false);
                }
            }
        } else {
            if let Some(db_path) = self.state_db_path() {
                let conn = db::open_db(&db_path)?;
                db::migrate(&conn)?;
                let durable =
                    repository::PeerConnectionStateRepository::new(&conn).get(device_id)?;
                if durable.is_some_and(|state| state.remote_revision.is_some()) {
                    crate::diagnostics::record_operation(
                        "peer_connection_state_legacy_ignored",
                        format!(
                            "peer_device_id={device_id} disconnected={disconnected} durable revision already exists"
                        ),
                    );
                    return Ok(false);
                }
            }
            crate::diagnostics::record_operation(
                "LegacyProtocolFallback",
                format!("peer_device_id={device_id} connection state has no revision"),
            );
        }

        self.load_peer_requested_disconnect(device_id, disconnected, revision);
        crate::diagnostics::record_operation(
            "peer_connection_state_applied",
            format!("peer_device_id={device_id} disconnected={disconnected} revision={revision:?}"),
        );
        Ok(true)
    }

    pub fn clear_peer_requested_disconnect(&self, device_id: &str) {
        self.load_peer_requested_disconnect(device_id, false, None);
    }

    pub fn peer_requested_disconnect(&self, device_id: &str) -> bool {
        let peers = self.peer_requested_disconnects.lock().unwrap();
        peers.contains_key(device_id)
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
        self.register_for_peer(
            &invite_snapshot.task_id,
            &local_path,
            &invite_snapshot.requester_device_id,
        )?;

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
        if self
            .local_identity()
            .as_ref()
            .map(|identity| identity.device_id.as_str())
            == Some(requester_device_id.as_str())
        {
            anyhow::bail!("不能连接本机");
        }
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
            let persisted: HashMap<String, PersistedTaskRootEntry> =
                serde_json::from_slice(&bytes)?;
            let mut roots = self.roots.lock().unwrap();
            let mut authorized_peers = self.authorized_peers.lock().unwrap();
            for (task_id, entry) in persisted {
                match entry {
                    PersistedTaskRootEntry::Current(entry) => {
                        roots.insert(task_id.clone(), PathBuf::from(entry.root));
                        if let Some(peer_device_id) = entry.peer_device_id {
                            authorized_peers.insert(task_id, peer_device_id);
                        }
                    }
                    PersistedTaskRootEntry::Legacy(root) => {
                        roots.insert(task_id, PathBuf::from(root));
                    }
                }
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

    fn recover_incomplete_commits(&self) -> Result<usize> {
        let Some(db_path) = self.state_db_path() else {
            return Ok(0);
        };
        let conn = db::open_db(&db_path)?;
        db::migrate(&conn)?;
        let rows = {
            let mut stmt = conn.prepare(
                "SELECT commit_id, task_id, relative_path, incoming_hash, partial_path,
                        history_path, state, created_unix_ms
                 FROM transfer_commit_journal
                 ORDER BY created_unix_ms",
            )?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut recovered = 0;
        for (
            commit_id,
            task_id,
            relative_path,
            incoming_hash,
            partial_path,
            history_path,
            state,
            created_unix_ms,
        ) in rows
        {
            if state == "MetadataCommitted" {
                conn.execute(
                    "DELETE FROM transfer_commit_journal WHERE commit_id = ?1",
                    params![commit_id],
                )?;
                continue;
            }
            let Some(root) = self.root_for(&task_id) else {
                continue;
            };
            let final_path = match safe_join(&root, &relative_path) {
                Ok(path) => path,
                Err(error) => {
                    tracing::error!(
                        event = "UnsafePath",
                        task_id = %task_id,
                        relative_path = %relative_path,
                        error = %error,
                        "cannot recover unsafe receive commit"
                    );
                    continue;
                }
            };
            let target_matches = final_path.is_file()
                && crate::core::scanner::hash_file(&final_path)? == incoming_hash;
            if !target_matches {
                if let Err(error) = std::fs::remove_file(&partial_path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(
                            event = "PartialCleanupFailed",
                            partial_path = %partial_path,
                            error = %error,
                            "failed to remove abandoned recovery partial"
                        );
                    }
                }
                conn.execute(
                    "DELETE FROM transfer_commit_journal WHERE commit_id = ?1",
                    params![commit_id],
                )?;
                continue;
            }
            let history_entry = history_path.as_deref().and_then(|stored_path| {
                let metadata = std::fs::metadata(stored_path).ok()?;
                Some(HistoryEntry {
                    id: Uuid::parse_str(&commit_id).unwrap_or_else(|_| Uuid::new_v4()),
                    task_id: Uuid::parse_str(&task_id).ok()?,
                    original_relative_path: relative_path.clone(),
                    stored_path: stored_path.to_string(),
                    reason: crate::core::model::HistoryReason::Overwritten,
                    created_unix_ms,
                    size: metadata.len() as i64,
                })
            });
            record_received_file_commit(
                self,
                &task_id,
                &relative_path,
                &final_path,
                &incoming_hash,
                history_entry.as_ref(),
                Some(&commit_id),
                ReceivedChangeKind::File,
            )?;
            if let Err(error) = std::fs::remove_file(&partial_path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        event = "PartialCleanupFailed",
                        partial_path = %partial_path,
                        error = %error,
                        "failed to remove recovery partial after metadata commit"
                    );
                }
            }
            recovered += 1;
        }
        Ok(recovered)
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
        let authorized_peers = self.authorized_peers.lock().unwrap();
        let persisted = roots
            .iter()
            .map(|(task_id, root)| {
                (
                    task_id.clone(),
                    PersistedTaskRoot {
                        root: root.to_string_lossy().to_string(),
                        peer_device_id: authorized_peers.get(task_id).cloned(),
                    },
                )
            })
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
        Self::start_with_bound_listener(std::net::TcpListener::bind(format!("0.0.0.0:{}", port))?)
    }

    pub fn start_in_background_with_fallback(preferred_port: u16) -> Result<Self> {
        match Self::start_in_background(preferred_port) {
            Ok(server) => Ok(server),
            Err(preferred_error) => {
                tracing::warn!(
                    "failed to bind preferred sync port {}: {}; falling back to an OS-assigned port",
                    preferred_port,
                    preferred_error
                );
                Self::start_in_background(0)
            }
        }
    }

    fn start_with_bound_listener(listener: std::net::TcpListener) -> Result<Self> {
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
        peer_device_id: impl Into<String>,
    ) -> Result<()> {
        self.task_roots
            .register_for_peer(task_id, root, peer_device_id)
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

    pub fn set_receive_commit_notifier(
        &self,
        notifier: impl Fn(String, ReceivedChangeKind) + Send + Sync + 'static,
    ) {
        self.task_roots.set_receive_commit_notifier(notifier);
    }

    pub fn set_transfer_activity_notifier(
        &self,
        notifier: impl Fn(String) + Send + Sync + 'static,
    ) {
        self.task_roots.set_transfer_activity_notifier(notifier);
    }

    pub fn recover_incomplete_commits(&self) -> Result<usize> {
        self.task_roots.recover_incomplete_commits()
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

    pub fn peer_requested_disconnect(&self, device_id: &str) -> bool {
        self.task_roots.peer_requested_disconnect(device_id)
    }

    pub fn set_local_peer_disconnected(&self, device_id: &str, disconnected: bool) {
        self.task_roots
            .set_local_peer_disconnected(device_id, disconnected);
    }

    pub fn local_peer_disconnected(&self, device_id: &str) -> bool {
        self.task_roots.local_peer_disconnected(device_id)
    }

    pub fn load_peer_requested_disconnect(
        &self,
        device_id: &str,
        disconnected: bool,
        revision: Option<u64>,
    ) {
        self.task_roots
            .load_peer_requested_disconnect(device_id, disconnected, revision);
    }

    pub fn clear_peer_requested_disconnect(&self, device_id: &str) {
        self.task_roots.clear_peer_requested_disconnect(device_id)
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

#[cfg(test)]
mod sync_server_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn background_server_falls_back_when_preferred_port_is_busy() {
        let busy = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let preferred_port = busy.local_addr().unwrap().port();

        let server = SyncServer::start_in_background_with_fallback(preferred_port).unwrap();

        assert_ne!(server.port(), preferred_port);
        assert!(server.port() > 0);
    }

    #[test]
    fn task_operations_require_both_peers_to_allow_connection() {
        let dir = TempDir::new().unwrap();
        let registry = TaskRootRegistry::new();
        registry
            .register_for_peer("task-a", dir.path(), "peer-a")
            .unwrap();
        let authenticated = Some("peer-a".to_string());

        assert!(require_authorized_task(&registry, &authenticated, "task-a").is_ok());

        registry.set_local_peer_disconnected("peer-a", true);
        assert_eq!(
            require_authorized_task(&registry, &authenticated, "task-a")
                .unwrap_err()
                .to_string(),
            "PeerDisconnected"
        );

        registry.set_local_peer_disconnected("peer-a", false);
        registry
            .apply_peer_requested_disconnect("peer-a", true, Some(1))
            .unwrap();
        assert_eq!(
            require_authorized_task(&registry, &authenticated, "task-a")
                .unwrap_err()
                .to_string(),
            "PeerDisconnected"
        );
    }

    #[test]
    fn legacy_state_message_cannot_override_a_durable_revision() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("state.db");
        let conn = db::open_db(&db_path).unwrap();
        db::migrate(&conn).unwrap();
        repository::PairedDeviceRepository::new(&conn)
            .upsert(&crate::core::model::PairedDevice {
                device_id: "peer-a".to_string(),
                display_name: "Peer A".to_string(),
                public_key: vec![1, 2, 3],
                last_seen_unix_ms: 1,
                trusted: true,
                last_address: None,
            })
            .unwrap();
        drop(conn);

        let registry = TaskRootRegistry::new();
        registry.set_state_db_path(&db_path).unwrap();
        assert!(registry
            .apply_peer_requested_disconnect("peer-a", true, Some(5))
            .unwrap());
        assert!(!registry
            .apply_peer_requested_disconnect("peer-a", false, None)
            .unwrap());
        assert!(registry.peer_requested_disconnect("peer-a"));
    }
}

impl Drop for SyncServer {
    fn drop(&mut self) {
        self.task_roots.cancel_all_incoming();
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

fn file_ack(
    task_id: String,
    relative_path: String,
    success: bool,
    error: Option<String>,
) -> protocol::SyncMessage {
    protocol::SyncMessage::FileAck {
        task_id,
        relative_path,
        success,
        error,
        resolution: None,
        conflict_path: None,
        primary_hash: None,
        secondary_hash: None,
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    task_roots: TaskRootRegistry,
) -> Result<()> {
    use crate::transport::protocol;
    let connection_id = Uuid::new_v4().to_string();
    let _incoming_cleanup = IncomingConnectionCleanup {
        task_roots: task_roots.clone(),
        connection_id: connection_id.clone(),
    };
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
                                app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                                protocol_version: 2,
                                min_protocol_version: 1,
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
                    protocol::SyncMessage::PeerDisconnect {
                        device_id,
                        state_revision,
                    } => {
                        if authenticated_device_id.as_deref() != Some(device_id.as_str()) {
                            let reject = protocol::SyncMessage::AuthReject {
                                reason: "peer disconnect requires authentication".to_string(),
                            };
                            writer
                                .write_all(&protocol::encode_message(&reject)?)
                                .await?;
                            continue;
                        }
                        task_roots.apply_peer_requested_disconnect(
                            &device_id,
                            true,
                            state_revision,
                        )?;
                        let ack = protocol::SyncMessage::PeerDisconnectAck {
                            device_id,
                            state_revision,
                        };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::PeerReconnect {
                        device_id,
                        state_revision,
                    } => {
                        if authenticated_device_id.as_deref() != Some(device_id.as_str()) {
                            let reject = protocol::SyncMessage::AuthReject {
                                reason: "peer reconnect requires authentication".to_string(),
                            };
                            writer
                                .write_all(&protocol::encode_message(&reject)?)
                                .await?;
                            continue;
                        }
                        task_roots.apply_peer_requested_disconnect(
                            &device_id,
                            false,
                            state_revision,
                        )?;
                        let ack = protocol::SyncMessage::PeerReconnectAck {
                            device_id,
                            state_revision,
                        };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::TransferCancel {
                        task_id,
                        relative_path,
                        direction,
                    } => {
                        let ack = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
                        .and_then(|_| {
                            if direction.as_deref() == Some("serve") {
                                connection::cancel_active_transfer(
                                    &task_id,
                                    &relative_path,
                                    direction.as_deref(),
                                );
                                Ok(())
                            } else {
                                task_roots.cancel_incoming_transfer_for_connection(
                                    &connection_id,
                                    &task_id,
                                    &relative_path,
                                )
                            }
                        }) {
                            Ok(()) => file_ack(task_id, relative_path, true, None),
                            Err(e) => file_ack(task_id, relative_path, false, Some(e.to_string())),
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
                        let ack = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
                        .and_then(|_| {
                            write_incoming_file(
                                &task_roots,
                                &task_id,
                                &relative_path,
                                &file_hash,
                                total_bytes,
                                &data,
                            )
                        }) {
                            Ok(()) => file_ack(task_id, relative_path, true, None),
                            Err(e) => file_ack(task_id, relative_path, false, Some(e.to_string())),
                        };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::FileChunkStart {
                        task_id,
                        relative_path,
                        file_hash,
                        total_bytes,
                        expected_target_hash,
                    } => {
                        let ack = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
                        .and_then(|_| {
                            start_incoming_chunked_file(
                                &task_roots,
                                &task_id,
                                &relative_path,
                                &file_hash,
                                total_bytes,
                                expected_target_hash.as_deref(),
                                &connection_id,
                            )
                        }) {
                            Ok(()) => file_ack(task_id, relative_path, true, None),
                            Err(e) => file_ack(task_id, relative_path, false, Some(e.to_string())),
                        };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::FileChunk {
                        task_id,
                        relative_path,
                        offset,
                        data,
                    } => {
                        let ack = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
                        .and_then(|_| {
                            append_incoming_chunk(
                                &task_roots,
                                &connection_id,
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
                                    Some(file_ack(task_id, relative_path, true, None))
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
                        let ack = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
                        .and_then(|_| {
                            finish_incoming_chunked_file(
                                &task_roots,
                                &connection_id,
                                &task_id,
                                &relative_path,
                                file_hash.as_deref(),
                            )
                        }) {
                            Ok(()) => file_ack(task_id, relative_path, true, None),
                            Err(e) => file_ack(task_id, relative_path, false, Some(e.to_string())),
                        };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::FileStreamStartV2 {
                        task_id,
                        relative_path,
                        total_bytes,
                        expected_target_hash,
                    } => {
                        if let Err(e) =
                            require_authorized_task(&task_roots, &authenticated_device_id, &task_id)
                                .and_then(|_| {
                                    start_incoming_v2(
                                        &task_roots,
                                        &task_id,
                                        &relative_path,
                                        total_bytes,
                                        expected_target_hash.as_deref(),
                                        &connection_id,
                                    )
                                })
                        {
                            let ack = file_ack(task_id, relative_path, false, Some(e.to_string()));
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
                        let ack_result = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        ) {
                            Ok(()) => {
                                let task_roots_for_write = task_roots.clone();
                                let connection_id_for_write = connection_id.clone();
                                let task_id_for_write = task_id.clone();
                                let relative_path_for_write = relative_path.clone();
                                tokio::task::spawn_blocking(move || {
                                    append_incoming_chunk(
                                        &task_roots_for_write,
                                        &connection_id_for_write,
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
                        let ack = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
                        .and_then(|_| {
                            finish_incoming_v2(
                                &task_roots,
                                &connection_id,
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
                    } => match require_authorized_task(
                        &task_roots,
                        &authenticated_device_id,
                        &task_id,
                    ) {
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
                                let ack =
                                    file_ack(task_id, relative_path, false, Some(e.to_string()));
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
                            let ack = file_ack(task_id, relative_path, false, Some(e.to_string()));
                            writer.write_all(&protocol::encode_message(&ack)?).await?;
                        }
                    },
                    protocol::SyncMessage::FileDownloadRequest {
                        task_id,
                        relative_path,
                    } => match require_authorized_task(
                        &task_roots,
                        &authenticated_device_id,
                        &task_id,
                    ) {
                        Ok(()) => {
                            if let Err(e) = send_file_download(
                                &task_roots,
                                &task_id,
                                &relative_path,
                                &mut writer,
                            )
                            .await
                            {
                                let ack =
                                    file_ack(task_id, relative_path, false, Some(e.to_string()));
                                writer.write_all(&protocol::encode_message(&ack)?).await?;
                            }
                        }
                        Err(e) => {
                            let ack = file_ack(task_id, relative_path, false, Some(e.to_string()));
                            writer.write_all(&protocol::encode_message(&ack)?).await?;
                        }
                    },
                    protocol::SyncMessage::DirectoryCreate {
                        task_id,
                        relative_path,
                    } => {
                        let ack = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
                        .and_then(|_| {
                            record_task_transfer_activity(&task_roots, &task_id);
                            create_incoming_directory(&task_roots, &task_id, &relative_path)
                        }) {
                            Ok(()) => file_ack(task_id, relative_path, true, None),
                            Err(e) => file_ack(task_id, relative_path, false, Some(e.to_string())),
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
                        let ack = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
                        .and_then(|_| {
                            record_task_transfer_activity(&task_roots, &task_id);
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
                            Ok(()) => file_ack(task_id, relative_path, true, None),
                            Err(e) => file_ack(task_id, relative_path, false, Some(e.to_string())),
                        };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::ConflictApply {
                        task_id,
                        relative_path,
                        staged_relative_path,
                        mode,
                        resolution_id,
                    } => {
                        let ack = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
                        .and_then(|_| {
                            record_task_transfer_activity(&task_roots, &task_id);
                            apply_incoming_conflict_file(
                                &task_roots,
                                &task_id,
                                &relative_path,
                                &staged_relative_path,
                                &mode,
                                resolution_id.as_deref(),
                            )
                        }) {
                            Ok(outcome) => protocol::SyncMessage::FileAck {
                                task_id,
                                relative_path: outcome.applied_path.clone(),
                                success: true,
                                error: None,
                                resolution: Some(mode),
                                conflict_path: Some(outcome.applied_path),
                                primary_hash: outcome.primary_hash,
                                secondary_hash: Some(outcome.secondary_hash),
                            },
                            Err(e) => file_ack(task_id, relative_path, false, Some(e.to_string())),
                        };
                        writer.write_all(&protocol::encode_message(&ack)?).await?;
                    }
                    protocol::SyncMessage::TaskRegister { task_id, root_path } => {
                        let ack = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
                        .and_then(|_| require_authenticated(&authenticated_device_id))
                        .and_then(|device_id| {
                            task_roots.validate_task_registration(
                                &task_id,
                                Path::new(&root_path),
                                device_id,
                            )
                        }) {
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
                        let ack = match require_authenticated(&authenticated_device_id).and_then(
                            |requester_device_id| {
                                let requester_device_id = requester_device_id.to_string();
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
                                task_roots.register_for_peer(
                                    &task_id,
                                    &root,
                                    &requester_device_id,
                                )?;
                                Ok(protocol::SyncMessage::TaskInviteAck {
                                    task_id,
                                    success: true,
                                    remote_path: Some(root.to_string_lossy().to_string()),
                                    error: None,
                                })
                            },
                        ) {
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
                    | protocol::SyncMessage::PeerDisconnectAck { .. }
                    | protocol::SyncMessage::PeerReconnectAck { .. }
                    | protocol::SyncMessage::TransferReady { .. } => {
                        tracing::info!("received control message from {}", peer_addr);
                    }
                    protocol::SyncMessage::ScanRequest { task_id } => {
                        let response = match require_authorized_task(
                            &task_roots,
                            &authenticated_device_id,
                            &task_id,
                        )
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

fn require_authenticated(authenticated_device_id: &Option<String>) -> Result<&str> {
    authenticated_device_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("peer is not authenticated"))
}

fn require_authorized_task(
    task_roots: &TaskRootRegistry,
    authenticated_device_id: &Option<String>,
    task_id: &str,
) -> Result<()> {
    let device_id = require_authenticated(authenticated_device_id)?;
    task_roots.authorize_task_access(task_id, device_id)?;
    if task_roots.local_peer_disconnected(device_id)
        || task_roots.peer_requested_disconnect(device_id)
    {
        anyhow::bail!("PeerDisconnected");
    }
    Ok(())
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
    validate_incoming_relative_path(relative_path)?;
    let dest = safe_join(&root, relative_path)?;
    if dest.exists() {
        anyhow::bail!("TargetPreconditionFailed");
    }
    create_safe_parent_dirs(&root, &dest)?;

    let partial = unique_incoming_partial_path(&root)?;
    let mut partial_cleanup = ServerPartialFileCleanup::new(partial.clone());
    std::fs::write(&partial, data)?;
    commit_received_partial(
        task_roots,
        task_id,
        relative_path,
        &partial,
        &dest,
        file_hash,
        Some(""),
        ReceivedChangeKind::File,
    )?;
    partial_cleanup.commit();
    Ok(())
}

fn transfer_target_key(task_id: &str, relative_path: &str) -> String {
    format!("{}\n{}", task_id, relative_path)
}

fn transfer_key(connection_id: &str, task_id: &str, relative_path: &str) -> String {
    format!("{}\n{}\n{}", connection_id, task_id, relative_path)
}

fn cleanup_incoming_transfer(task_roots: &TaskRootRegistry, transfer: IncomingTransfer) {
    task_roots.release_incoming_lease(&transfer);
    let transfer_id = transfer.transfer_id.clone();
    drop(transfer.file);
    if let Err(error) = std::fs::remove_file(&transfer.partial_path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                event = "PartialCleanupFailed",
                partial_path = %transfer.partial_path.display(),
                error = %error,
                "failed to remove incoming partial file"
            );
        }
    }
    connection::finish_transfer_progress(&transfer_id);
}

struct ServerPartialFileCleanup {
    path: PathBuf,
    committed: bool,
}

impl ServerPartialFileCleanup {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for ServerPartialFileCleanup {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Err(error) = std::fs::remove_file(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    event = "PartialCleanupFailed",
                    partial_path = %self.path.display(),
                    error = %error,
                    "failed to remove incoming partial file"
                );
            }
        }
    }
}

fn validate_incoming_relative_path(relative_path: &str) -> Result<()> {
    let normalized = relative_path.replace('\\', "/");
    let first = normalized.split('/').next().unwrap_or_default();
    let is_conflict_staging = normalized.starts_with(".lanbridge-temp/conflict-");
    if matches!(
        first,
        ".lanbridge-history"
            | ".lanbridge-temp"
            | "lanbridge.log"
            | "startup-crash.log"
            | "crash-diagnostics.log"
    ) && !is_conflict_staging
    {
        anyhow::bail!("UnsafePath: LanBridge internal path is reserved");
    }
    if normalized.ends_with(".lanbridge-partial") {
        anyhow::bail!("UnsafePath: partial path suffix is reserved");
    }
    Ok(())
}

fn unique_incoming_partial_path(root: &Path) -> Result<PathBuf> {
    let relative = format!(
        ".lanbridge-temp/incoming/{}.lanbridge-partial",
        Uuid::new_v4()
    );
    let path = safe_join(root, &relative)?;
    create_safe_parent_dirs(root, &path)?;
    Ok(path)
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
    expected_target_hash: Option<&str>,
    connection_id: &str,
) -> Result<()> {
    ensure_transfer_not_deferred(task_id, relative_path, "receive")?;
    validate_incoming_relative_path(relative_path)?;
    connection::clear_transfer_cancel(task_id, relative_path, Some("receive"));
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let root_handle = TaskRootHandle::new(&root)?;
    let final_path = root_handle.resolve(relative_path)?;
    if expected_target_hash.is_none() && final_path.exists() {
        anyhow::bail!("TargetPreconditionFailed");
    }
    ensure_receive_target_precondition(
        task_roots,
        task_id,
        relative_path,
        &final_path,
        expected_target_hash,
    )?;
    let mutation_guard = root_handle.prepare_mutation(&final_path)?;
    task_roots.acquire_incoming_lease(connection_id, task_id, relative_path)?;
    let setup = (|| -> Result<(PathBuf, std::fs::File)> {
        let partial_path = unique_incoming_partial_path(&root)?;
        let file = std::fs::File::create(&partial_path)?;
        Ok((partial_path, file))
    })();
    let (partial_path, file) = match setup {
        Ok(value) => value,
        Err(error) => {
            task_roots.release_incoming_lease_key(connection_id, task_id, relative_path);
            return Err(error);
        }
    };

    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "receive",
        bytes_total = total_bytes,
        "incoming chunked file start"
    );
    let transfer_id = record_server_transfer_start(
        task_roots,
        task_id,
        relative_path,
        "receive",
        total_bytes,
        "v1_json",
    );

    let transfer = IncomingTransfer {
        connection_id: connection_id.to_string(),
        task_id: task_id.to_string(),
        relative_path: relative_path.to_string(),
        transfer_id,
        partial_path,
        final_path,
        mutation_guard,
        file_hash: file_hash.to_string(),
        expected_target_hash: expected_target_hash.map(str::to_string),
        total_bytes,
        written_bytes: 0,
        hasher: blake3::Hasher::new(),
        start_time: Instant::now(),
        first_byte_time: None,
        next_progress_at: TRANSFER_PROGRESS_INTERVAL_BYTES,
        next_ack_at: TRANSFER_V1_ACK_INTERVAL_BYTES,
        ack_every_chunk: !file_hash.is_empty(),
        protocol_version: "v1_json",
        file: Some(file),
        timing: V2ReceiveTiming::default(),
    };
    let mut incoming = task_roots.incoming.lock().unwrap();
    incoming.insert(
        transfer_key(connection_id, task_id, relative_path),
        transfer,
    );
    Ok(())
}

/// Append a chunk to an incoming transfer.
/// Returns `Some(written_bytes)` when the receiver should send a checkpoint ACK,
/// `None` when the sender should keep streaming without waiting.
fn append_incoming_chunk(
    task_roots: &TaskRootRegistry,
    connection_id: &str,
    task_id: &str,
    relative_path: &str,
    offset: u64,
    data: &[u8],
) -> Result<IncomingChunkAck> {
    if connection::is_transfer_cancelled(task_id, relative_path, "receive") {
        let _ = task_roots.cancel_incoming_transfer(task_id, relative_path);
        anyhow::bail!("transfer cancelled");
    }

    let key = transfer_key(connection_id, task_id, relative_path);
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
        let transfer = incoming.remove(&key);
        drop(incoming);
        if let Some(transfer) = transfer {
            cleanup_incoming_transfer(task_roots, transfer);
        } else {
            let _ = std::fs::remove_file(partial_path);
        }
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
    let write_result = transfer
        .file
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("incoming transfer file is closed"))?
        .write_all(data);
    if let Err(error) = write_result {
        let transfer = incoming.remove(&key);
        drop(incoming);
        if let Some(transfer) = transfer {
            cleanup_incoming_transfer(task_roots, transfer);
        }
        return Err(error.into());
    }
    transfer.timing.file_write_ms += elapsed_ms(write_start);
    let hash_start = Instant::now();
    transfer.hasher.update(data);
    transfer.timing.hash_ms += elapsed_ms(hash_start);
    transfer.timing.chunk_count += 1;
    transfer.written_bytes += data.len() as u64;
    if transfer.written_bytes > transfer.total_bytes {
        let transfer = incoming.remove(&key);
        drop(incoming);
        if let Some(transfer) = transfer {
            cleanup_incoming_transfer(task_roots, transfer);
        }
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
    connection_id: &str,
    task_id: &str,
    relative_path: &str,
    end_hash: Option<&str>,
) -> Result<()> {
    use std::io::Write;
    let key = transfer_key(connection_id, task_id, relative_path);
    let mut transfer = {
        let mut incoming = task_roots.incoming.lock().unwrap();
        incoming
            .remove(&key)
            .ok_or_else(|| anyhow::anyhow!("chunked transfer not started"))?
    };
    if transfer.written_bytes != transfer.total_bytes {
        cleanup_incoming_transfer(task_roots, transfer);
        anyhow::bail!("chunked transfer size mismatch");
    }
    let expected_hash = end_hash
        .filter(|h| !h.is_empty())
        .unwrap_or(&transfer.file_hash)
        .to_string();
    let actual_hash = transfer.hasher.finalize().to_hex().to_string();
    if actual_hash != expected_hash {
        cleanup_incoming_transfer(task_roots, transfer);
        anyhow::bail!("file hash mismatch");
    }
    if let Err(error) = transfer
        .file
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("incoming transfer file is closed"))
        .and_then(|file| file.flush().map_err(anyhow::Error::from))
    {
        cleanup_incoming_transfer(task_roots, transfer);
        return Err(error.into());
    }
    transfer.file.take();
    if let Err(error) = transfer.mutation_guard.validate() {
        cleanup_incoming_transfer(task_roots, transfer);
        return Err(error);
    }
    if let Err(error) = ensure_safe_for_mutation(
        &task_roots
            .root_for(task_id)
            .ok_or_else(|| anyhow::anyhow!("task root not registered"))?,
        &transfer.final_path,
    ) {
        cleanup_incoming_transfer(task_roots, transfer);
        return Err(error);
    }
    if let Err(error) = ensure_receive_target_precondition(
        task_roots,
        task_id,
        relative_path,
        &transfer.final_path,
        transfer.expected_target_hash.as_deref(),
    ) {
        cleanup_incoming_transfer(task_roots, transfer);
        return Err(error);
    }
    if let Err(error) = commit_received_partial(
        task_roots,
        task_id,
        relative_path,
        &transfer.partial_path,
        &transfer.final_path,
        &expected_hash,
        transfer.expected_target_hash.as_deref(),
        ReceivedChangeKind::File,
    ) {
        cleanup_incoming_transfer(task_roots, transfer);
        return Err(error);
    }

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

    task_roots.release_incoming_lease(&transfer);
    connection::finish_transfer_progress(&transfer.transfer_id);
    Ok(())
}

fn start_incoming_v2(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    total_bytes: u64,
    expected_target_hash: Option<&str>,
    connection_id: &str,
) -> Result<()> {
    ensure_transfer_not_deferred(task_id, relative_path, "receive")?;
    validate_incoming_relative_path(relative_path)?;
    connection::clear_transfer_cancel(task_id, relative_path, Some("receive"));
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let root_handle = TaskRootHandle::new(&root)?;
    let final_path = root_handle.resolve(relative_path)?;
    if expected_target_hash.is_none() && final_path.exists() {
        anyhow::bail!("TargetPreconditionFailed");
    }
    ensure_receive_target_precondition(
        task_roots,
        task_id,
        relative_path,
        &final_path,
        expected_target_hash,
    )?;
    let mutation_guard = root_handle.prepare_mutation(&final_path)?;
    task_roots.acquire_incoming_lease(connection_id, task_id, relative_path)?;
    let setup = (|| -> Result<(PathBuf, std::fs::File)> {
        let partial_path = unique_incoming_partial_path(&root)?;
        let file = std::fs::File::create(&partial_path)?;
        Ok((partial_path, file))
    })();
    let (partial_path, file) = match setup {
        Ok(value) => value,
        Err(error) => {
            task_roots.release_incoming_lease_key(connection_id, task_id, relative_path);
            return Err(error);
        }
    };

    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        direction = "receive",
        bytes_total = total_bytes,
        protocol_version = "v2",
        "incoming v2 file stream start"
    );
    let transfer_id = record_server_transfer_start(
        task_roots,
        task_id,
        relative_path,
        "receive",
        total_bytes,
        "v2_binary",
    );

    let transfer = IncomingTransfer {
        connection_id: connection_id.to_string(),
        task_id: task_id.to_string(),
        relative_path: relative_path.to_string(),
        transfer_id,
        partial_path,
        final_path,
        mutation_guard,
        file_hash: String::new(),
        expected_target_hash: expected_target_hash.map(str::to_string),
        total_bytes,
        written_bytes: 0,
        hasher: blake3::Hasher::new(),
        start_time: Instant::now(),
        first_byte_time: None,
        next_progress_at: TRANSFER_PROGRESS_INTERVAL_BYTES,
        next_ack_at: TRANSFER_V2_ACK_INTERVAL_BYTES,
        ack_every_chunk: false,
        protocol_version: "v2_binary",
        file: Some(file),
        timing: V2ReceiveTiming::default(),
    };
    let mut incoming = task_roots.incoming.lock().unwrap();
    incoming.insert(
        transfer_key(connection_id, task_id, relative_path),
        transfer,
    );
    Ok(())
}

fn finish_incoming_v2(
    task_roots: &TaskRootRegistry,
    connection_id: &str,
    task_id: &str,
    relative_path: &str,
    file_hash: &str,
) -> Result<()> {
    use std::io::Write;
    let key = transfer_key(connection_id, task_id, relative_path);
    let mut transfer = {
        let mut incoming = task_roots.incoming.lock().unwrap();
        incoming
            .remove(&key)
            .ok_or_else(|| anyhow::anyhow!("v2 stream not started"))?
    };
    if transfer.written_bytes != transfer.total_bytes {
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
        cleanup_incoming_transfer(task_roots, transfer);
        anyhow::bail!("v2 stream size mismatch");
    }
    let hash_start = Instant::now();
    let actual_hash = transfer.hasher.finalize().to_hex().to_string();
    transfer.timing.hash_ms += elapsed_ms(hash_start);
    if actual_hash != file_hash {
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
        cleanup_incoming_transfer(task_roots, transfer);
        anyhow::bail!("v2 file hash mismatch");
    }
    let flush_start = Instant::now();
    if let Err(e) = transfer
        .file
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("incoming transfer file is closed"))
        .and_then(|file| file.flush().map_err(anyhow::Error::from))
    {
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
        cleanup_incoming_transfer(task_roots, transfer);
        return Err(e);
    }
    transfer.timing.flush_ms += elapsed_ms(flush_start);
    transfer.file.take();
    if let Err(error) = transfer.mutation_guard.validate() {
        cleanup_incoming_transfer(task_roots, transfer);
        return Err(error);
    }
    if let Err(e) = ensure_safe_for_mutation(
        &task_roots
            .root_for(task_id)
            .ok_or_else(|| anyhow::anyhow!("task root not registered"))?,
        &transfer.final_path,
    ) {
        cleanup_incoming_transfer(task_roots, transfer);
        return Err(e);
    }
    if let Err(e) = ensure_receive_target_precondition(
        task_roots,
        task_id,
        relative_path,
        &transfer.final_path,
        transfer.expected_target_hash.as_deref(),
    ) {
        log_v2_receive_timing_summary(
            &transfer.transfer_id,
            task_id,
            relative_path,
            transfer.total_bytes,
            elapsed_ms(transfer.start_time),
            &transfer.timing,
            false,
            Some("v2 target changed"),
        );
        cleanup_incoming_transfer(task_roots, transfer);
        return Err(e);
    }
    let rename_start = Instant::now();
    if let Err(e) = commit_received_partial(
        task_roots,
        task_id,
        relative_path,
        &transfer.partial_path,
        &transfer.final_path,
        file_hash,
        transfer.expected_target_hash.as_deref(),
        ReceivedChangeKind::File,
    ) {
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
        cleanup_incoming_transfer(task_roots, transfer);
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

    task_roots.release_incoming_lease(&transfer);
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
    let _progress_guard = server_transfer_progress_guard(
        task_roots,
        task_id,
        relative_path,
        "serve",
        total_bytes,
        "v2_binary",
    );
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
            expected_target_hash: None,
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
    let _progress_guard = server_transfer_progress_guard(
        task_roots,
        task_id,
        relative_path,
        "serve",
        total_bytes,
        "v1_json",
    );

    writer
        .write_all(&encode_message(&SyncMessage::FileChunkStart {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
            file_hash: file_hash.clone(),
            total_bytes,
            expected_target_hash: None,
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
    resolution_id: Option<&str>,
) -> Result<ConflictApplyOutcome> {
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
    if let (Some(resolution_id), Some(db_path)) = (resolution_id, task_roots.state_db_path()) {
        let conn = db::open_db(&db_path)?;
        db::migrate(&conn)?;
        let existing = conn
            .query_row(
                "SELECT conflict_path, primary_hash, secondary_hash
                 WHERE resolution_id = ?1 AND state IN ('RemoteApplied', 'LocalCommitted')",
                params![resolution_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((Some(applied_path), primary_hash, Some(secondary_hash))) = existing {
            cleanup_conflict_staging(task_roots, task_id, staged_relative_path);
            return Ok(ConflictApplyOutcome {
                applied_path,
                primary_hash,
                secondary_hash,
            });
        }
        conn.execute(
            "INSERT OR IGNORE INTO conflict_resolution_journal
             (resolution_id, task_id, relative_path, mode, state, created_unix_ms, updated_unix_ms)
             VALUES (?1, ?2, ?3, ?4, 'Prepared', ?5, ?5)",
            params![resolution_id, task_id, relative_path, mode, now_ms()],
        )?;
    }
    if !staged.is_file() {
        anyhow::bail!("staged conflict file missing");
    }

    let now = now_ms();
    let staged_hash = crate::core::scanner::hash_file(&staged)?;
    let original_target = safe_join(&root, relative_path)?;
    let primary_hash = original_target
        .is_file()
        .then(|| crate::core::scanner::hash_file(&original_target))
        .transpose()?;
    let applied_relative_path = if mode == "overwrite" {
        let target = safe_join(&root, relative_path)?;
        create_safe_parent_dirs(&root, &target)?;
        commit_received_partial(
            task_roots,
            task_id,
            relative_path,
            &staged,
            &target,
            &staged_hash,
            None,
            ReceivedChangeKind::ConflictApply,
        )?;
        relative_path.to_string()
    } else {
        let conflict_relative_path =
            crate::core::conflict::conflict_filename(relative_path, "Secondary", now, |name| {
                root.join(name).exists()
            });
        let target = safe_join(&root, &conflict_relative_path)?;
        create_safe_parent_dirs(&root, &target)?;
        commit_received_partial(
            task_roots,
            task_id,
            &conflict_relative_path,
            &staged,
            &target,
            &staged_hash,
            Some(""),
            ReceivedChangeKind::ConflictApply,
        )?;
        conflict_relative_path
    };

    if let (Some(resolution_id), Some(db_path)) = (resolution_id, task_roots.state_db_path()) {
        let conn = db::open_db(&db_path)?;
        conn.execute(
            "UPDATE conflict_resolution_journal
             SET conflict_path = ?2, secondary_hash = ?3, primary_hash = ?4,
                 state = 'RemoteApplied', updated_unix_ms = ?5
             WHERE resolution_id = ?1",
            params![
                resolution_id,
                applied_relative_path,
                staged_hash,
                primary_hash,
                now_ms()
            ],
        )?;
    }
    cleanup_conflict_staging(task_roots, task_id, staged_relative_path);
    Ok(ConflictApplyOutcome {
        applied_path: applied_relative_path,
        primary_hash,
        secondary_hash: staged_hash,
    })
}

struct ConflictApplyOutcome {
    applied_path: String,
    primary_hash: Option<String>,
    secondary_hash: String,
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
        anyhow::bail!("TargetPreconditionFailed: legacy delete lacks target state");
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
    use std::sync::{Arc, Mutex};
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
    fn task_root_authorization_requires_matching_peer() {
        let dir = TempDir::new().unwrap();
        let registry = TaskRootRegistry::new();
        registry
            .register_for_peer("task", dir.path(), "peer-a")
            .unwrap();

        assert!(registry.authorize_task_access("task", "peer-a").is_ok());
        assert!(registry.authorize_task_access("task", "peer-b").is_err());
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

    #[test]
    fn legacy_transfer_cannot_overwrite_existing_target() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("document.txt"), b"primary").unwrap();
        let registry = TaskRootRegistry::new();
        registry.register("task", dir.path()).unwrap();
        let data = b"secondary";

        let error = write_incoming_file(
            &registry,
            "task",
            "document.txt",
            &blake3::hash(data).to_hex().to_string(),
            data.len() as u64,
            data,
        )
        .unwrap_err();

        assert!(error.to_string().contains("TargetPreconditionFailed"));
        assert_eq!(
            std::fs::read(dir.path().join("document.txt")).unwrap(),
            b"primary"
        );
    }

    #[test]
    fn failed_receive_does_not_notify_the_ui() {
        let dir = TempDir::new().unwrap();
        let registry = TaskRootRegistry::new();
        registry.register("task", dir.path()).unwrap();
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let captured = notifications.clone();
        registry.set_receive_commit_notifier(move |task_id, kind| {
            captured.lock().unwrap().push((task_id, kind));
        });

        let error = write_incoming_file(&registry, "task", "failed.txt", "wrong-hash", 4, b"data")
            .unwrap_err();

        assert!(error.to_string().contains("file hash mismatch"));
        assert!(notifications.lock().unwrap().is_empty());
    }

    #[test]
    fn transfer_activity_is_persisted_and_notified_before_receive_commit() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let db_path = dir.path().join("state.db");
        let task_id = Uuid::new_v4();
        let conn = db::open_db(&db_path).unwrap();
        db::migrate(&conn).unwrap();
        repository::SyncTaskRepository::new(&conn)
            .insert(&crate::core::model::SyncTask {
                id: task_id,
                name: "activity".to_string(),
                primary_device_id: "primary".to_string(),
                secondary_device_id: "secondary".to_string(),
                local_path: root.to_string_lossy().to_string(),
                remote_path: root.to_string_lossy().to_string(),
                local_role: crate::core::model::DeviceRole::Secondary,
                enabled: true,
                created_unix_ms: 1,
                updated_unix_ms: 1,
                last_transfer_activity_unix_ms: 0,
            })
            .unwrap();

        let registry = TaskRootRegistry::new();
        registry.set_state_db_path(&db_path).unwrap();
        let notified = Arc::new(Mutex::new(Vec::new()));
        let captured = notified.clone();
        registry.set_transfer_activity_notifier(move |id| captured.lock().unwrap().push(id));

        record_task_transfer_activity(&registry, &task_id.to_string());

        assert_eq!(notified.lock().unwrap().as_slice(), &[task_id.to_string()]);
        let recorded = repository::SyncTaskRepository::new(&conn)
            .get(&task_id)
            .unwrap()
            .unwrap();
        assert!(recorded.last_transfer_activity_unix_ms > 0);
    }

    #[test]
    fn legacy_delete_without_precondition_rejects_existing_target() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("document.txt"), b"primary").unwrap();
        let registry = TaskRootRegistry::new();
        registry.register("task", dir.path()).unwrap();

        let error = move_incoming_delete_to_history(
            &registry,
            "task",
            "document.txt",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap_err();

        assert!(error.to_string().contains("TargetPreconditionFailed"));
        assert!(dir.path().join("document.txt").exists());
    }

    #[test]
    fn incoming_transfer_lease_is_exclusive_and_connection_scoped() {
        let dir = TempDir::new().unwrap();
        let registry = TaskRootRegistry::new();
        registry.register("task", dir.path()).unwrap();

        start_incoming_v2(&registry, "task", "same.txt", 4, Some(""), "conn-a").unwrap();
        let error =
            start_incoming_v2(&registry, "task", "same.txt", 4, Some(""), "conn-b").unwrap_err();
        assert!(error.to_string().contains("TransferAlreadyInProgress"));

        let wrong_connection = registry
            .cancel_incoming_transfer_for_connection("conn-b", "task", "same.txt")
            .unwrap_err();
        assert!(wrong_connection.to_string().contains("not owned"));
        assert_eq!(registry.incoming.lock().unwrap().len(), 1);

        registry
            .cancel_incoming_transfer_for_connection("conn-a", "task", "same.txt")
            .unwrap();
        assert!(registry.incoming.lock().unwrap().is_empty());
        assert!(registry.incoming_leases.lock().unwrap().is_empty());
    }

    #[test]
    fn disconnect_cleans_only_its_unique_partial_and_lease() {
        let dir = TempDir::new().unwrap();
        let registry = TaskRootRegistry::new();
        registry.register("task", dir.path()).unwrap();
        start_incoming_v2(&registry, "task", "one.txt", 3, Some(""), "conn-a").unwrap();
        start_incoming_v2(&registry, "task", "two.txt", 3, Some(""), "conn-b").unwrap();
        let partials = registry
            .incoming
            .lock()
            .unwrap()
            .values()
            .map(|transfer| transfer.partial_path.clone())
            .collect::<Vec<_>>();
        assert_ne!(partials[0], partials[1]);
        assert!(partials.iter().all(|path| path.exists()));

        registry.cancel_incoming_for_connection("conn-a");
        let incoming = registry.incoming.lock().unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming.values().next().unwrap().connection_id, "conn-b");
        drop(incoming);
        assert_eq!(partials.iter().filter(|path| path.exists()).count(), 1);

        registry.cancel_incoming_for_connection("conn-b");
        assert!(partials.iter().all(|path| !path.exists()));
        assert!(registry.incoming_leases.lock().unwrap().is_empty());
    }

    #[test]
    fn oversized_chunk_cleans_partial_and_lease() {
        let dir = TempDir::new().unwrap();
        let registry = TaskRootRegistry::new();
        registry.register("task", dir.path()).unwrap();
        start_incoming_v2(&registry, "task", "large.txt", 2, Some(""), "conn-a").unwrap();
        let partial = registry
            .incoming
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .partial_path
            .clone();

        let error =
            append_incoming_chunk(&registry, "conn-a", "task", "large.txt", 0, b"too large")
                .unwrap_err();
        assert!(error.to_string().contains("exceeded expected size"));
        assert!(!partial.exists());
        assert!(registry.incoming.lock().unwrap().is_empty());
        assert!(registry.incoming_leases.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn finish_rejects_parent_directory_replaced_after_transfer_start() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("parent")).unwrap();
        let registry = TaskRootRegistry::new();
        registry.register("task", dir.path()).unwrap();
        let data = b"safe";
        start_incoming_v2(
            &registry,
            "task",
            "parent/file.txt",
            data.len() as u64,
            Some(""),
            "conn-a",
        )
        .unwrap();
        append_incoming_chunk(&registry, "conn-a", "task", "parent/file.txt", 0, data).unwrap();
        std::fs::rename(dir.path().join("parent"), dir.path().join("old-parent")).unwrap();
        std::fs::create_dir(dir.path().join("parent")).unwrap();

        let error = finish_incoming_v2(
            &registry,
            "conn-a",
            "task",
            "parent/file.txt",
            &blake3::hash(data).to_hex().to_string(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("identity changed"));
        assert!(!dir.path().join("parent/file.txt").exists());
        assert!(registry.incoming_leases.lock().unwrap().is_empty());
    }

    #[test]
    fn startup_recovery_finishes_metadata_after_filesystem_commit() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let db_path = dir.path().join("state.db");
        let task_id = Uuid::new_v4();
        let relative_path = "recovered.txt";
        let final_path = root.join(relative_path);
        std::fs::write(&final_path, b"committed bytes").unwrap();
        let incoming_hash = blake3::hash(b"committed bytes").to_hex().to_string();
        let stale_partial = root.join("stale.partial");
        std::fs::write(&stale_partial, b"duplicate").unwrap();

        let conn = db::open_db(&db_path).unwrap();
        db::migrate(&conn).unwrap();
        repository::SyncTaskRepository::new(&conn)
            .insert(&crate::core::model::SyncTask {
                id: task_id,
                name: "recovery".to_string(),
                primary_device_id: "primary".to_string(),
                secondary_device_id: "secondary".to_string(),
                local_path: root.to_string_lossy().to_string(),
                remote_path: root.to_string_lossy().to_string(),
                local_role: crate::core::model::DeviceRole::Secondary,
                enabled: true,
                created_unix_ms: 1,
                updated_unix_ms: 1,
                last_transfer_activity_unix_ms: 0,
            })
            .unwrap();
        conn.execute(
            "INSERT INTO transfer_commit_journal
             (commit_id, task_id, relative_path, expected_target_hash, incoming_hash,
              partial_path, history_path, state, created_unix_ms, updated_unix_ms)
             VALUES (?1, ?2, ?3, '', ?4, ?5, NULL, 'FilesystemCommitted', 1, 1)",
            params![
                Uuid::new_v4().to_string(),
                task_id.to_string(),
                relative_path,
                incoming_hash,
                stale_partial.to_string_lossy().to_string()
            ],
        )
        .unwrap();
        drop(conn);

        let registry = TaskRootRegistry::new();
        registry.register(task_id.to_string(), &root).unwrap();
        registry.set_state_db_path(&db_path).unwrap();
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let captured = notifications.clone();
        registry.set_receive_commit_notifier(move |task_id, kind| {
            captured.lock().unwrap().push((task_id, kind));
        });
        assert_eq!(registry.recover_incomplete_commits().unwrap(), 1);
        assert!(!stale_partial.exists());
        assert_eq!(
            notifications.lock().unwrap().as_slice(),
            &[(task_id.to_string(), ReceivedChangeKind::File)]
        );

        let conn = db::open_db(&db_path).unwrap();
        let journal_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM transfer_commit_journal", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(journal_count, 0);
        assert_eq!(
            repository::FileSnapshotRepository::new(&conn)
                .get(&task_id, relative_path)
                .unwrap()
                .unwrap()
                .blake3_hash
                .as_deref(),
            Some(incoming_hash.as_str())
        );
        assert_eq!(
            repository::SyncBaselineRepository::new(&conn)
                .get(&task_id, relative_path)
                .unwrap()
                .unwrap()
                .primary_hash
                .as_deref(),
            Some(incoming_hash.as_str())
        );
    }
}

struct RecoverableCommit {
    commit_id: String,
    history_path: Option<String>,
    created_unix_ms: i64,
}

fn recoverable_commit(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    incoming_hash: &str,
) -> Result<Option<RecoverableCommit>> {
    let Some(db_path) = task_roots.state_db_path() else {
        return Ok(None);
    };
    let conn = db::open_db(&db_path)?;
    db::migrate(&conn)?;
    let mut stmt = conn.prepare(
        "SELECT commit_id, history_path, created_unix_ms
         FROM transfer_commit_journal
         WHERE task_id = ?1 AND relative_path = ?2 AND incoming_hash = ?3
           AND state IN ('Prepared', 'FilesystemCommitted')
         ORDER BY updated_unix_ms DESC LIMIT 1",
    )?;
    let mut rows = stmt.query(params![task_id, relative_path, incoming_hash])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(RecoverableCommit {
        commit_id: row.get(0)?,
        history_path: row.get(1)?,
        created_unix_ms: row.get(2)?,
    }))
}

fn ensure_receive_target_precondition(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    target_path: &Path,
    expected_target_hash: Option<&str>,
) -> Result<()> {
    match connection::ensure_target_precondition(target_path, expected_target_hash) {
        Ok(()) => Ok(()),
        Err(original_error) => {
            let Some(db_path) = task_roots.state_db_path() else {
                return Err(original_error);
            };
            if !target_path.is_file() {
                return Err(original_error);
            }
            let current_hash = crate::core::scanner::hash_file(target_path)?;
            let conn = db::open_db(&db_path)?;
            db::migrate(&conn)?;
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM transfer_commit_journal
                 WHERE task_id = ?1 AND relative_path = ?2 AND incoming_hash = ?3
                   AND state IN ('Prepared', 'FilesystemCommitted')",
                params![task_id, relative_path, current_hash],
                |row| row.get(0),
            )?;
            if count > 0 {
                Ok(())
            } else {
                Err(original_error)
            }
        }
    }
}

fn commit_received_partial(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    partial_path: &Path,
    final_path: &Path,
    incoming_hash: &str,
    expected_target_hash: Option<&str>,
    change_kind: ReceivedChangeKind,
) -> Result<()> {
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let root_handle = TaskRootHandle::new(&root)?;
    let mutation_guard = root_handle.prepare_mutation(final_path)?;
    ensure_safe_for_mutation(&root, final_path)?;

    if let Some(recovery) = recoverable_commit(task_roots, task_id, relative_path, incoming_hash)? {
        let target_matches =
            final_path.is_file() && crate::core::scanner::hash_file(final_path)? == incoming_hash;
        if target_matches {
            if let Err(error) = std::fs::remove_file(partial_path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        event = "PartialCleanupFailed",
                        partial_path = %partial_path.display(),
                        error = %error,
                        "failed to remove duplicate recovery partial"
                    );
                }
            }
            let history_entry = recovery.history_path.as_deref().and_then(|stored_path| {
                let metadata = std::fs::metadata(stored_path).ok()?;
                Some(HistoryEntry {
                    id: Uuid::parse_str(&recovery.commit_id).unwrap_or_else(|_| Uuid::new_v4()),
                    task_id: Uuid::parse_str(task_id).ok()?,
                    original_relative_path: relative_path.to_string(),
                    stored_path: stored_path.to_string(),
                    reason: crate::core::model::HistoryReason::Overwritten,
                    created_unix_ms: recovery.created_unix_ms,
                    size: metadata.len() as i64,
                })
            });
            return record_received_file_commit(
                task_roots,
                task_id,
                relative_path,
                final_path,
                incoming_hash,
                history_entry.as_ref(),
                Some(&recovery.commit_id),
                change_kind,
            );
        }
    }

    let commit_uuid = Uuid::new_v4();
    let commit_id = commit_uuid.to_string();
    let created_unix_ms = now_ms();
    let history_entry = if final_path.exists() {
        let history = HistoryStore::new(&root);
        history.check_storage_blocked(created_unix_ms)?;
        let mut entry =
            history.backup_to_overwritten(final_path, relative_path, created_unix_ms)?;
        entry.id = commit_uuid;
        entry.task_id = Uuid::parse_str(task_id)?;
        Some(entry)
    } else {
        None
    };

    let db_conn = if let Some(db_path) = task_roots.state_db_path() {
        let conn = db::open_db(&db_path)?;
        db::migrate(&conn)?;
        conn.execute(
            "INSERT INTO transfer_commit_journal
             (commit_id, task_id, relative_path, expected_target_hash, incoming_hash,
              partial_path, history_path, state, created_unix_ms, updated_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'Prepared', ?8, ?8)",
            params![
                commit_id,
                task_id,
                relative_path,
                expected_target_hash,
                incoming_hash,
                partial_path.to_string_lossy().to_string(),
                history_entry
                    .as_ref()
                    .map(|entry| entry.stored_path.as_str()),
                created_unix_ms,
            ],
        )?;
        Some(conn)
    } else {
        None
    };

    mutation_guard.validate()?;
    ensure_safe_for_mutation(&root, final_path)?;
    if let Err(error) = connection::replace_partial_file(partial_path, final_path) {
        if let Some(conn) = &db_conn {
            let _ = conn.execute(
                "DELETE FROM transfer_commit_journal WHERE commit_id = ?1 AND state = 'Prepared'",
                params![commit_id],
            );
        }
        return Err(error);
    }

    if let Some(conn) = &db_conn {
        conn.execute(
            "UPDATE transfer_commit_journal
             SET state = 'FilesystemCommitted', updated_unix_ms = ?2
             WHERE commit_id = ?1",
            params![commit_id, now_ms()],
        )?;
    }
    record_received_file_commit(
        task_roots,
        task_id,
        relative_path,
        final_path,
        incoming_hash,
        history_entry.as_ref(),
        db_conn.as_ref().map(|_| commit_id.as_str()),
        change_kind,
    )?;
    Ok(())
}

fn record_received_file_commit(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    path: &Path,
    file_hash: &str,
    history_entry: Option<&HistoryEntry>,
    commit_id: Option<&str>,
    change_kind: ReceivedChangeKind,
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
        let tx = conn.unchecked_transaction()?;
        repository::FileSnapshotRepository::new(&tx).upsert(&snapshot)?;
        repository::SyncBaselineRepository::new(&tx).upsert(&SyncBaseline {
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
        repository::LogRepository::new(&tx).insert(&LogEntry {
            id: None,
            level: LogLevel::Info,
            task_id: Some(task_id),
            relative_path: Some(relative_path.to_string()),
            message: "received file from peer".to_string(),
            created_unix_ms: now_ms(),
        })?;
        if let Some(entry) = history_entry {
            tx.execute(
                "INSERT OR IGNORE INTO history_entries
                 (id, task_id, original_relative_path, stored_path, reason, created_unix_ms, size)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    entry.id.to_string(),
                    entry.task_id.to_string(),
                    entry.original_relative_path,
                    entry.stored_path,
                    format!("{:?}", entry.reason),
                    entry.created_unix_ms,
                    entry.size,
                ],
            )?;
        }
        if let Some(commit_id) = commit_id {
            tx.execute(
                "UPDATE transfer_commit_journal
                 SET state = 'MetadataCommitted', updated_unix_ms = ?2
                 WHERE commit_id = ?1",
                params![commit_id, now_ms()],
            )?;
        }
        tx.commit()?;
        Ok(())
    })?;
    if let Some(commit_id) = commit_id {
        conn.execute(
            "DELETE FROM transfer_commit_journal WHERE commit_id = ?1",
            params![commit_id],
        )?;
    }
    task_roots.notify_receive_commit(&task_id.to_string(), change_kind);
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
    task_roots.notify_receive_commit(&task_id.to_string(), ReceivedChangeKind::Directory);
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
    task_roots.notify_receive_commit(&task_id.to_string(), ReceivedChangeKind::Delete);
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
    task_roots.notify_receive_commit(&task_id.to_string(), ReceivedChangeKind::Delete);
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
