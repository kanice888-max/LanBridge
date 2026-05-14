use anyhow::Result;
use ed25519_dalek::Signature;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use uuid::Uuid;

use crate::core::model::{EntryKind, FileSnapshot, HashStatus, LogEntry, LogLevel, SyncBaseline};
use crate::pairing::{DeviceIdentity, PublicIdentity};
use crate::state::{db, repository};
use crate::transport::connection::auth_payload;
use crate::transport::protocol::RemoteFileState;

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

#[derive(Debug, Clone)]
struct IncomingTransfer {
    partial_path: PathBuf,
    final_path: PathBuf,
    file_hash: String,
    total_bytes: u64,
    written_bytes: u64,
    hasher: blake3::Hasher,
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
        let registry = Self::default();
        registry.set_auto_accept_task_invites(true);
        registry
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

    fn root_for(&self, task_id: &str) -> Option<PathBuf> {
        let roots = self.roots.lock().unwrap();
        roots.get(task_id).cloned()
    }

    pub fn register_trusted_peer(&self, identity: PublicIdentity) {
        let mut peers = self.trusted_peers.lock().unwrap();
        peers.insert(identity.device_id.clone(), identity);
    }

    fn trusted_peer(&self, device_id: &str) -> Option<PublicIdentity> {
        let peers = self.trusted_peers.lock().unwrap();
        peers.get(device_id).cloned()
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
        validate_invite_local_path(&local_path)?;
        let task_id = {
            let invites = self.task_invites.lock().unwrap();
            invites
                .get(invite_id)
                .ok_or_else(|| anyhow::anyhow!("task invite not found"))?
                .task_id
                .clone()
        };
        self.register(&task_id, &local_path)?;

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
            let rt = tokio::runtime::Builder::new_current_thread()
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
                    protocol::SyncMessage::Ping => {
                        let pong = protocol::encode_message(&protocol::SyncMessage::Pong)?;
                        writer.write_all(&pong).await?;
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
                    protocol::SyncMessage::FileChunkEnd {
                        task_id,
                        relative_path,
                    } => {
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                finish_incoming_chunked_file(&task_roots, &task_id, &relative_path)
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
                    protocol::SyncMessage::FileDelete {
                        task_id,
                        relative_path,
                    } => {
                        let ack =
                            match require_authenticated(&authenticated_device_id).and_then(|_| {
                                move_incoming_delete_to_history(
                                    &task_roots,
                                    &task_id,
                                    &relative_path,
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
                    | protocol::SyncMessage::ScanResponse { .. }
                    | protocol::SyncMessage::Pong => {
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

fn start_incoming_chunked_file(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    file_hash: &str,
    total_bytes: u64,
) -> Result<()> {
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let final_path = safe_join(&root, relative_path)?;
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial_path = partial_path(&final_path);
    std::fs::write(&partial_path, [])?;

    let transfer = IncomingTransfer {
        partial_path,
        final_path,
        file_hash: file_hash.to_string(),
        total_bytes,
        written_bytes: 0,
        hasher: blake3::Hasher::new(),
    };
    let mut incoming = task_roots.incoming.lock().unwrap();
    incoming.insert(transfer_key(task_id, relative_path), transfer);
    Ok(())
}

fn append_incoming_chunk(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    offset: u64,
    data: &[u8],
) -> Result<()> {
    let key = transfer_key(task_id, relative_path);
    let mut incoming = task_roots.incoming.lock().unwrap();
    let transfer = incoming
        .get_mut(&key)
        .ok_or_else(|| anyhow::anyhow!("chunked transfer not started"))?;
    if transfer.written_bytes != offset {
        anyhow::bail!("unexpected chunk offset");
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&transfer.partial_path)?;
    file.write_all(data)?;
    transfer.hasher.update(data);
    transfer.written_bytes += data.len() as u64;
    if transfer.written_bytes > transfer.total_bytes {
        anyhow::bail!("chunked transfer exceeded expected size");
    }
    Ok(())
}

fn finish_incoming_chunked_file(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
) -> Result<()> {
    let key = transfer_key(task_id, relative_path);
    let transfer = {
        let mut incoming = task_roots.incoming.lock().unwrap();
        incoming
            .remove(&key)
            .ok_or_else(|| anyhow::anyhow!("chunked transfer not started"))?
    };
    if transfer.written_bytes != transfer.total_bytes {
        anyhow::bail!("chunked transfer size mismatch");
    }
    let actual_hash = transfer.hasher.finalize().to_hex().to_string();
    if actual_hash != transfer.file_hash {
        let _ = std::fs::remove_file(&transfer.partial_path);
        anyhow::bail!("file hash mismatch");
    }
    std::fs::rename(&transfer.partial_path, &transfer.final_path)?;
    record_received_file(
        task_roots,
        task_id,
        relative_path,
        &transfer.final_path,
        &transfer.file_hash,
    )?;
    Ok(())
}

async fn send_file_download(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
) -> Result<()> {
    const CHUNK_SIZE: usize = 64 * 1024;
    use crate::transport::protocol::{encode_message, SyncMessage};

    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let source = safe_join(&root, relative_path)?;
    if !source.is_file() {
        anyhow::bail!("requested file not found");
    }

    let metadata = std::fs::metadata(&source)?;
    let file_hash = crate::core::scanner::hash_file(&source)?;
    writer
        .write_all(&encode_message(&SyncMessage::FileChunkStart {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
            file_hash,
            total_bytes: metadata.len(),
        })?)
        .await?;

    let mut file = std::fs::File::open(&source)?;
    let mut offset = 0u64;
    loop {
        let mut buf = vec![0u8; CHUNK_SIZE];
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        buf.truncate(read);
        writer
            .write_all(&encode_message(&SyncMessage::FileChunk {
                task_id: task_id.to_string(),
                relative_path: relative_path.to_string(),
                offset,
                data: buf,
            })?)
            .await?;
        offset += read as u64;
    }

    writer
        .write_all(&encode_message(&SyncMessage::FileChunkEnd {
            task_id: task_id.to_string(),
            relative_path: relative_path.to_string(),
        })?)
        .await?;
    Ok(())
}

fn scan_task_root(task_roots: &TaskRootRegistry, task_id: &str) -> Result<Vec<RemoteFileState>> {
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }

    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.components().any(
            |component| matches!(component, Component::Normal(name) if name == ".lanbridge-history"),
        ) {
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
        files.push(RemoteFileState {
            relative_path,
            blake3_hash: Some(crate::core::scanner::hash_file(path)?),
            size: metadata.len() as i64,
            modified_unix_ms,
        });
    }
    Ok(files)
}

fn move_incoming_delete_to_history(
    task_roots: &TaskRootRegistry,
    task_id: &str,
    relative_path: &str,
) -> Result<()> {
    let root = task_roots
        .root_for(task_id)
        .ok_or_else(|| anyhow::anyhow!("task root not registered"))?;
    let target = safe_join(&root, relative_path)?;
    if !target.exists() {
        return Ok(());
    }

    let history_path = safe_join(&root.join(".lanbridge-history").join("trash"), relative_path)?;
    if let Some(parent) = history_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(target, history_path)?;
    record_received_delete(task_roots, task_id, relative_path)?;
    Ok(())
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
    repository::FileSnapshotRepository::new(&conn).upsert(&snapshot)?;
    repository::SyncBaselineRepository::new(&conn).upsert(&SyncBaseline {
        task_id,
        relative_path: relative_path.to_string(),
        primary_hash: Some(file_hash.to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_size: metadata.len() as i64,
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
}

fn record_received_delete(
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
    repository::FileSnapshotRepository::new(&conn).mark_deleted(&task_id, relative_path)?;
    repository::LogRepository::new(&conn).insert(&LogEntry {
        id: None,
        level: LogLevel::Info,
        task_id: Some(task_id),
        relative_path: Some(relative_path.to_string()),
        message: "received delete from peer".to_string(),
        created_unix_ms: now_ms(),
    })?;
    Ok(())
}

fn requester_address(peer_addr: SocketAddr, requester_port: u16) -> Option<String> {
    if requester_port == 0 {
        None
    } else {
        Some(format!("{}:{}", peer_addr.ip(), requester_port))
    }
}

fn safe_join(root: &Path, relative_path: &str) -> Result<PathBuf> {
    let path = Path::new(relative_path);
    let mut dest = root.to_path_buf();
    for component in path.components() {
        match component {
            Component::Normal(part) => dest.push(part),
            Component::CurDir => {}
            _ => anyhow::bail!("invalid relative path"),
        }
    }
    if dest == root {
        anyhow::bail!("empty relative path");
    }
    Ok(dest)
}

fn validate_invite_local_path(path: &Path) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("invite local path must exist");
    }
    if !path.is_dir() {
        anyhow::bail!("invite local path must be a directory");
    }

    let mut entries = std::fs::read_dir(path)?;
    let has_user_entries = entries.any(|entry| {
        entry
            .ok()
            .and_then(|entry| entry.file_name().into_string().ok())
            .map(|name| name != ".lanbridge-history")
            .unwrap_or(true)
    });
    if has_user_entries {
        anyhow::bail!("invite local path must be empty");
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
