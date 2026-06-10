pub mod pairing;

// Re-export all public items so main.rs can reference commands::* unchanged.
pub use pairing::*;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State, Window};
use uuid::Uuid;

use crate::app_state::{AppState, PendingOutgoingTaskInvite, SyncRunAdmission};
use crate::core::conflict;
use crate::core::executor;
use crate::core::model::*;
use crate::core::path_safety;
use crate::core::planner;
use crate::core::scanner;
use crate::history::store::HistoryStore;
use crate::state::repository;
use crate::transport::protocol::RemoteFileState;
use crate::transport::{connection, SyncMessage};

const MAX_NETWORK_ATTEMPTS: usize = 3;
const FILE_LIST_REFRESH_QUIET_MS: u64 = 800;
const FILE_LIST_METADATA_CHECK_INTERVAL: Duration = Duration::from_secs(10);
const PRIMARY_NON_EMPTY_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
struct NetworkActionResult {
    result: executor::ExecutionResult,
    transferred_hash: Option<String>,
}

#[derive(Debug, Clone)]
struct PullActionResult {
    result: executor::ExecutionResult,
    transferred_hash: Option<String>,
}

#[derive(Debug, Clone)]
struct DeleteExpectation {
    expected_kind: EntryKind,
    expected_hash: Option<String>,
    expected_hash_status: HashStatus,
    expected_size: i64,
    expected_modified_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderInspection {
    pub exists: bool,
    pub is_dir: bool,
    pub is_empty: bool,
    pub total_size: u64,
    pub file_count: u64,
    pub dir_count: u64,
    pub over_limit: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub enum DeleteDestination {
    LanBridgeHistory,
    SystemTrash,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteEntryResult {
    pub relative_path: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub enum ImportCollisionPolicy {
    Cancel,
    KeepBoth,
    Overwrite,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportEntryResult {
    pub source_path: String,
    pub relative_path: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportTaskEntriesResult {
    pub imported: Vec<ImportEntryResult>,
    pub conflicts: Vec<ImportEntryResult>,
    pub failed: Vec<ImportEntryResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowCursorPosition {
    pub x: f64,
    pub y: f64,
}

fn network_result(result: executor::ExecutionResult) -> NetworkActionResult {
    NetworkActionResult {
        result,
        transferred_hash: None,
    }
}

fn pull_result(result: executor::ExecutionResult) -> PullActionResult {
    PullActionResult {
        result,
        transferred_hash: None,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn deferred_transfer_key(relative_path: &str, direction: &str) -> String {
    format!("{}\n{}", relative_path, direction)
}

fn deferred_transfer_set(records: &[DeferredTransferRecord], task_id: Uuid) -> HashSet<String> {
    records
        .iter()
        .filter(|record| record.task_id == task_id)
        .map(|record| deferred_transfer_key(&record.relative_path, &record.direction))
        .collect()
}

fn is_path_deferred(deferred: &HashSet<String>, relative_path: &str, direction: &str) -> bool {
    deferred.contains(&deferred_transfer_key(relative_path, direction))
}

fn is_retryable_network_error(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_lowercase();
    if message.contains("authentication rejected")
        || message.contains("not trusted")
        || message.contains("conflict")
        || message.contains("hash mismatch")
        || message.contains("invalid path")
        || message.contains("permission denied")
        || message.contains("transfer cancelled")
        || message.contains("transfer deferred")
        || message.contains("already exists")
        || message.contains("changed since last sync")
        || message.contains("remote file changed")
    {
        return false;
    }

    message.contains("timed out")
        || message.contains("timeout")
        || message.contains("connection refused")
        || message.contains("connection reset")
        || message.contains("connection aborted")
        || message.contains("broken pipe")
        || message.contains("unexpected eof")
        || message.contains("peer disconnected")
        || message.contains("failed to connect")
        || message.contains("network")
        || message.contains("temporarily")
        || message.contains("still changing")
        || message.contains("source file changed while preparing transfer")
}

async fn sleep_before_retry(attempt: usize) {
    let base_ms = 700u64 * 2u64.pow(attempt as u32);
    let jitter_ms = base_ms / 2;
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
        % (jitter_ms + 1);
    tokio::time::sleep(std::time::Duration::from_millis(base_ms + jitter)).await;
}

#[tauri::command]
pub fn inspect_task_folder(
    state: State<'_, AppState>,
    path: String,
    role: String,
) -> Result<FolderInspection, String> {
    let role = parse_device_role(&role)?;
    let inspection = inspect_folder_for_role(&state, &path, role)?;
    Ok(inspection)
}

fn validate_task_folder_for_role(
    state: &State<'_, AppState>,
    path: &str,
    role: DeviceRole,
) -> Result<FolderInspection, String> {
    let inspection = inspect_folder_for_role(state, path, role)?;
    if !inspection.exists {
        return Err("文件夹不存在".to_string());
    }
    if !inspection.is_dir {
        return Err("请选择文件夹".to_string());
    }
    match role {
        DeviceRole::Primary if inspection.over_limit => {
            Err("文件夹超过 2GB，请选择更小的文件夹".to_string())
        }
        DeviceRole::Secondary if !inspection.is_empty => Err("请选择一个空文件夹".to_string()),
        _ => Ok(inspection),
    }
}

fn inspect_folder_for_role(
    state: &State<'_, AppState>,
    path: &str,
    _role: DeviceRole,
) -> Result<FolderInspection, String> {
    let raw_path = Path::new(path);
    if !raw_path.exists() {
        return Ok(FolderInspection {
            exists: false,
            is_dir: false,
            is_empty: true,
            total_size: 0,
            file_count: 0,
            dir_count: 0,
            over_limit: false,
        });
    }
    if !raw_path.is_dir() {
        return Ok(FolderInspection {
            exists: true,
            is_dir: false,
            is_empty: true,
            total_size: 0,
            file_count: 0,
            dir_count: 0,
            over_limit: false,
        });
    }
    let root = state
        .platform
        .validate_sync_root(raw_path)
        .map_err(|e| e.to_string())?;
    let mut total_size = 0u64;
    let mut file_count = 0u64;
    let mut dir_count = 0u64;

    let mut walker = walkdir::WalkDir::new(&root)
        .min_depth(1)
        .follow_links(false)
        .into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type();
        let is_dir = file_type.is_dir();
        let name = entry.file_name().to_string_lossy();
        if matches!(
            state.platform.classify_ignored_entry(&name, is_dir),
            crate::platform::traits::IgnoreDecision::Ignored(_)
        ) {
            if is_dir {
                walker.skip_current_dir();
            }
            continue;
        }
        if is_dir {
            dir_count += 1;
        } else if file_type.is_file() {
            file_count += 1;
            total_size = total_size.saturating_add(entry.metadata().map(|m| m.len()).unwrap_or(0));
        }
        if total_size > PRIMARY_NON_EMPTY_LIMIT_BYTES {
            break;
        }
    }

    Ok(FolderInspection {
        exists: true,
        is_dir: true,
        is_empty: file_count == 0 && dir_count == 0,
        total_size,
        file_count,
        dir_count,
        over_limit: total_size > PRIMARY_NON_EMPTY_LIMIT_BYTES,
    })
}
// ─── Sync Tasks ───

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTaskRequest {
    pub name: String,
    pub local_path: String,
    pub remote_path: Option<String>,
    pub peer_device_id: String,
    pub local_role: String, // "Primary" or "Secondary"
}

#[derive(Debug, Clone, Deserialize)]
pub struct SendTaskInviteRequest {
    pub name: String,
    pub local_path: String,
    pub peer_device_id: String,
    pub local_role: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskInviteProgress {
    pub invite_id: String,
    pub task_id: String,
    pub status: String,
    pub task: Option<SyncTask>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IncomingTaskInviteInfo {
    pub invite_id: String,
    pub task_id: String,
    pub task_name: String,
    pub requester_device_id: String,
    pub requester_address: Option<String>,
    pub requester_path: Option<String>,
    pub proposed_role: String,
    pub status: String,
    pub local_path: Option<String>,
    pub error: Option<String>,
    pub created_unix_ms: i64,
}

#[tauri::command]
pub async fn send_task_invite(
    state: State<'_, AppState>,
    request: SendTaskInviteRequest,
) -> Result<TaskInviteProgress, String> {
    let local_role = parse_device_role(&request.local_role)?;
    validate_task_folder_for_role(&state, &request.local_path, local_role)?;
    state
        .connections
        .get_pinned(&request.peer_device_id)
        .ok_or("peer device not pinned")?;
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let existing = repository::SyncTaskRepository::new(&db)
            .list_all()
            .map_err(|e| e.to_string())?;
        validate_no_overlapping_task_roots(&existing, &request.local_path)?;
    }

    let invite_id = Uuid::new_v4().to_string();
    let task_id = Uuid::new_v4();
    let proposed_role = match local_role {
        DeviceRole::Primary => "Secondary".to_string(),
        DeviceRole::Secondary => "Primary".to_string(),
    };
    let response = send_task_invite_to_peer(
        &state,
        &request.peer_device_id,
        invite_id.clone(),
        task_id.to_string(),
        request.name.clone(),
        Some(request.local_path.clone()),
        proposed_role,
    )
    .await?;

    let pending = PendingOutgoingTaskInvite {
        task_id,
        name: request.name,
        local_path: request.local_path,
        peer_device_id: request.peer_device_id,
        local_role,
    };

    match response {
        SyncMessage::TaskInvitePending { .. } => {
            state
                .pending_outgoing_invites
                .lock()
                .map_err(|e| e.to_string())?
                .insert(invite_id.clone(), pending.clone());
            let db = state.db.lock().map_err(|e| e.to_string())?;
            persist_pending_outgoing_invite(&db, &invite_id, &pending)?;
            Ok(TaskInviteProgress {
                invite_id,
                task_id: task_id.to_string(),
                status: "Pending".to_string(),
                task: None,
                error: None,
            })
        }
        SyncMessage::TaskInviteAck {
            success: true,
            remote_path: Some(remote_path),
            ..
        } => {
            let task = create_task_from_invite(&state, &pending, remote_path)?;
            Ok(TaskInviteProgress {
                invite_id,
                task_id: task_id.to_string(),
                status: "Accepted".to_string(),
                task: Some(task),
                error: None,
            })
        }
        SyncMessage::TaskInviteAck { error, .. } => Ok(TaskInviteProgress {
            invite_id,
            task_id: task_id.to_string(),
            status: "Rejected".to_string(),
            task: None,
            error,
        }),
        other => Err(format!("unexpected peer response: {:?}", other)),
    }
}

#[tauri::command]
pub async fn poll_task_invite(
    state: State<'_, AppState>,
    invite_id: String,
) -> Result<TaskInviteProgress, String> {
    let pending = state
        .pending_outgoing_invites
        .lock()
        .map_err(|e| e.to_string())?
        .get(&invite_id)
        .cloned();
    let pending = match pending {
        Some(pending) => pending,
        None => {
            let db = state.db.lock().map_err(|e| e.to_string())?;
            load_pending_outgoing_invite(&db, &invite_id)?.ok_or("task invite not found")?
        }
    };

    let status_request = SyncMessage::TaskInviteStatusRequest {
        invite_id: invite_id.clone(),
    };
    let response = match connection::send_authenticated_message_to_peer(
        &state.connections,
        &state.identity,
        &pending.peer_device_id,
        status_request.clone(),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => connection::send_message_to_peer(
            &state.connections,
            &pending.peer_device_id,
            status_request,
        )
        .await
        .map_err(|e| format!("task invitation status failed: {}", e))?,
    };

    match response {
        SyncMessage::TaskInviteStatus {
            status,
            remote_path,
            error,
            ..
        } if status == "Accepted" => {
            let Some(remote_path) = remote_path else {
                return Err("accepted task invite did not include remote path".to_string());
            };
            let task = create_task_from_invite(&state, &pending, remote_path)?;
            state
                .pending_outgoing_invites
                .lock()
                .map_err(|e| e.to_string())?
                .remove(&invite_id);
            let db = state.db.lock().map_err(|e| e.to_string())?;
            remove_pending_outgoing_invite(&db, &invite_id)?;
            Ok(TaskInviteProgress {
                invite_id,
                task_id: pending.task_id.to_string(),
                status,
                task: Some(task),
                error,
            })
        }
        SyncMessage::TaskInviteStatus { status, error, .. } => {
            if status == "Rejected" || status == "Missing" {
                state
                    .pending_outgoing_invites
                    .lock()
                    .map_err(|e| e.to_string())?
                    .remove(&invite_id);
                let db = state.db.lock().map_err(|e| e.to_string())?;
                remove_pending_outgoing_invite(&db, &invite_id)?;
            }
            Ok(TaskInviteProgress {
                invite_id,
                task_id: pending.task_id.to_string(),
                status,
                task: None,
                error,
            })
        }
        other => Err(format!("unexpected peer response: {:?}", other)),
    }
}

#[tauri::command]
pub fn list_task_invites(
    state: State<'_, AppState>,
) -> Result<Vec<IncomingTaskInviteInfo>, String> {
    let server = state._server.as_ref().ok_or("sync server is not running")?;
    Ok(server
        .list_task_invites()
        .into_iter()
        .map(|invite| IncomingTaskInviteInfo {
            invite_id: invite.invite_id,
            task_id: invite.task_id,
            task_name: invite.task_name,
            requester_device_id: invite.requester_device_id,
            requester_address: invite.requester_address,
            requester_path: invite.requester_path,
            proposed_role: invite.proposed_role,
            status: invite.status,
            local_path: invite.local_path,
            error: invite.error,
            created_unix_ms: invite.created_unix_ms,
        })
        .collect())
}

#[tauri::command]
pub fn accept_task_invite(
    state: State<'_, AppState>,
    invite_id: String,
    local_path: String,
) -> Result<SyncTask, String> {
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let existing = repository::SyncTaskRepository::new(&db)
            .list_all()
            .map_err(|e| e.to_string())?;
        validate_no_overlapping_task_roots(&existing, &local_path)?;
    }
    let server = state._server.as_ref().ok_or("sync server is not running")?;
    if let Some(invite) = server
        .list_task_invites()
        .into_iter()
        .find(|invite| invite.invite_id == invite_id)
    {
        let proposed_role = parse_device_role(&invite.proposed_role)?;
        validate_task_folder_for_role(&state, &local_path, proposed_role)?;
        let db = state.db.lock().map_err(|e| e.to_string())?;
        ensure_paired_device_public_key_matches(
            &db,
            &invite.requester_device_id,
            &invite.requester_public_key,
        )
        .map_err(|e| e.to_string())?;
    }
    let invite = server
        .accept_task_invite(&invite_id, &local_path)
        .map_err(|e| e.to_string())?;
    let local_role = parse_device_role(&invite.proposed_role)?;
    let local_identity = state.identity.public();
    let peer_device_id = invite.requester_device_id.clone();
    let requester_address = invite.requester_address.clone();
    let (primary_device_id, secondary_device_id) = match local_role {
        DeviceRole::Primary => (local_identity.device_id, peer_device_id.clone()),
        DeviceRole::Secondary => (peer_device_id.clone(), local_identity.device_id),
    };
    let task_id = Uuid::parse_str(&invite.task_id).map_err(|e| e.to_string())?;
    let task = SyncTask {
        id: task_id,
        name: invite.task_name,
        primary_device_id,
        secondary_device_id,
        local_path,
        remote_path: invite.requester_path.unwrap_or_default(),
        local_role,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };
    let db = state.db.lock().map_err(|e| e.to_string())?;
    if !invite.requester_public_key.is_empty() {
        ensure_paired_device_public_key_matches(&db, &peer_device_id, &invite.requester_public_key)
            .map_err(|e| e.to_string())?;
        state.connections.pin_peer(crate::pairing::PublicIdentity {
            device_id: peer_device_id.clone(),
            public_key: invite.requester_public_key.clone(),
        });
        repository::PairedDeviceRepository::new(&db)
            .upsert(&PairedDevice {
                device_id: peer_device_id.clone(),
                display_name: invite.requester_device_id.clone(),
                public_key: invite.requester_public_key,
                last_seen_unix_ms: now_ms(),
                trusted: true,
                last_address: requester_address.clone(),
            })
            .map_err(|e| e.to_string())?;
    }
    if let Some(address) = requester_address {
        state
            .connections
            .register_connection(connection::PeerConnection {
                device_id: peer_device_id,
                address,
                connected: true,
                last_seen_unix_ms: now_ms(),
            });
    }
    repository::SyncTaskRepository::new(&db)
        .insert(&task)
        .map_err(|e| e.to_string())?;
    Ok(task)
}

#[tauri::command]
pub fn reject_task_invite(
    state: State<'_, AppState>,
    invite_id: String,
    reason: Option<String>,
) -> Result<(), String> {
    let server = state._server.as_ref().ok_or("sync server is not running")?;
    server
        .reject_task_invite(&invite_id, reason.as_deref().unwrap_or("rejected by peer"))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn create_sync_task(
    state: State<'_, AppState>,
    request: CreateTaskRequest,
) -> Result<SyncTask, String> {
    let local_role = parse_device_role(&request.local_role)?;
    validate_task_folder_for_role(&state, &request.local_path, local_role)?;
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let existing = repository::SyncTaskRepository::new(&db)
            .list_all()
            .map_err(|e| e.to_string())?;
        validate_no_overlapping_task_roots(&existing, &request.local_path)?;
    }

    let identity = state.identity.public();
    // Verify peer is pinned before creating task
    state
        .connections
        .get_pinned(&request.peer_device_id)
        .ok_or("peer device not pinned")?;

    let peer_device_id = request.peer_device_id.clone();
    let (primary_id, secondary_id) = match local_role {
        DeviceRole::Primary => (identity.device_id, peer_device_id.clone()),
        DeviceRole::Secondary => (peer_device_id.clone(), identity.device_id),
    };

    let task = SyncTask {
        id: Uuid::new_v4(),
        name: request.name,
        primary_device_id: primary_id,
        secondary_device_id: secondary_id,
        local_path: request.local_path,
        remote_path: request.remote_path.unwrap_or_default().trim().to_string(),
        local_role,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };

    if let Some(server) = &state._server {
        server
            .register_task_root(task.id.to_string(), &task.local_path)
            .map_err(|e| e.to_string())?;
    }

    let mut task = task;
    if task.remote_path.is_empty() {
        let proposed_role = match task.local_role {
            DeviceRole::Primary => "Secondary".to_string(),
            DeviceRole::Secondary => "Primary".to_string(),
        };
        match send_task_invite_to_peer(
            &state,
            &peer_device_id,
            Uuid::new_v4().to_string(),
            task.id.to_string(),
            task.name.clone(),
            Some(task.local_path.clone()),
            proposed_role,
        )
        .await
        {
            Ok(SyncMessage::TaskInviteAck {
                success: true,
                remote_path: Some(remote_path),
                ..
            }) => task.remote_path = remote_path,
            Ok(SyncMessage::TaskInvitePending { .. }) => {
                return Err("task invitation is waiting for peer approval".to_string())
            }
            Ok(SyncMessage::TaskInviteAck { error, .. }) => {
                return Err(error.unwrap_or_else(|| "peer rejected task invitation".to_string()))
            }
            Ok(other) => return Err(format!("unexpected peer response: {:?}", other)),
            Err(e) => return Err(format!("task invitation failed: {}", e)),
        }
    } else {
        let register_msg = SyncMessage::TaskRegister {
            task_id: task.id.to_string(),
            root_path: task.remote_path.clone(),
        };
        match connection::send_authenticated_message_to_peer(
            &state.connections,
            &state.identity,
            &peer_device_id,
            register_msg,
        )
        .await
        {
            Ok(SyncMessage::TaskAck { success: true, .. }) => {}
            Ok(SyncMessage::TaskAck { error, .. }) => {
                return Err(error.unwrap_or_else(|| "peer rejected task registration".to_string()))
            }
            Ok(other) => return Err(format!("unexpected peer response: {:?}", other)),
            Err(e) => return Err(format!("task registration failed: {}", e)),
        }
    }

    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let repo = repository::SyncTaskRepository::new(&db);
        repo.insert(&task).map_err(|e| e.to_string())?;
    }
    state.start_task_watcher(&task).map_err(|e| e.to_string())?;
    if task.local_role == DeviceRole::Primary {
        state.dirty_tasks.mark_task_dirty(task.id);
    }
    Ok(task)
}

fn ensure_paired_device_public_key_matches(
    db: &rusqlite::Connection,
    device_id: &str,
    public_key: &[u8],
) -> Result<(), String> {
    if public_key.is_empty() {
        return Ok(());
    }
    let existing = repository::PairedDeviceRepository::new(db)
        .get(device_id)
        .map_err(|e| e.to_string())?;
    if let Some(existing) = existing {
        if existing.public_key != public_key {
            return Err(format!(
                "paired device public key changed for device {}; reject and re-pair before accepting this invite",
                device_id
            ));
        }
    }
    Ok(())
}

async fn send_task_invite_to_peer(
    state: &State<'_, AppState>,
    peer_device_id: &str,
    invite_id: String,
    task_id: String,
    task_name: String,
    requester_path: Option<String>,
    proposed_role: String,
) -> Result<SyncMessage, String> {
    let requester_port = state._server.as_ref().map_or(0, |server| server.port());
    let authenticated_msg = SyncMessage::TaskInvite {
        invite_id: invite_id.clone(),
        task_id: task_id.clone(),
        task_name: task_name.clone(),
        requester_port,
        requester_path: requester_path.clone(),
        proposed_role: proposed_role.clone(),
    };

    match connection::send_authenticated_message_to_peer(
        &state.connections,
        &state.identity,
        peer_device_id,
        authenticated_msg,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(auth_error) => {
            let local_public = state.identity.public();
            connection::send_message_to_peer(
                &state.connections,
                peer_device_id,
                SyncMessage::TaskInviteProposal {
                    invite_id,
                    task_id,
                    task_name,
                    requester_device_id: local_public.device_id,
                    requester_public_key: local_public.public_key,
                    requester_port,
                    requester_path,
                    proposed_role,
                },
            )
            .await
            .map_err(|proposal_error| {
                format!(
                    "task invitation failed: {}; proposal fallback failed: {}",
                    auth_error, proposal_error
                )
            })
        }
    }
}

fn parse_device_role(role: &str) -> Result<DeviceRole, String> {
    match role {
        "Primary" => Ok(DeviceRole::Primary),
        "Secondary" => Ok(DeviceRole::Secondary),
        other => Err(format!("invalid device role: {}", other)),
    }
}

fn validate_no_overlapping_task_roots(
    existing_tasks: &[SyncTask],
    new_local_path: &str,
) -> Result<(), String> {
    for task in existing_tasks {
        if paths_overlap(&task.local_path, new_local_path) {
            return Err(format!(
                "sync folder overlaps with existing task '{}'",
                task.name
            ));
        }
    }

    Ok(())
}

fn paths_overlap(left: &str, right: &str) -> bool {
    let left = comparable_path_components(left);
    let right = comparable_path_components(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }

    is_prefix_path(&left, &right) || is_prefix_path(&right, &left)
}

fn comparable_path_components(path: &str) -> Vec<String> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| Path::new(path).to_path_buf());
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().replace('\\', "/"))
        .filter(|component| !component.is_empty())
        .map(|component| {
            if cfg!(windows) {
                component.to_ascii_lowercase()
            } else {
                component
            }
        })
        .collect()
}

fn is_prefix_path(prefix: &[String], path: &[String]) -> bool {
    prefix.len() <= path.len()
        && prefix
            .iter()
            .zip(path.iter())
            .all(|(prefix_component, path_component)| prefix_component == path_component)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn parse_device_role_rejects_unknown_role() {
        let result = parse_device_role("Mirror");

        assert!(result.is_err());
    }

    fn task_with_local_path(path: &str) -> SyncTask {
        SyncTask {
            id: Uuid::new_v4(),
            name: "task".to_string(),
            primary_device_id: "primary".to_string(),
            secondary_device_id: "secondary".to_string(),
            local_path: path.to_string(),
            remote_path: String::new(),
            local_role: DeviceRole::Primary,
            enabled: true,
            created_unix_ms: 0,
            updated_unix_ms: 0,
        }
    }

    #[test]
    fn task_roots_reject_nested_local_path() {
        let existing = vec![task_with_local_path("C:/Sync/Main")];

        let result = validate_no_overlapping_task_roots(&existing, "C:/Sync/Main/Child");

        assert!(result.is_err());
    }

    #[test]
    fn task_roots_reject_parent_local_path() {
        let existing = vec![task_with_local_path("C:/Sync/Main/Child")];

        let result = validate_no_overlapping_task_roots(&existing, "C:/Sync/Main");

        assert!(result.is_err());
    }

    #[test]
    fn paired_device_public_key_mismatch_is_rejected() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::state::db::migrate(&conn).unwrap();
        repository::PairedDeviceRepository::new(&conn)
            .upsert(&PairedDevice {
                device_id: "peer-1".to_string(),
                display_name: "Peer".to_string(),
                public_key: vec![1; 32],
                last_seen_unix_ms: 1,
                trusted: true,
                last_address: None,
            })
            .unwrap();

        let result = ensure_paired_device_public_key_matches(&conn, "peer-1", &[2; 32]);

        assert!(result.unwrap_err().contains("public key changed"));
        let loaded = repository::PairedDeviceRepository::new(&conn)
            .get("peer-1")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.public_key, vec![1; 32]);
    }

    #[test]
    fn collapse_history_folder_entries_hides_same_batch_children() {
        let task_id = Uuid::new_v4();
        let parent = HistoryEntry {
            id: Uuid::new_v4(),
            task_id,
            original_relative_path: "folder".to_string(),
            stored_path: "/tmp/history/trash/batch/folder".to_string(),
            reason: HistoryReason::Trash,
            created_unix_ms: 2_000,
            size: 0,
        };
        let child = HistoryEntry {
            id: Uuid::new_v4(),
            task_id,
            original_relative_path: "folder/file.txt".to_string(),
            stored_path: "/tmp/history/trash/batch/folder/file.txt".to_string(),
            reason: HistoryReason::Trash,
            created_unix_ms: 2_000,
            size: 12,
        };
        let overwritten = HistoryEntry {
            id: Uuid::new_v4(),
            task_id,
            original_relative_path: "folder/old.txt".to_string(),
            stored_path: "/tmp/history/overwritten/3000/folder/old.txt".to_string(),
            reason: HistoryReason::Overwritten,
            created_unix_ms: 3_000,
            size: 9,
        };

        let collapsed = collapse_history_folder_entries(vec![
            child.clone(),
            overwritten.clone(),
            parent.clone(),
        ]);

        assert!(collapsed.iter().any(|entry| entry.id == parent.id));
        assert!(collapsed.iter().any(|entry| entry.id == overwritten.id));
        assert!(!collapsed.iter().any(|entry| entry.id == child.id));
    }

    #[test]
    fn primary_missing_baseline_detects_file_moved_without_watcher_event() {
        let dir = tempfile::tempdir().unwrap();
        let local_path = dir.path().to_string_lossy().to_string();
        let task = task_with_local_path(&local_path);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::state::db::migrate(&conn).unwrap();
        repository::SyncTaskRepository::new(&conn)
            .insert(&task)
            .unwrap();
        repository::SyncBaselineRepository::new(&conn)
            .upsert(&SyncBaseline {
                task_id: task.id,
                relative_path: "moved.zip".to_string(),
                primary_hash: Some("hash".to_string()),
                primary_hash_status: HashStatus::Verified,
                primary_size: 1,
                secondary_size: 1,
                primary_modified_unix_ms: 1,
                secondary_hash: Some("hash".to_string()),
                secondary_hash_status: HashStatus::Verified,
                secondary_modified_unix_ms: 1,
                last_synced_unix_ms: 1,
            })
            .unwrap();

        assert!(primary_task_has_missing_baseline(&task, &conn).unwrap());

        std::fs::write(dir.path().join("moved.zip"), b"x").unwrap();
        assert!(!primary_task_has_missing_baseline(&task, &conn).unwrap());
    }

    #[test]
    fn successful_apply_hashes_unverified_file_for_baseline() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("large.bin"), "contents").unwrap();
        let task = SyncTask {
            id: Uuid::new_v4(),
            name: "task".to_string(),
            primary_device_id: "primary".to_string(),
            secondary_device_id: "secondary".to_string(),
            local_path: dir.path().to_string_lossy().to_string(),
            remote_path: String::new(),
            local_role: DeviceRole::Primary,
            enabled: true,
            created_unix_ms: 0,
            updated_unix_ms: 0,
        };
        let snap = FileSnapshot {
            task_id: task.id,
            relative_path: "large.bin".to_string(),
            kind: EntryKind::File,
            size: 8,
            modified_unix_ms: 0,
            blake3_hash: None,
            hash_status: HashStatus::UnverifiedLargeFile,
            deleted: false,
            is_symlink: false,
        };
        let action = planner::PlannedAction {
            relative_path: "large.bin".to_string(),
            decision: SyncDecision::ApplyToSecondary,
            snapshot: Some(snap.clone()),
            baseline: None,
        };

        let (hash, status) = verified_hash_for_successful_apply(&task, &action, &snap, None);

        assert_eq!(status, HashStatus::Verified);
        assert_eq!(hash, Some(blake3::hash(b"contents").to_hex().to_string()));
    }

    #[test]
    fn successful_apply_reuses_transferred_hash_for_baseline() {
        let task = task_with_local_path("/tmp/does-not-need-to-exist");
        let snap = FileSnapshot {
            task_id: task.id,
            relative_path: "large.bin".to_string(),
            kind: EntryKind::File,
            size: crate::core::scanner::EAGER_HASH_LIMIT + 1,
            modified_unix_ms: 0,
            blake3_hash: None,
            hash_status: HashStatus::UnverifiedLargeFile,
            deleted: false,
            is_symlink: false,
        };
        let action = planner::PlannedAction {
            relative_path: "large.bin".to_string(),
            decision: SyncDecision::ApplyToSecondary,
            snapshot: Some(snap.clone()),
            baseline: None,
        };

        let (hash, status) =
            verified_hash_for_successful_apply(&task, &action, &snap, Some("streamed-hash"));

        assert_eq!(status, HashStatus::Verified);
        assert_eq!(hash, Some("streamed-hash".to_string()));
    }

    #[test]
    fn metadata_delta_detects_new_and_missing_paths() {
        let old = vec![FileSnapshot {
            task_id: Uuid::nil(),
            relative_path: "folder/old.txt".to_string(),
            kind: EntryKind::File,
            size: 3,
            modified_unix_ms: 1,
            blake3_hash: Some("hash".to_string()),
            hash_status: HashStatus::Verified,
            deleted: false,
            is_symlink: false,
        }];
        let same = vec![FileSnapshot {
            blake3_hash: None,
            hash_status: HashStatus::Unavailable,
            ..old[0].clone()
        }];
        assert!(!metadata_delta(&old, &same));
        assert!(metadata_delta(&old, &[]));
        let mut with_new = same.clone();
        with_new.push(FileSnapshot {
            task_id: Uuid::nil(),
            relative_path: "folder/new.txt".to_string(),
            kind: EntryKind::File,
            size: 4,
            modified_unix_ms: 2,
            blake3_hash: None,
            hash_status: HashStatus::Unavailable,
            deleted: false,
            is_symlink: false,
        });
        assert!(metadata_delta(&old, &with_new));
    }

    #[test]
    fn recovered_delete_requires_verified_remote_match_without_baseline() {
        let hash = blake3::hash(b"same").to_hex().to_string();
        let action = planner::PlannedAction {
            relative_path: "old.txt".to_string(),
            decision: SyncDecision::MoveSecondaryToHistory,
            snapshot: Some(FileSnapshot {
                task_id: Uuid::nil(),
                relative_path: "old.txt".to_string(),
                kind: EntryKind::File,
                size: 4,
                modified_unix_ms: 1,
                blake3_hash: Some(hash.clone()),
                hash_status: HashStatus::Verified,
                deleted: false,
                is_symlink: false,
            }),
            baseline: None,
        };
        let matching = RemoteFileState {
            relative_path: "old.txt".to_string(),
            kind: EntryKind::File,
            blake3_hash: Some(hash),
            hash_status: HashStatus::Verified,
            size: 4,
            modified_unix_ms: 1,
        };
        let remote = HashMap::from([("old.txt", &matching)]);
        assert_eq!(remote_delete_safety_error(&action, &remote), None);

        let changed = RemoteFileState {
            relative_path: "old.txt".to_string(),
            kind: EntryKind::File,
            blake3_hash: Some("changed".to_string()),
            hash_status: HashStatus::Verified,
            size: 4,
            modified_unix_ms: 1,
        };
        let remote = HashMap::from([("old.txt", &changed)]);
        assert_eq!(
            remote_delete_safety_error(&action, &remote).as_deref(),
            Some("remote changed since last sync")
        );
    }

    #[test]
    fn baseline_delete_defers_verified_large_file_check_to_receiver() {
        let baseline = SyncBaseline {
            task_id: Uuid::nil(),
            relative_path: "large.bin".to_string(),
            primary_hash: Some("hash".to_string()),
            primary_hash_status: HashStatus::Verified,
            primary_size: crate::core::scanner::EAGER_HASH_LIMIT + 1,
            secondary_size: crate::core::scanner::EAGER_HASH_LIMIT + 1,
            primary_modified_unix_ms: 1,
            secondary_hash: Some("hash".to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: 1,
            last_synced_unix_ms: 1,
        };
        let action = planner::PlannedAction {
            relative_path: "large.bin".to_string(),
            decision: SyncDecision::MoveSecondaryToHistory,
            snapshot: None,
            baseline: Some(baseline),
        };
        let remote = RemoteFileState {
            relative_path: "large.bin".to_string(),
            kind: EntryKind::File,
            blake3_hash: None,
            hash_status: HashStatus::UnverifiedLargeFile,
            size: crate::core::scanner::EAGER_HASH_LIMIT + 1,
            modified_unix_ms: 1,
        };
        let remote_map = HashMap::from([("large.bin", &remote)]);

        assert_eq!(remote_delete_safety_error(&action, &remote_map), None);
    }
}

fn create_task_from_invite(
    state: &State<'_, AppState>,
    pending: &PendingOutgoingTaskInvite,
    remote_path: String,
) -> Result<SyncTask, String> {
    let identity = state.identity.public();
    let (primary_device_id, secondary_device_id) = match pending.local_role {
        DeviceRole::Primary => (identity.device_id, pending.peer_device_id.clone()),
        DeviceRole::Secondary => (pending.peer_device_id.clone(), identity.device_id),
    };
    let task = SyncTask {
        id: pending.task_id,
        name: pending.name.clone(),
        primary_device_id,
        secondary_device_id,
        local_path: pending.local_path.clone(),
        remote_path,
        local_role: pending.local_role,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let existing = repository::SyncTaskRepository::new(&db)
            .list_all()
            .map_err(|e| e.to_string())?;
        validate_no_overlapping_task_roots(&existing, &task.local_path)?;
    }

    if let Some(server) = &state._server {
        server
            .register_task_root(task.id.to_string(), &task.local_path)
            .map_err(|e| e.to_string())?;
    }

    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::SyncTaskRepository::new(&db)
            .insert(&task)
            .map_err(|e| e.to_string())?;
    }
    state.start_task_watcher(&task).map_err(|e| e.to_string())?;
    if task.local_role == DeviceRole::Primary {
        state.dirty_tasks.mark_task_dirty(task.id);
    }
    Ok(task)
}

fn persist_pending_outgoing_invite(
    db: &rusqlite::Connection,
    invite_id: &str,
    pending: &PendingOutgoingTaskInvite,
) -> Result<(), String> {
    let local_role = match pending.local_role {
        DeviceRole::Primary => "Primary",
        DeviceRole::Secondary => "Secondary",
    };
    db.execute(
        "INSERT INTO pending_outgoing_task_invites
            (invite_id, task_id, name, local_path, peer_device_id, local_role, created_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(invite_id) DO UPDATE SET
            task_id = excluded.task_id,
            name = excluded.name,
            local_path = excluded.local_path,
            peer_device_id = excluded.peer_device_id,
            local_role = excluded.local_role",
        params![
            invite_id,
            pending.task_id.to_string(),
            &pending.name,
            &pending.local_path,
            &pending.peer_device_id,
            local_role,
            now_ms(),
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn load_pending_outgoing_invite(
    db: &rusqlite::Connection,
    invite_id: &str,
) -> Result<Option<PendingOutgoingTaskInvite>, String> {
    let mut stmt = db
        .prepare(
            "SELECT task_id, name, local_path, peer_device_id, local_role
             FROM pending_outgoing_task_invites WHERE invite_id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let result = stmt.query_row(params![invite_id], |row| {
        let role: String = row.get(4)?;
        Ok(PendingOutgoingTaskInvite {
            task_id: Uuid::parse_str(&row.get::<_, String>(0)?).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
            name: row.get(1)?,
            local_path: row.get(2)?,
            peer_device_id: row.get(3)?,
            local_role: parse_device_role(&role).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
                )
            })?,
        })
    });
    match result {
        Ok(invite) => Ok(Some(invite)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn remove_pending_outgoing_invite(
    db: &rusqlite::Connection,
    invite_id: &str,
) -> Result<(), String> {
    db.execute(
        "DELETE FROM pending_outgoing_task_invites WHERE invite_id = ?1",
        params![invite_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_sync_tasks(state: State<'_, AppState>) -> Result<Vec<SyncTask>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::SyncTaskRepository::new(&db);
    repo.list_all().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_sync_task(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Option<SyncTask>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::SyncTaskRepository::new(&db);
    repo.get(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_task_enabled(
    state: State<'_, AppState>,
    task_id: String,
    enabled: bool,
) -> Result<(), String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let task = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let repo = repository::SyncTaskRepository::new(&db);
        repo.get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?
    };
    if enabled {
        if let Some(server) = &state._server {
            server
                .register_task_root(task.id.to_string(), &task.local_path)
                .map_err(|e| e.to_string())?;
        }
    }
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let repo = repository::SyncTaskRepository::new(&db);
        repo.update_enabled(&id, enabled, now_ms())
            .map_err(|e| e.to_string())?;
    }
    if enabled {
        state.start_task_watcher(&task).map_err(|e| e.to_string())?;
        if task.local_role == DeviceRole::Primary {
            state.dirty_tasks.mark_task_dirty(task.id);
        }
    }
    if !enabled {
        if let Some(server) = &state._server {
            server
                .unregister_task_root(&task_id)
                .map_err(|e| e.to_string())?;
        }
        state.dirty_tasks.clear(id);
    }
    Ok(())
}

#[tauri::command]
pub fn list_ready_auto_sync_tasks(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let ready_ids = state.dirty_tasks.ready_task_ids();
    let mut ready_set = ready_ids.into_iter().collect::<HashSet<_>>();

    let task_states = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let tasks = repository::SyncTaskRepository::new(&db)
            .list_all()
            .map_err(|e| e.to_string())?;
        let baseline_repo = repository::SyncBaselineRepository::new(&db);
        let snapshot_repo = repository::FileSnapshotRepository::new(&db);
        let mut task_states = Vec::new();
        for task in tasks {
            let baselines = baseline_repo
                .list_by_task(&task.id)
                .map_err(|e| e.to_string())?;
            let cached_snapshots = snapshot_repo
                .list_by_task(&task.id)
                .map_err(|e| e.to_string())?;
            task_states.push((task, baselines, cached_snapshots));
        }
        task_states
    };

    for (task, baselines, cached_snapshots) in &task_states {
        if task.enabled && task.local_role == DeviceRole::Primary {
            let reasons =
                primary_task_needs_sync_sweep(task, baselines, cached_snapshots, &*state.platform)
                    .map_err(|e| e.to_string())?;
            if !reasons.is_empty() {
                tracing::info!(
                    auto_sync_ready = true,
                    task_id = %task.id,
                    ready_reason = %reasons.join(",")
                );
                ready_set.insert(task.id);
            }
        }
    }
    if ready_set.is_empty() {
        return Ok(Vec::new());
    }
    Ok(task_states
        .into_iter()
        .map(|(task, _, _)| task)
        .filter(|task| {
            task.enabled && task.local_role == DeviceRole::Primary && ready_set.contains(&task.id)
        })
        .map(|task| task.id.to_string())
        .collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskFileListRefreshHint {
    pub revision: u64,
    pub should_refresh: bool,
    pub quiet_ms: u64,
    pub reason: String,
}

#[tauri::command]
pub fn get_task_file_list_refresh_hint(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<TaskFileListRefreshHint, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;

    if state
        .file_list_refresh
        .should_check_metadata(id, FILE_LIST_METADATA_CHECK_INTERVAL)
    {
        let (task, cached_snapshots) = {
            let db = state.db.lock().map_err(|e| e.to_string())?;
            let task = repository::SyncTaskRepository::new(&db)
                .get(&id)
                .map_err(|e| e.to_string())?
                .ok_or("task not found")?;
            let cached_snapshots = repository::FileSnapshotRepository::new(&db)
                .list_by_task(&id)
                .map_err(|e| e.to_string())?;
            (task, cached_snapshots)
        };

        if task.enabled {
            let current_metadata =
                scanner::scan_root_metadata(Path::new(&task.local_path), &*state.platform)
                    .map_err(|e| e.to_string())?;
            if metadata_delta(&cached_snapshots, &current_metadata) {
                state.file_list_refresh.mark(id, "metadata_delta");
                if task.local_role == DeviceRole::Primary {
                    state.dirty_tasks.mark_task_dirty(id);
                }
            }
        }
    }

    let snapshot = state.file_list_refresh.snapshot(id);
    let quiet_ms = snapshot
        .last_changed_at
        .map(|last| Instant::now().duration_since(last).as_millis() as u64)
        .unwrap_or(0);
    let should_refresh = snapshot.revision > 0 && quiet_ms >= FILE_LIST_REFRESH_QUIET_MS;
    Ok(TaskFileListRefreshHint {
        revision: snapshot.revision,
        should_refresh,
        quiet_ms,
        reason: snapshot.reason.to_string(),
    })
}

#[cfg(test)]
fn primary_task_has_missing_baseline(
    task: &SyncTask,
    db: &rusqlite::Connection,
) -> anyhow::Result<bool> {
    let root = Path::new(&task.local_path);
    let baselines = repository::SyncBaselineRepository::new(db).list_by_task(&task.id)?;
    for baseline in baselines {
        let path = path_safety::safe_join(root, &baseline.relative_path)?;
        if !path.exists() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn primary_task_needs_sync_sweep(
    task: &SyncTask,
    baselines: &[SyncBaseline],
    cached_snapshots: &[FileSnapshot],
    platform: &dyn crate::platform::Platform,
) -> anyhow::Result<Vec<&'static str>> {
    let root = Path::new(&task.local_path);
    let mut reasons = Vec::new();
    for baseline in baselines {
        let path = path_safety::safe_join(root, &baseline.relative_path)?;
        if !path.exists() {
            reasons.push("missing_baseline");
            break;
        }
    }

    let current_metadata = scanner::scan_root_metadata(root, platform)?;
    if metadata_delta(cached_snapshots, &current_metadata) {
        reasons.push("metadata_delta");
    }
    reasons.sort_unstable();
    reasons.dedup();
    Ok(reasons)
}

fn metadata_delta(cached_snapshots: &[FileSnapshot], current_metadata: &[FileSnapshot]) -> bool {
    let old_map = cached_snapshots
        .iter()
        .filter(|snapshot| !snapshot.deleted && !snapshot.is_symlink)
        .map(|snapshot| (snapshot.relative_path.as_str(), snapshot))
        .collect::<HashMap<_, _>>();
    let current_map = current_metadata
        .iter()
        .filter(|snapshot| !snapshot.deleted && !snapshot.is_symlink)
        .map(|snapshot| (snapshot.relative_path.as_str(), snapshot))
        .collect::<HashMap<_, _>>();

    for (path, old) in &old_map {
        let Some(current) = current_map.get(path) else {
            return true;
        };
        if snapshot_metadata_changed(old, current) {
            return true;
        }
    }
    current_map.keys().any(|path| !old_map.contains_key(path))
}

fn snapshot_metadata_changed(old: &FileSnapshot, current: &FileSnapshot) -> bool {
    if old.kind != current.kind {
        return true;
    }
    if old.kind == EntryKind::Directory {
        return false;
    }
    old.size != current.size || old.modified_unix_ms != current.modified_unix_ms
}

// ─── Scan ───

#[tauri::command]
pub fn scan_task(state: State<'_, AppState>, task_id: String) -> Result<Vec<FileSnapshot>, String> {
    crate::diagnostics::record_operation("scan_task_command_enter", format!("task_id={task_id}"));
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let (task, cached_snapshots) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let task_repo = repository::SyncTaskRepository::new(&db);
        let task = task_repo
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?;
        let cached_snapshots = repository::FileSnapshotRepository::new(&db)
            .list_by_task(&id)
            .map_err(|e| e.to_string())?;
        (task, cached_snapshots)
    };

    let sync_root = std::path::Path::new(&task.local_path);
    let cached_snapshot_list = cached_snapshots;
    let cache = snapshot_cache_by_path(cached_snapshot_list.clone());
    let results =
        guarded_scan_root_with_cache(id, sync_root, &*state.platform, &cache, "scan_task")?;

    let mut snapshots = Vec::new();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let snap_repo = repository::FileSnapshotRepository::new(&db);
    for result in &results {
        let mut snap = result.snapshot.clone();
        snap.task_id = id;
        snapshots.push(snap);
    }
    snap_repo
        .replace_for_task(&id, &snapshots)
        .map_err(|e| e.to_string())?;

    crate::diagnostics::record_operation(
        "scan_task_command_complete",
        format!(
            "task_id={} task_name={} entries={}",
            id,
            task.name,
            snapshots.len()
        ),
    );
    Ok(snapshots)
}

fn refresh_task_snapshots(state: &AppState, task: &SyncTask) -> Result<Vec<FileSnapshot>, String> {
    let sync_root = std::path::Path::new(&task.local_path);
    let cached_snapshots = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::FileSnapshotRepository::new(&db)
            .list_by_task(&task.id)
            .map_err(|e| e.to_string())?
    };
    let cache = snapshot_cache_by_path(cached_snapshots);
    let results = guarded_scan_root_with_cache(
        task.id,
        sync_root,
        &*state.platform,
        &cache,
        "refresh_task_snapshots",
    )?;

    let snapshots = results
        .into_iter()
        .map(|result| {
            let mut snap = result.snapshot;
            snap.task_id = task.id;
            snap
        })
        .collect::<Vec<_>>();

    let db = state.db.lock().map_err(|e| e.to_string())?;
    repository::FileSnapshotRepository::new(&db)
        .replace_for_task(&task.id, &snapshots)
        .map_err(|e| e.to_string())?;

    Ok(snapshots)
}

fn snapshot_cache_by_path(snapshots: Vec<FileSnapshot>) -> HashMap<String, FileSnapshot> {
    snapshots
        .into_iter()
        .map(|snapshot| (snapshot.relative_path.clone(), snapshot))
        .collect()
}

fn guarded_scan_root_with_cache(
    task_id: Uuid,
    sync_root: &Path,
    platform: &dyn crate::platform::traits::Platform,
    cache: &HashMap<String, FileSnapshot>,
    reason: &str,
) -> Result<Vec<scanner::ScanResult>, String> {
    tracing::info!(
        task_id = %task_id,
        root = %sync_root.display(),
        reason,
        cached = cache.len(),
        "local scan command start"
    );
    crate::diagnostics::record_operation(
        "scan_start",
        format!(
            "task_id={} root={} reason={} cached={}",
            task_id,
            sync_root.display(),
            reason,
            cache.len()
        ),
    );
    let started = Instant::now();
    let scanned = catch_unwind(AssertUnwindSafe(|| {
        scanner::scan_root_with_cache(sync_root, platform, cache)
    }));

    match scanned {
        Ok(Ok(results)) => {
            crate::diagnostics::record_operation(
                "scan_complete",
                format!(
                    "task_id={} root={} reason={} entries={} elapsed_ms={}",
                    task_id,
                    sync_root.display(),
                    reason,
                    results.len(),
                    started.elapsed().as_millis()
                ),
            );
            tracing::info!(
                task_id = %task_id,
                root = %sync_root.display(),
                reason,
                entries = results.len(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "local scan command complete"
            );
            Ok(results)
        }
        Ok(Err(error)) => {
            crate::diagnostics::record_operation(
                "scan_failed",
                format!(
                    "task_id={} root={} reason={} error={} elapsed_ms={}",
                    task_id,
                    sync_root.display(),
                    reason,
                    error,
                    started.elapsed().as_millis()
                ),
            );
            tracing::error!(
                task_id = %task_id,
                root = %sync_root.display(),
                reason,
                error = %error,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "local scan command failed"
            );
            Err(error.to_string())
        }
        Err(_) => {
            crate::diagnostics::record_operation(
                "scan_panicked",
                format!(
                    "task_id={} root={} reason={} elapsed_ms={}",
                    task_id,
                    sync_root.display(),
                    reason,
                    started.elapsed().as_millis()
                ),
            );
            tracing::error!(
                task_id = %task_id,
                root = %sync_root.display(),
                reason,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "local scan command panicked"
            );
            Err("本地扫描异常，请查看 LanBridge 日志".to_string())
        }
    }
}

// ─── Sync ───

#[derive(Debug, Clone, Serialize)]
pub struct SyncActionResult {
    pub relative_path: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncProgress {
    pub task_id: String,
    pub phase: String,
    pub detail: Option<String>,
    pub items_done: Option<u64>,
    pub items_total: Option<u64>,
    pub bytes_done: Option<u64>,
    pub bytes_total: Option<u64>,
    pub finished: Option<bool>,
    #[serde(skip_serializing)]
    pub finished_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskPeerStatus {
    pub task_id: String,
    pub peer_device_id: String,
    pub address: Option<String>,
    pub connected: bool,
    pub last_seen_unix_ms: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeferredTransfer {
    pub task_id: String,
    pub relative_path: String,
    pub direction: String,
    pub reason: String,
    pub created_unix_ms: i64,
}

static SYNC_PROGRESS: std::sync::LazyLock<Mutex<HashMap<String, SyncProgress>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn record_sync_progress(task_id: Uuid, phase: &str, detail: Option<String>) {
    if let Ok(mut progress) = SYNC_PROGRESS.lock() {
        let key = task_id.to_string();
        let entry = progress.entry(key.clone()).or_insert_with(|| SyncProgress {
            task_id: key,
            phase: phase.to_string(),
            detail: detail.clone(),
            items_done: None,
            items_total: None,
            bytes_done: None,
            bytes_total: None,
            finished: Some(false),
            finished_at_unix_ms: None,
        });
        entry.phase = phase.to_string();
        entry.detail = detail;
        entry.finished = Some(false);
        entry.finished_at_unix_ms = None;
    }
}

fn record_sync_progress_totals(task_id: Uuid, items_total: u64, bytes_total: u64) {
    if let Ok(mut progress) = SYNC_PROGRESS.lock() {
        let key = task_id.to_string();
        let entry = progress.entry(key.clone()).or_insert_with(|| SyncProgress {
            task_id: key,
            phase: "同步中".to_string(),
            detail: None,
            items_done: Some(0),
            items_total: Some(items_total),
            bytes_done: Some(0),
            bytes_total: Some(bytes_total),
            finished: Some(false),
            finished_at_unix_ms: None,
        });
        entry.items_done = Some(0);
        entry.items_total = Some(items_total);
        entry.bytes_done = Some(0);
        entry.bytes_total = Some(bytes_total);
        entry.finished = Some(false);
        entry.finished_at_unix_ms = None;
    }
}

fn advance_sync_progress(task_id: Uuid, bytes_done_delta: u64) {
    if let Ok(mut progress) = SYNC_PROGRESS.lock() {
        let Some(entry) = progress.get_mut(&task_id.to_string()) else {
            return;
        };
        entry.items_done = Some(entry.items_done.unwrap_or(0).saturating_add(1));
        entry.bytes_done = Some(
            entry
                .bytes_done
                .unwrap_or(0)
                .saturating_add(bytes_done_delta),
        );
    }
}

fn finish_sync_progress(task_id: Uuid) {
    if let Ok(mut progress) = SYNC_PROGRESS.lock() {
        if let Some(entry) = progress.get_mut(&task_id.to_string()) {
            entry.phase = "同步完成".to_string();
            entry.detail = None;
            entry.finished = Some(true);
            entry.finished_at_unix_ms = Some(now_ms());
            if let Some(total) = entry.items_total {
                entry.items_done = Some(total);
            }
            if let Some(total) = entry.bytes_total {
                entry.bytes_done = Some(total);
            }
        }
    }
}

#[tauri::command]
pub async fn sync_now(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<SyncActionResult>, String> {
    run_sync_now(state.inner(), task_id).await
}

pub async fn run_sync_now(
    state: &AppState,
    task_id: String,
) -> Result<Vec<SyncActionResult>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    if state.sync_runs.begin(id) == SyncRunAdmission::Queued {
        record_sync_progress(id, "已排队", Some("已有同步正在执行".to_string()));
        return Ok(vec![SyncActionResult {
            relative_path: String::new(),
            success: true,
            error: Some("sync already running; queued another run".to_string()),
        }]);
    }

    record_sync_progress(id, "准备同步", None);
    state.dirty_tasks.clear(id);
    let mut all_results = Vec::new();
    loop {
        match run_sync_now_once(state, id).await {
            Ok(mut results) => all_results.append(&mut results),
            Err(error) => {
                state.sync_runs.abort(id);
                finish_sync_progress(id);
                return Err(error);
            }
        }
        if !state.sync_runs.finish(id) {
            break;
        }
    }
    state.file_list_refresh.mark(id, "sync_completed");
    finish_sync_progress(id);
    Ok(all_results)
}

async fn run_sync_now_once(state: &AppState, id: Uuid) -> Result<Vec<SyncActionResult>, String> {
    let sync_start = Instant::now();
    record_sync_progress(id, "扫描本机", None);
    let (task, cached_snapshots, baselines, deferred_transfers) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;

        let task_repo = repository::SyncTaskRepository::new(&db);
        let task = task_repo
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?;
        if !task.enabled {
            return Err("task is paused".to_string());
        }

        let snap_repo = repository::FileSnapshotRepository::new(&db);
        let cached_snapshots = snap_repo.list_by_task(&id).map_err(|e| e.to_string())?;
        let baseline_repo = repository::SyncBaselineRepository::new(&db);
        let baselines = baseline_repo.list_by_task(&id).map_err(|e| e.to_string())?;
        let deferred_transfers = repository::DeferredTransferRepository::new(&db)
            .list_all()
            .map_err(|e| e.to_string())?;
        (task, cached_snapshots, baselines, deferred_transfers)
    };
    let deferred_transfers = deferred_transfer_set(&deferred_transfers, id);

    let sync_root = Path::new(&task.local_path);

    // Scan outside the SQLite lock; hashing can be the slowest local stage.
    let local_scan_start = Instant::now();
    let cached_snapshot_list = cached_snapshots;
    let cache = snapshot_cache_by_path(cached_snapshot_list.clone());
    let scan_results =
        guarded_scan_root_with_cache(id, sync_root, &*state.platform, &cache, "sync_now")?;
    let local_scan_ms = local_scan_start.elapsed().as_millis() as u64;
    let snapshots = scan_results
        .into_iter()
        .map(|result| {
            let mut snap = result.snapshot;
            snap.task_id = id;
            snap
        })
        .collect::<Vec<_>>();

    let local_snapshot_db_start = Instant::now();
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::FileSnapshotRepository::new(&db)
            .replace_for_task(&id, &snapshots)
            .map_err(|e| e.to_string())?;
    }
    let local_snapshot_db_ms = local_snapshot_db_start.elapsed().as_millis() as u64;

    let plan_start = Instant::now();
    let mut actions = planner::plan_sync(&snapshots, &baselines, task.local_role);
    if task.local_role == DeviceRole::Primary {
        let recovered_delete_count = append_recovered_delete_actions(
            &mut actions,
            &cached_snapshot_list,
            &snapshots,
            &baselines,
        );
        if recovered_delete_count > 0 {
            tracing::info!(
                sync_timing = true,
                task_id = %id,
                ready_reason = "safe_recovered_delete",
                recovered_delete_count
            );
        }
    }
    compress_directory_delete_actions(&mut actions);
    sort_actions_by_priority(&mut actions);
    record_sync_progress_totals(
        id,
        actions.len() as u64,
        actions.iter().map(action_progress_bytes).sum(),
    );
    let plan_ms = plan_start.elapsed().as_millis() as u64;

    let remote_scan_ms;
    let transfer_total_ms;
    let mut baseline_update_ms = 0;
    record_sync_progress(
        id,
        "请求对端状态",
        Some(format!("{} 个本机变更待检查", actions.len())),
    );
    let results = if task.local_role == DeviceRole::Primary {
        let connections = state.connections.clone();
        let sync_root = Path::new(&task.local_path);
        let remote_scan_start = Instant::now();
        let remote_scan = request_scan_with_retry(
            &connections,
            &state.identity,
            &task.secondary_device_id,
            task.id.to_string(),
        )
        .await;
        remote_scan_ms = remote_scan_start.elapsed().as_millis() as u64;
        let transfer_start = Instant::now();
        let mut network_results = match remote_scan {
            Ok(remote_files) => {
                let repaired_count = append_remote_missing_repair_actions(
                    &mut actions,
                    &snapshots,
                    &baselines,
                    &remote_files,
                );
                if repaired_count > 0 {
                    sort_actions_by_priority(&mut actions);
                    record_sync_progress_totals(
                        id,
                        actions.len() as u64,
                        actions.iter().map(action_progress_bytes).sum(),
                    );
                }
                record_sync_progress(id, "传输中", Some(format!("{} 个动作", actions.len())));
                execute_primary_actions_over_network(
                    &actions,
                    &task,
                    sync_root,
                    &connections,
                    &state.identity,
                    &remote_files,
                    &deferred_transfers,
                )
                .await
            }
            Err(e) => actions
                .iter()
                .map(|action| {
                    network_result(network_error(
                        &action.relative_path,
                        &format!("remote scan failed: {}", e),
                        true,
                    ))
                })
                .collect(),
        };
        transfer_total_ms = transfer_start.elapsed().as_millis() as u64;
        let baseline_update_start = Instant::now();
        let db = state.db.lock().map_err(|e| e.to_string())?;
        persist_network_successes(&actions, &task, &mut network_results, &db);
        baseline_update_ms = baseline_update_start.elapsed().as_millis() as u64;
        mark_dirty_if_directory_tree_changed_after_sync(
            state,
            id,
            sync_root,
            &snapshots,
            &actions,
            &network_results,
        );
        network_results
            .into_iter()
            .map(|network_result| network_result.result)
            .collect()
    } else {
        let connections = state.connections.clone();
        let sync_root = Path::new(&task.local_path);
        let remote_scan_start = Instant::now();
        let remote_scan = request_scan_with_retry(
            &connections,
            &state.identity,
            &task.primary_device_id,
            task.id.to_string(),
        )
        .await;
        remote_scan_ms = remote_scan_start.elapsed().as_millis() as u64;
        match remote_scan {
            Ok(mut remote_files) => {
                sort_remote_files_by_priority(&mut remote_files);
                record_sync_progress_totals(
                    id,
                    (actions.len() + remote_files.len()) as u64,
                    actions.iter().map(action_progress_bytes).sum::<u64>()
                        + remote_files.iter().map(remote_progress_bytes).sum::<u64>(),
                );
                record_sync_progress(
                    id,
                    "处理本机变更",
                    Some(format!("{} 个动作", actions.len())),
                );
                let transfer_start = Instant::now();
                let mut local_results = {
                    let db = state.db.lock().map_err(|e| e.to_string())?;
                    executor::execute_actions(&actions, &task, sync_root, &db)
                };
                for (action, result) in actions.iter().zip(local_results.iter()) {
                    if result.success {
                        advance_sync_progress(id, action_progress_bytes(action));
                    }
                }
                record_sync_progress(
                    id,
                    "拉取主机变更",
                    Some(format!("{} 个远端条目", remote_files.len())),
                );
                let mut pull_results = execute_secondary_pull_over_network(
                    &task,
                    sync_root,
                    &connections,
                    &state.identity,
                    &remote_files,
                    &snapshots,
                    &baselines,
                    &deferred_transfers,
                )
                .await;
                transfer_total_ms = transfer_start.elapsed().as_millis() as u64;
                let baseline_update_start = Instant::now();
                let db = state.db.lock().map_err(|e| e.to_string())?;
                persist_secondary_pull_successes(&task, &mut pull_results, sync_root, &db);
                baseline_update_ms = baseline_update_start.elapsed().as_millis() as u64;
                local_results.extend(
                    pull_results
                        .into_iter()
                        .map(|pull_result| pull_result.result),
                );
                let results = local_results;
                results
            }
            Err(e) => {
                let transfer_start = Instant::now();
                let mut local_results = {
                    let db = state.db.lock().map_err(|e| e.to_string())?;
                    executor::execute_actions(&actions, &task, sync_root, &db)
                };
                for (action, result) in actions.iter().zip(local_results.iter()) {
                    if result.success {
                        advance_sync_progress(id, action_progress_bytes(action));
                    }
                }
                transfer_total_ms = transfer_start.elapsed().as_millis() as u64;
                for (action, result) in actions.iter().zip(local_results.iter_mut()) {
                    if action.decision == SyncDecision::MarkPendingReturn && result.success {
                        result.success = false;
                        result.error = Some(format!("remote scan failed: {}", e));
                        result.retryable = true;
                    }
                }
                local_results
            }
        }
    };

    tracing::info!(
        sync_timing = true,
        task_id = %id,
        role = ?task.local_role,
        local_scan_ms,
        local_snapshot_db_ms,
        remote_scan_ms,
        plan_ms,
        actions_count = actions.len(),
        transfer_total_ms,
        baseline_update_ms,
        total_ms = sync_start.elapsed().as_millis() as u64,
    );

    Ok(results
        .into_iter()
        .map(|r| SyncActionResult {
            relative_path: r.relative_path,
            success: r.success,
            error: r.error,
        })
        .collect())
}

fn append_recovered_delete_actions(
    actions: &mut Vec<planner::PlannedAction>,
    cached_snapshots: &[FileSnapshot],
    current_snapshots: &[FileSnapshot],
    baselines: &[SyncBaseline],
) -> usize {
    let current_paths = current_snapshots
        .iter()
        .filter(|snapshot| !snapshot.deleted && !snapshot.is_symlink)
        .map(|snapshot| snapshot.relative_path.as_str())
        .collect::<HashSet<_>>();
    let baseline_paths = baselines
        .iter()
        .map(|baseline| baseline.relative_path.as_str())
        .collect::<HashSet<_>>();
    let action_paths = actions
        .iter()
        .map(|action| action.relative_path.clone())
        .collect::<HashSet<_>>();

    let mut added = 0;
    for snapshot in cached_snapshots {
        if snapshot.deleted
            || snapshot.is_symlink
            || current_paths.contains(snapshot.relative_path.as_str())
            || baseline_paths.contains(snapshot.relative_path.as_str())
            || action_paths.contains(&snapshot.relative_path)
        {
            continue;
        }
        actions.push(planner::PlannedAction {
            relative_path: snapshot.relative_path.clone(),
            decision: SyncDecision::MoveSecondaryToHistory,
            snapshot: Some(snapshot.clone()),
            baseline: None,
        });
        added += 1;
    }
    added
}

fn compress_directory_delete_actions(actions: &mut Vec<planner::PlannedAction>) {
    let directory_deletes = actions
        .iter()
        .filter(|action| {
            action.decision == SyncDecision::MoveSecondaryToHistory
                && action_is_directory_delete(action)
        })
        .map(|action| action.relative_path.clone())
        .collect::<Vec<_>>();
    if directory_deletes.is_empty() {
        return;
    }

    actions.retain(|action| {
        if action.decision != SyncDecision::MoveSecondaryToHistory {
            return true;
        }
        !directory_deletes.iter().any(|dir| {
            action.relative_path != *dir && is_descendant_path(&action.relative_path, dir)
        })
    });
}

fn action_is_directory_delete(action: &planner::PlannedAction) -> bool {
    matches!(
        action.snapshot.as_ref().map(|snapshot| snapshot.kind),
        Some(EntryKind::Directory)
    ) || action
        .baseline
        .as_ref()
        .is_some_and(baseline_looks_like_directory)
}

fn action_progress_bytes(action: &planner::PlannedAction) -> u64 {
    action
        .snapshot
        .as_ref()
        .filter(|snapshot| snapshot.kind == EntryKind::File)
        .map(|snapshot| snapshot.size.max(0) as u64)
        .unwrap_or(0)
}

fn append_remote_missing_repair_actions(
    actions: &mut Vec<planner::PlannedAction>,
    snapshots: &[FileSnapshot],
    baselines: &[SyncBaseline],
    remote_files: &[RemoteFileState],
) -> usize {
    let existing = actions
        .iter()
        .map(|action| action.relative_path.clone())
        .collect::<HashSet<_>>();
    let remote_paths = remote_files
        .iter()
        .map(|remote| remote.relative_path.clone())
        .collect::<HashSet<_>>();
    let snapshot_map = snapshots
        .iter()
        .filter(|snapshot| !snapshot.deleted && !snapshot.is_symlink)
        .map(|snapshot| (snapshot.relative_path.as_str(), snapshot))
        .collect::<HashMap<_, _>>();
    let mut added = 0;

    for baseline in baselines {
        if existing.contains(&baseline.relative_path)
            || remote_paths.contains(&baseline.relative_path)
        {
            continue;
        }
        let Some(snapshot) = snapshot_map.get(baseline.relative_path.as_str()) else {
            continue;
        };
        if !snapshot_matches_primary_baseline(snapshot, baseline) {
            continue;
        }
        actions.push(planner::PlannedAction {
            relative_path: baseline.relative_path.clone(),
            decision: SyncDecision::ApplyToSecondary,
            snapshot: Some((*snapshot).clone()),
            baseline: Some(baseline.clone()),
        });
        added += 1;
    }

    added
}

fn snapshot_matches_primary_baseline(snapshot: &FileSnapshot, baseline: &SyncBaseline) -> bool {
    if snapshot.kind == EntryKind::Directory {
        return baseline.primary_hash.is_none();
    }
    if snapshot.hash_status == HashStatus::Verified
        && baseline.primary_hash_status == HashStatus::Verified
    {
        return snapshot.blake3_hash == baseline.primary_hash;
    }
    snapshot.size == baseline.primary_size
        && snapshot.modified_unix_ms == baseline.primary_modified_unix_ms
}

fn remote_progress_bytes(remote: &RemoteFileState) -> u64 {
    if remote.kind == EntryKind::File {
        remote.size.max(0) as u64
    } else {
        0
    }
}

fn baseline_looks_like_directory(baseline: &SyncBaseline) -> bool {
    baseline.primary_hash.is_none()
        && baseline.secondary_hash.is_none()
        && baseline.primary_hash_status == HashStatus::Unavailable
        && baseline.secondary_hash_status == HashStatus::Unavailable
        && baseline.primary_size == 0
        && baseline.secondary_size == 0
}

fn mark_dirty_if_directory_tree_changed_after_sync(
    state: &AppState,
    task_id: Uuid,
    sync_root: &Path,
    original_snapshots: &[FileSnapshot],
    actions: &[planner::PlannedAction],
    results: &[NetworkActionResult],
) {
    let synced_dirs = actions
        .iter()
        .zip(results.iter())
        .filter_map(|(action, result)| {
            if result.result.success
                && action.decision == SyncDecision::ApplyToSecondary
                && matches!(
                    action.snapshot.as_ref().map(|snapshot| snapshot.kind),
                    Some(EntryKind::Directory)
                )
            {
                Some(action.relative_path.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if synced_dirs.is_empty() {
        return;
    }

    let original_paths = original_snapshots
        .iter()
        .filter(|snapshot| !snapshot.deleted && !snapshot.is_symlink)
        .map(|snapshot| snapshot.relative_path.as_str())
        .collect::<HashSet<_>>();
    let Ok(current_metadata) = scanner::scan_root_metadata(sync_root, &*state.platform) else {
        return;
    };
    let has_new_descendant = current_metadata.iter().any(|snapshot| {
        !original_paths.contains(snapshot.relative_path.as_str())
            && synced_dirs
                .iter()
                .any(|dir| is_descendant_path(&snapshot.relative_path, dir))
    });
    if has_new_descendant {
        tracing::info!(
            sync_timing = true,
            task_id = %task_id,
            ready_reason = "metadata_delta",
            directory_rescan_delta = true
        );
        state.dirty_tasks.mark_task_dirty(task_id);
    }
}

fn is_descendant_path(path: &str, parent: &str) -> bool {
    path.strip_prefix(parent)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn return_path_depth(path: &str) -> usize {
    path.split('/').filter(|part| !part.is_empty()).count()
}

/// Priority for scheduling transfer actions. Lower value = higher priority.
fn action_priority(action: &planner::PlannedAction) -> u8 {
    match action.decision {
        SyncDecision::MoveSecondaryToHistory => 0,
        SyncDecision::ApplyToSecondary => match action.snapshot.as_ref().map(|s| s.kind) {
            Some(EntryKind::Directory) => 0,
            _ if action.snapshot.as_ref().map_or(0, |s| s.size) <= 8 * 1024 * 1024 => 1,
            _ => 2,
        },
        _ => 3,
    }
}

/// Sort actions so deletes and directory creates run first,
/// then small files, then large files. Never reorders same relative_path.
fn sort_actions_by_priority(actions: &mut [planner::PlannedAction]) {
    actions.sort_by_key(action_priority);
}

/// Sort remote files for download: directories first, then small files, then large files.
fn sort_remote_files_by_priority(files: &mut [RemoteFileState]) {
    files.sort_by_key(|f| {
        if f.kind == EntryKind::Directory {
            0
        } else if f.size <= 8 * 1024 * 1024 {
            1
        } else {
            2
        }
    });
}

async fn request_scan_with_retry(
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
    peer_device_id: &str,
    task_id: String,
) -> anyhow::Result<Vec<RemoteFileState>> {
    let mut last_error = None;
    for attempt in 0..MAX_NETWORK_ATTEMPTS {
        match connection::request_authenticated_scan(
            connections,
            local_identity,
            peer_device_id,
            task_id.clone(),
        )
        .await
        {
            Ok(files) => return Ok(files),
            Err(error) => {
                let retryable = is_retryable_network_error(&error);
                last_error = Some(error);
                if retryable && attempt + 1 < MAX_NETWORK_ATTEMPTS {
                    sleep_before_retry(attempt).await;
                    continue;
                }
                break;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("remote scan failed")))
}

async fn execute_primary_actions_over_network(
    actions: &[planner::PlannedAction],
    task: &SyncTask,
    sync_root: &Path,
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
    remote_files: &[RemoteFileState],
    deferred_transfers: &HashSet<String>,
) -> Vec<NetworkActionResult> {
    let remote_map = remote_files
        .iter()
        .map(|file| (file.relative_path.as_str(), file))
        .collect::<HashMap<_, _>>();
    let mut results = Vec::new();
    for action in actions {
        let result = match action.decision {
            SyncDecision::ApplyToSecondary => match remote_conflict(action, &remote_map) {
                Some(error) => network_result(executor::ExecutionResult {
                    relative_path: action.relative_path.clone(),
                    success: false,
                    error: Some(error),
                    retryable: false,
                }),
                None => match action.snapshot.as_ref().map(|snapshot| snapshot.kind) {
                    Some(EntryKind::Directory) => network_result(
                        send_directory_action(
                            action,
                            &task.secondary_device_id,
                            task.id,
                            connections,
                            local_identity,
                        )
                        .await,
                    ),
                    _ if is_path_deferred(deferred_transfers, &action.relative_path, "upload") => {
                        network_result(network_error(
                            &action.relative_path,
                            "transfer deferred by user",
                            false,
                        ))
                    }
                    _ => {
                        send_file_action(action, task, sync_root, connections, local_identity).await
                    }
                },
            },
            SyncDecision::MoveSecondaryToHistory => {
                match remote_delete_safety_error(action, &remote_map) {
                    Some(error) => {
                        network_result(network_error(&action.relative_path, &error, false))
                    }
                    None => network_result(
                        send_delete_action(action, task, connections, local_identity).await,
                    ),
                }
            }
            SyncDecision::RequireConflictDecision => network_result(executor::ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: false,
                error: Some("conflict requires user decision".to_string()),
                retryable: false,
            }),
            SyncDecision::KeepBoth | SyncDecision::MarkPendingReturn => {
                network_result(executor::ExecutionResult {
                    relative_path: action.relative_path.clone(),
                    success: false,
                    error: Some("unsupported network action for primary sync".to_string()),
                    retryable: false,
                })
            }
            SyncDecision::Noop => network_result(executor::ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: true,
                error: None,
                retryable: false,
            }),
        };
        if result.result.success {
            advance_sync_progress(task.id, action_progress_bytes(action));
        }
        results.push(result);
    }
    results
}

fn remote_delete_safety_error(
    action: &planner::PlannedAction,
    remote_map: &HashMap<&str, &RemoteFileState>,
) -> Option<String> {
    let Some(remote) = remote_map.get(action.relative_path.as_str()) else {
        return None;
    };

    if let Some(baseline) = &action.baseline {
        return if remote_matches_baseline_for_delete(remote, baseline) {
            None
        } else {
            Some("remote changed since last sync".to_string())
        };
    }

    let Some(snapshot) = &action.snapshot else {
        return Some("delete requires baseline or verified previous snapshot".to_string());
    };
    if snapshot.kind != remote.kind {
        return Some("remote changed since last sync".to_string());
    }
    if snapshot.kind == EntryKind::Directory {
        return Some("directory delete requires baseline".to_string());
    }
    if snapshot.hash_status == HashStatus::Verified
        && remote.hash_status == HashStatus::Verified
        && snapshot.blake3_hash == remote.blake3_hash
    {
        None
    } else {
        Some("remote changed since last sync".to_string())
    }
}

fn remote_matches_baseline_for_delete(remote: &RemoteFileState, baseline: &SyncBaseline) -> bool {
    if baseline_looks_like_directory(baseline) {
        return remote.kind == EntryKind::Directory;
    }
    if remote.kind != EntryKind::File {
        return false;
    }

    if baseline.secondary_hash_status == HashStatus::Verified && baseline.secondary_hash.is_some() {
        return remote.hash_status != HashStatus::Verified
            || baseline.secondary_hash == remote.blake3_hash;
    }

    baseline.secondary_hash_status != HashStatus::Verified
        && remote.hash_status != HashStatus::Verified
        && baseline.secondary_size == remote.size
        && baseline.secondary_modified_unix_ms == remote.modified_unix_ms
}

fn remote_conflict(
    action: &planner::PlannedAction,
    remote_map: &HashMap<&str, &RemoteFileState>,
) -> Option<String> {
    let remote = remote_map.get(action.relative_path.as_str())?;
    if matches!(
        action.snapshot.as_ref().map(|snapshot| snapshot.kind),
        Some(EntryKind::Directory)
    ) {
        return if remote.kind == EntryKind::Directory {
            None
        } else {
            Some("remote path exists and is not a directory".to_string())
        };
    }
    match &action.baseline {
        Some(baseline) if baseline.secondary_hash == remote.blake3_hash => None,
        Some(_) => Some("remote file changed since last sync".to_string()),
        None => Some("remote file already exists".to_string()),
    }
}

async fn send_directory_action(
    action: &planner::PlannedAction,
    peer_device_id: &str,
    task_id: Uuid,
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
) -> executor::ExecutionResult {
    let msg = SyncMessage::DirectoryCreate {
        task_id: task_id.to_string(),
        relative_path: action.relative_path.clone(),
    };
    expect_file_ack(
        peer_device_id,
        &action.relative_path,
        connections,
        local_identity,
        msg,
    )
    .await
}

async fn send_file_action(
    action: &planner::PlannedAction,
    task: &SyncTask,
    sync_root: &Path,
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
) -> NetworkActionResult {
    if action.snapshot.is_none() {
        return network_result(network_error(
            &action.relative_path,
            "no snapshot for apply action",
            false,
        ));
    }

    let source = sync_root.join(&action.relative_path);
    match send_file_with_retry(
        connections,
        local_identity,
        &task.secondary_device_id,
        task.id.to_string(),
        action.relative_path.clone(),
        &source,
    )
    .await
    {
        Ok(outcome) => NetworkActionResult {
            result: executor::ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: true,
                error: None,
                retryable: false,
            },
            transferred_hash: Some(outcome.blake3_hash),
        },
        Err(e) => {
            let retryable = is_retryable_network_error(&e);
            network_result(network_error(
                &action.relative_path,
                &format!("network file transfer failed: {}", e),
                retryable,
            ))
        }
    }
}

async fn send_file_with_retry(
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
    peer_device_id: &str,
    task_id: String,
    relative_path: String,
    source: &Path,
) -> anyhow::Result<connection::FileTransferOutcome> {
    connection::clear_transfer_cancel(&task_id, &relative_path, Some("upload"));
    if connection::is_transfer_deferred(&task_id, &relative_path, "upload") {
        return Err(anyhow::anyhow!("transfer deferred by user"));
    }

    let mut last_error = None;
    for attempt in 0..MAX_NETWORK_ATTEMPTS {
        let transfer_result = match connection::wait_for_source_file_stability(source).await {
            Ok(_) => {
                connection::send_authenticated_file_to_peer(
                    connections,
                    local_identity,
                    peer_device_id,
                    task_id.clone(),
                    relative_path.clone(),
                    source,
                )
                .await
            }
            Err(error) => Err(error),
        };

        match transfer_result {
            Ok(outcome) => {
                return Ok(outcome);
            }
            Err(e) => {
                if connection::is_transfer_cancelled(&task_id, &relative_path, "upload") {
                    return Err(e);
                }
                let retryable = is_retryable_network_error(&e);
                last_error = Some(e);
                if retryable && attempt + 1 < MAX_NETWORK_ATTEMPTS {
                    sleep_before_retry(attempt).await;
                    continue;
                }
                break;
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("network file transfer failed")))
}

async fn download_file_with_retry(
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
    peer_device_id: &str,
    task_id: String,
    relative_path: String,
    target: &Path,
    _total_bytes: u64,
) -> anyhow::Result<connection::FileTransferOutcome> {
    connection::clear_transfer_cancel(&task_id, &relative_path, Some("download"));
    if connection::is_transfer_deferred(&task_id, &relative_path, "download") {
        return Err(anyhow::anyhow!("transfer deferred by user"));
    }

    let mut last_error = None;
    for attempt in 0..MAX_NETWORK_ATTEMPTS {
        match connection::request_authenticated_file_from_peer(
            connections,
            local_identity,
            peer_device_id,
            task_id.clone(),
            relative_path.clone(),
            target,
        )
        .await
        {
            Ok(outcome) => {
                return Ok(outcome);
            }
            Err(error) => {
                if connection::is_transfer_cancelled(&task_id, &relative_path, "download") {
                    return Err(error);
                }
                let retryable = is_retryable_network_error(&error);
                last_error = Some(error);
                if retryable && attempt + 1 < MAX_NETWORK_ATTEMPTS {
                    sleep_before_retry(attempt).await;
                    continue;
                }
                break;
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("network file download failed")))
}

async fn send_delete_action(
    action: &planner::PlannedAction,
    task: &SyncTask,
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
) -> executor::ExecutionResult {
    send_delete_to_peer(
        &task.secondary_device_id,
        task.id,
        &action.relative_path,
        delete_expectation_from_action(action),
        connections,
        local_identity,
    )
    .await
}

async fn send_delete_to_peer(
    peer_device_id: &str,
    task_id: Uuid,
    relative_path: &str,
    expectation: Option<DeleteExpectation>,
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
) -> executor::ExecutionResult {
    let delete_batch_id = expectation
        .as_ref()
        .map(|_| format!("{}-{}", now_ms(), Uuid::new_v4()));
    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        peer_device_id = %peer_device_id,
        has_expectation = expectation.is_some(),
        "send delete to peer"
    );
    let msg = SyncMessage::FileDelete {
        task_id: task_id.to_string(),
        relative_path: relative_path.to_string(),
        expected_kind: expectation.as_ref().map(|expected| expected.expected_kind),
        expected_hash: expectation
            .as_ref()
            .and_then(|expected| expected.expected_hash.clone()),
        expected_hash_status: expectation
            .as_ref()
            .map(|expected| expected.expected_hash_status),
        expected_size: expectation.as_ref().map(|expected| expected.expected_size),
        expected_modified_unix_ms: expectation
            .as_ref()
            .map(|expected| expected.expected_modified_unix_ms),
        delete_batch_id,
    };
    let result = expect_file_ack(
        peer_device_id,
        relative_path,
        connections,
        local_identity,
        msg,
    )
    .await;
    tracing::info!(
        task_id = %task_id,
        relative_path = %relative_path,
        peer_device_id = %peer_device_id,
        success = result.success,
        error = result.error.as_deref().unwrap_or(""),
        "delete peer response"
    );
    result
}

fn delete_expectation_from_action(action: &planner::PlannedAction) -> Option<DeleteExpectation> {
    if let Some(baseline) = &action.baseline {
        let expected_kind = if baseline_looks_like_directory(baseline) {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        return Some(DeleteExpectation {
            expected_kind,
            expected_hash: baseline.secondary_hash.clone(),
            expected_hash_status: baseline.secondary_hash_status,
            expected_size: baseline.secondary_size,
            expected_modified_unix_ms: baseline.secondary_modified_unix_ms,
        });
    }
    let snapshot = action.snapshot.as_ref()?;
    Some(DeleteExpectation {
        expected_kind: snapshot.kind,
        expected_hash: snapshot.blake3_hash.clone(),
        expected_hash_status: snapshot.hash_status,
        expected_size: snapshot.size,
        expected_modified_unix_ms: snapshot.modified_unix_ms,
    })
}

async fn expect_file_ack(
    peer_device_id: &str,
    relative_path: &str,
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
    msg: SyncMessage,
) -> executor::ExecutionResult {
    match connection::send_authenticated_message_to_peer(
        connections,
        local_identity,
        peer_device_id,
        msg,
    )
    .await
    {
        Ok(SyncMessage::FileAck {
            relative_path: ack_path,
            success,
            ..
        }) if success => executor::ExecutionResult {
            relative_path: ack_path,
            success: true,
            error: None,
            retryable: false,
        },
        Ok(SyncMessage::FileAck { error, .. }) => executor::ExecutionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some(error.unwrap_or_else(|| "peer rejected file operation".to_string())),
            retryable: true,
        },
        Ok(other) => executor::ExecutionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some(format!("unexpected peer response: {:?}", other)),
            retryable: true,
        },
        Err(e) => network_error(relative_path, &format!("network send failed: {}", e), true),
    }
}

fn persist_network_successes(
    actions: &[planner::PlannedAction],
    task: &SyncTask,
    results: &mut [NetworkActionResult],
    db: &rusqlite::Connection,
) {
    let now = now_ms();
    let baseline_repo = repository::SyncBaselineRepository::new(db);
    let snap_repo = repository::FileSnapshotRepository::new(db);

    for (action, network_result) in actions.iter().zip(results.iter_mut()) {
        let result = &mut network_result.result;
        if !result.success {
            continue;
        }
        match action.decision {
            SyncDecision::ApplyToSecondary => {
                let Some(snap) = &action.snapshot else {
                    continue;
                };
                if let Err(e) = ensure_parent_directory_baselines(
                    &baseline_repo,
                    task.id,
                    &action.relative_path,
                    now,
                ) {
                    result.success = false;
                    result.error = Some(format!("parent baseline update failed: {}", e));
                    result.retryable = true;
                    continue;
                }
                let (hash, hash_status) = verified_hash_for_successful_apply(
                    task,
                    action,
                    snap,
                    network_result.transferred_hash.as_deref(),
                );
                let baseline = SyncBaseline {
                    task_id: task.id,
                    relative_path: action.relative_path.clone(),
                    primary_hash: hash.clone(),
                    primary_hash_status: hash_status,
                    primary_size: snap.size,
                    secondary_size: snap.size,
                    primary_modified_unix_ms: snap.modified_unix_ms,
                    secondary_hash: hash,
                    secondary_hash_status: hash_status,
                    secondary_modified_unix_ms: snap.modified_unix_ms,
                    last_synced_unix_ms: now,
                };
                if let Err(e) = baseline_repo.upsert(&baseline) {
                    result.success = false;
                    result.error = Some(format!("baseline update failed: {}", e));
                    result.retryable = true;
                }
            }
            SyncDecision::MoveSecondaryToHistory => {
                if let Err(e) = snap_repo.remove_tree(&task.id, &action.relative_path) {
                    result.success = false;
                    result.error = Some(format!("snapshot delete cleanup failed: {}", e));
                    result.retryable = true;
                    continue;
                }
                if let Err(e) = baseline_repo.remove_tree(&task.id, &action.relative_path) {
                    result.success = false;
                    result.error = Some(format!("baseline remove failed: {}", e));
                    result.retryable = true;
                }
            }
            _ => {}
        }
    }
}

fn ensure_parent_directory_baselines(
    baseline_repo: &repository::SyncBaselineRepository<'_>,
    task_id: Uuid,
    relative_path: &str,
    now: i64,
) -> anyhow::Result<()> {
    for parent in parent_directory_paths(relative_path) {
        baseline_repo.upsert(&directory_baseline(task_id, parent, now))?;
    }
    Ok(())
}

fn parent_directory_paths(relative_path: &str) -> Vec<String> {
    let parts = relative_path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() <= 1 {
        return Vec::new();
    }
    let mut parents = Vec::new();
    for depth in 1..parts.len() {
        parents.push(parts[..depth].join("/"));
    }
    parents
}

fn directory_baseline(task_id: Uuid, relative_path: String, now: i64) -> SyncBaseline {
    SyncBaseline {
        task_id,
        relative_path,
        primary_hash: None,
        primary_hash_status: HashStatus::Unavailable,
        primary_size: 0,
        secondary_size: 0,
        primary_modified_unix_ms: 0,
        secondary_hash: None,
        secondary_hash_status: HashStatus::Unavailable,
        secondary_modified_unix_ms: 0,
        last_synced_unix_ms: now,
    }
}

fn verified_hash_for_successful_apply(
    task: &SyncTask,
    action: &planner::PlannedAction,
    snap: &FileSnapshot,
    transferred_hash: Option<&str>,
) -> (Option<String>, HashStatus) {
    if let Some(hash) = transferred_hash {
        return (Some(hash.to_string()), HashStatus::Verified);
    }
    if snap.hash_status == HashStatus::Verified {
        return (snap.blake3_hash.clone(), snap.hash_status);
    }
    let source = Path::new(&task.local_path).join(&action.relative_path);
    match scanner::hash_file(&source) {
        Ok(hash) => (Some(hash), HashStatus::Verified),
        Err(_) => (snap.blake3_hash.clone(), snap.hash_status),
    }
}

async fn execute_secondary_pull_over_network(
    task: &SyncTask,
    sync_root: &Path,
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
    remote_files: &[RemoteFileState],
    local_snapshots: &[FileSnapshot],
    baselines: &[SyncBaseline],
    deferred_transfers: &HashSet<String>,
) -> Vec<PullActionResult> {
    let local_map = local_snapshots
        .iter()
        .map(|snapshot| (snapshot.relative_path.as_str(), snapshot))
        .collect::<HashMap<_, _>>();
    let baseline_map = baselines
        .iter()
        .map(|baseline| (baseline.relative_path.as_str(), baseline))
        .collect::<HashMap<_, _>>();
    let mut results = Vec::new();

    for remote in remote_files {
        if !secondary_should_download(remote, &local_map, &baseline_map) {
            continue;
        }
        if let Some(error) = secondary_pull_conflict(remote, &local_map, &baseline_map) {
            results.push(pull_result(network_error(
                &remote.relative_path,
                &error,
                false,
            )));
            continue;
        }

        let target = match path_safety::safe_join(sync_root, &remote.relative_path) {
            Ok(target) => target,
            Err(e) => {
                results.push(pull_result(network_error(
                    &remote.relative_path,
                    &format!("invalid remote path: {}", e),
                    false,
                )));
                continue;
            }
        };
        if remote.kind == EntryKind::Directory {
            match std::fs::create_dir_all(&target) {
                Ok(()) => {
                    advance_sync_progress(task.id, 0);
                    results.push(pull_result(executor::ExecutionResult {
                        relative_path: remote.relative_path.clone(),
                        success: true,
                        error: None,
                        retryable: false,
                    }));
                }
                Err(e) => results.push(pull_result(network_error(
                    &remote.relative_path,
                    &format!("directory create failed: {}", e),
                    true,
                ))),
            }
            continue;
        }

        if is_path_deferred(deferred_transfers, &remote.relative_path, "download") {
            results.push(pull_result(network_error(
                &remote.relative_path,
                "transfer deferred by user",
                false,
            )));
            continue;
        }

        match download_file_with_retry(
            connections,
            local_identity,
            &task.primary_device_id,
            task.id.to_string(),
            remote.relative_path.clone(),
            &target,
            remote.size.max(0) as u64,
        )
        .await
        {
            Ok(outcome) => {
                advance_sync_progress(task.id, remote_progress_bytes(remote));
                results.push(PullActionResult {
                    result: executor::ExecutionResult {
                        relative_path: remote.relative_path.clone(),
                        success: true,
                        error: None,
                        retryable: false,
                    },
                    transferred_hash: Some(outcome.blake3_hash),
                });
            }
            Err(e) => {
                let retryable = is_retryable_network_error(&e);
                results.push(pull_result(network_error(
                    &remote.relative_path,
                    &format!("network file download failed: {}", e),
                    retryable,
                )));
            }
        }
    }

    results
}

fn secondary_should_download(
    remote: &RemoteFileState,
    local_map: &HashMap<&str, &FileSnapshot>,
    baseline_map: &HashMap<&str, &SyncBaseline>,
) -> bool {
    if remote.kind == EntryKind::Directory {
        return match local_map.get(remote.relative_path.as_str()) {
            Some(local) => local.kind != EntryKind::Directory,
            None => !matches!(
                baseline_map.get(remote.relative_path.as_str()),
                Some(baseline)
                    if baseline.primary_hash.is_none() && baseline.secondary_hash.is_none()
            ),
        };
    }
    match baseline_map.get(remote.relative_path.as_str()) {
        Some(baseline) if baseline.primary_hash == remote.blake3_hash => false,
        _ => match local_map.get(remote.relative_path.as_str()) {
            Some(local) => local.blake3_hash != remote.blake3_hash,
            None => true,
        },
    }
}

fn secondary_pull_conflict(
    remote: &RemoteFileState,
    local_map: &HashMap<&str, &FileSnapshot>,
    baseline_map: &HashMap<&str, &SyncBaseline>,
) -> Option<String> {
    let local = local_map.get(remote.relative_path.as_str())?;
    if remote.kind == EntryKind::Directory {
        return if local.kind == EntryKind::Directory {
            None
        } else {
            Some("local path exists and is not a directory".to_string())
        };
    }
    match baseline_map.get(remote.relative_path.as_str()) {
        Some(baseline) if local.blake3_hash == baseline.secondary_hash => None,
        Some(_) => Some("local file changed since last sync".to_string()),
        None => Some("local file already exists".to_string()),
    }
}

fn persist_secondary_pull_successes(
    task: &SyncTask,
    results: &mut [PullActionResult],
    sync_root: &Path,
    db: &rusqlite::Connection,
) {
    let now = now_ms();
    let snap_repo = repository::FileSnapshotRepository::new(db);
    let baseline_repo = repository::SyncBaselineRepository::new(db);
    let pending_repo = repository::PendingReturnRepository::new(db);

    for pull_result in results {
        let result = &mut pull_result.result;
        if !result.success {
            continue;
        }
        let path = sync_root.join(&result.relative_path);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) => {
                result.success = false;
                result.error = Some(format!("pulled file metadata failed: {}", e));
                result.retryable = true;
                continue;
            }
        };
        let (kind, size, hash, hash_status) = if metadata.is_dir() {
            (EntryKind::Directory, 0, None, HashStatus::Unavailable)
        } else if let Some(hash) = pull_result.transferred_hash.as_ref() {
            (
                EntryKind::File,
                metadata.len() as i64,
                Some(hash.clone()),
                HashStatus::Verified,
            )
        } else {
            match crate::core::scanner::hash_file(&path) {
                Ok(hash) => (
                    EntryKind::File,
                    metadata.len() as i64,
                    Some(hash),
                    HashStatus::Verified,
                ),
                Err(e) => {
                    result.success = false;
                    result.error = Some(format!("pulled file hash failed: {}", e));
                    result.retryable = true;
                    continue;
                }
            }
        };
        let modified_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(now);
        let snapshot = FileSnapshot {
            task_id: task.id,
            relative_path: result.relative_path.clone(),
            kind,
            size,
            modified_unix_ms,
            blake3_hash: hash.clone(),
            hash_status,
            deleted: false,
            is_symlink: false,
        };
        if let Err(e) = snap_repo.upsert(&snapshot) {
            result.success = false;
            result.error = Some(format!("pulled snapshot update failed: {}", e));
            result.retryable = true;
            continue;
        }
        if let Err(e) =
            ensure_parent_directory_baselines(&baseline_repo, task.id, &result.relative_path, now)
        {
            result.success = false;
            result.error = Some(format!("pulled parent baseline update failed: {}", e));
            result.retryable = true;
            continue;
        }
        if let Err(e) = baseline_repo.upsert(&SyncBaseline {
            task_id: task.id,
            relative_path: result.relative_path.clone(),
            primary_hash: hash.clone(),
            primary_hash_status: hash_status,
            primary_size: size,
            secondary_size: size,
            primary_modified_unix_ms: modified_unix_ms,
            secondary_hash: hash,
            secondary_hash_status: hash_status,
            secondary_modified_unix_ms: modified_unix_ms,
            last_synced_unix_ms: now,
        }) {
            result.success = false;
            result.error = Some(format!("pulled baseline update failed: {}", e));
            result.retryable = true;
            continue;
        }
        if let Err(e) = pending_repo.remove(&task.id, &result.relative_path) {
            result.success = false;
            result.error = Some(format!("pulled pending cleanup failed: {}", e));
            result.retryable = true;
        }
    }
}

fn network_error(relative_path: &str, error: &str, retryable: bool) -> executor::ExecutionResult {
    executor::ExecutionResult {
        relative_path: relative_path.to_string(),
        success: false,
        error: Some(error.to_string()),
        retryable,
    }
}

// ─── Pending Return Changes ───

#[tauri::command]
pub fn list_pending_returns(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<PendingReturnChange>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::PendingReturnRepository::new(&db);
    repo.list_by_task(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_pending_count(state: State<'_, AppState>, task_id: String) -> Result<i64, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::PendingReturnRepository::new(&db);
    repo.count_by_task(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn refresh_pending_returns(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<SyncActionResult>, String> {
    run_refresh_pending_returns(state.inner(), task_id)
}

pub fn run_refresh_pending_returns(
    state: &AppState,
    task_id: String,
) -> Result<Vec<SyncActionResult>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let (task, cached_snapshots, baselines) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let task = repository::SyncTaskRepository::new(&db)
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?;
        let cached_snapshots = repository::FileSnapshotRepository::new(&db)
            .list_by_task(&id)
            .map_err(|e| e.to_string())?;
        let baselines = repository::SyncBaselineRepository::new(&db)
            .list_by_task(&id)
            .map_err(|e| e.to_string())?;
        (task, cached_snapshots, baselines)
    };

    if !task.enabled {
        return Err("task is paused".to_string());
    }
    if task.local_role != DeviceRole::Secondary {
        return Ok(Vec::new());
    }

    let sync_root = Path::new(&task.local_path);
    let cache = snapshot_cache_by_path(cached_snapshots);
    let scan_results = guarded_scan_root_with_cache(
        id,
        sync_root,
        &*state.platform,
        &cache,
        "refresh_pending_returns",
    )?;
    let snapshots = scan_results
        .into_iter()
        .map(|result| {
            let mut snap = result.snapshot;
            snap.task_id = id;
            snap
        })
        .collect::<Vec<_>>();

    let db = state.db.lock().map_err(|e| e.to_string())?;
    repository::FileSnapshotRepository::new(&db)
        .replace_for_task(&id, &snapshots)
        .map_err(|e| e.to_string())?;

    let mut actions = planner::plan_sync(&snapshots, &baselines, DeviceRole::Secondary)
        .into_iter()
        .filter(|action| action.decision == SyncDecision::MarkPendingReturn)
        .collect::<Vec<_>>();
    sort_actions_by_priority(&mut actions);

    Ok(executor::execute_actions(&actions, &task, sync_root, &db)
        .into_iter()
        .map(|r| SyncActionResult {
            relative_path: r.relative_path,
            success: r.success,
            error: r.error,
        })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct ReturnSyncResult {
    pub relative_path: String,
    pub success: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn execute_return_sync(
    state: State<'_, AppState>,
    task_id: String,
    selected_paths: Vec<String>,
) -> Result<Vec<ReturnSyncResult>, String> {
    run_execute_return_sync(state.inner(), task_id, selected_paths).await
}

pub async fn run_execute_return_sync(
    state: &AppState,
    task_id: String,
    selected_paths: Vec<String>,
) -> Result<Vec<ReturnSyncResult>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    tracing::info!(
        task_id = %task_id,
        selected_count = selected_paths.len(),
        "execute_return_sync start"
    );
    let task = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let task_repo = repository::SyncTaskRepository::new(&db);
        task_repo
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?
    };

    let results = if task.local_role == DeviceRole::Secondary {
        let (pending, baselines, deferred_transfers) = {
            let db = state.db.lock().map_err(|e| e.to_string())?;
            let pending_repo = repository::PendingReturnRepository::new(&db);
            let baseline_repo = repository::SyncBaselineRepository::new(&db);
            (
                pending_repo.list_by_task(&id).map_err(|e| e.to_string())?,
                baseline_repo.list_by_task(&id).map_err(|e| e.to_string())?,
                repository::DeferredTransferRepository::new(&db)
                    .list_all()
                    .map_err(|e| e.to_string())?,
            )
        };
        let deferred_transfers = deferred_transfer_set(&deferred_transfers, id);
        let pending_map: HashMap<String, PendingReturnChange> = pending
            .into_iter()
            .map(|change| (change.relative_path.clone(), change))
            .collect();
        let selected_paths = expand_selected_return_paths(&selected_paths, &pending_map);
        let connections = state.connections.clone();
        let sync_root = Path::new(&task.local_path);
        let remote_files = request_scan_with_retry(
            &connections,
            &state.identity,
            &task.primary_device_id,
            task.id.to_string(),
        )
        .await
        .map_err(|e| format!("remote scan failed: {}", e))?;
        let mut results = execute_secondary_return_over_network_checked(
            &task,
            &selected_paths,
            &pending_map,
            sync_root,
            &connections,
            &state.identity,
            &remote_files,
            &baselines,
            &deferred_transfers,
        )
        .await;
        let db = state.db.lock().map_err(|e| e.to_string())?;
        persist_return_successes(&task, &pending_map, &mut results, sync_root, &db);
        results
    } else {
        let sync_root = std::path::Path::new(&task.local_path);

        // Refresh current primary snapshots before conflict detection.
        let all_snaps = refresh_task_snapshots(state, &task)?;
        let primary_map: HashMap<String, FileSnapshot> = all_snaps
            .into_iter()
            .map(|s| (s.relative_path.clone(), s))
            .collect();

        let db = state.db.lock().map_err(|e| e.to_string())?;
        // Get baselines
        let baseline_repo = repository::SyncBaselineRepository::new(&db);
        let mut baseline_map = HashMap::new();
        for path in &selected_paths {
            if let Some(b) = baseline_repo.get(&id, path).map_err(|e| e.to_string())? {
                baseline_map.insert(path.clone(), b);
            }
        }

        executor::execute_return_sync(
            &task,
            &selected_paths,
            &primary_map,
            &baseline_map,
            sync_root,
            &db,
        )
    };

    tracing::info!(
        task_id = %task_id,
        success_count = results.iter().filter(|result| result.success).count(),
        failure_count = results.iter().filter(|result| !result.success).count(),
        "execute_return_sync complete"
    );

    if results.iter().any(|result| result.success) {
        state.file_list_refresh.mark(id, "sync_completed");
    }

    Ok(results
        .into_iter()
        .map(|r| ReturnSyncResult {
            relative_path: r.relative_path,
            success: r.success,
            error: r.error,
        })
        .collect())
}

fn expand_selected_return_paths(
    selected_paths: &[String],
    pending: &HashMap<String, PendingReturnChange>,
) -> Vec<String> {
    let mut expanded = HashSet::new();

    for selected in selected_paths {
        if pending.contains_key(selected) {
            expanded.insert(selected.clone());
        }
        if pending
            .keys()
            .any(|path| is_descendant_path(path, selected))
        {
            expanded.insert(selected.clone());
        }
        for path in pending.keys() {
            if is_descendant_path(path, selected) {
                expanded.insert(path.clone());
            }
        }
    }

    let mut paths = expanded.into_iter().collect::<Vec<_>>();
    sort_return_paths_for_execution(&mut paths, pending);
    paths
}

fn sort_return_paths_for_execution(
    paths: &mut [String],
    pending: &HashMap<String, PendingReturnChange>,
) {
    paths.sort_by(|left, right| {
        let left_deleted = return_path_is_deleted_group(left, pending);
        let right_deleted = return_path_is_deleted_group(right, pending);
        let left_depth = return_path_depth(left);
        let right_depth = return_path_depth(right);

        match (left_deleted, right_deleted) {
            (true, true) => right_depth.cmp(&left_depth).then_with(|| left.cmp(right)),
            (false, false) => left_depth.cmp(&right_depth).then_with(|| left.cmp(right)),
            (false, true) => std::cmp::Ordering::Less,
            (true, false) => std::cmp::Ordering::Greater,
        }
    });
}

fn return_path_is_deleted_group(
    path: &str,
    pending: &HashMap<String, PendingReturnChange>,
) -> bool {
    if let Some(change) = pending.get(path) {
        return change.change_kind == ChangeKind::Deleted;
    }
    let descendants = pending
        .iter()
        .filter(|(candidate, _)| is_descendant_path(candidate, path))
        .map(|(_, change)| change)
        .collect::<Vec<_>>();
    !descendants.is_empty()
        && descendants
            .iter()
            .all(|change| change.change_kind == ChangeKind::Deleted)
}

async fn execute_secondary_return_over_network_checked(
    task: &SyncTask,
    selected_paths: &[String],
    pending: &HashMap<String, PendingReturnChange>,
    sync_root: &Path,
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
    remote_files: &[RemoteFileState],
    baselines: &[SyncBaseline],
    deferred_transfers: &HashSet<String>,
) -> Vec<executor::ExecutionResult> {
    let remote_map = remote_files
        .iter()
        .map(|file| (file.relative_path.as_str(), file))
        .collect::<HashMap<_, _>>();
    let baseline_map = baselines
        .iter()
        .map(|baseline| (baseline.relative_path.as_str(), baseline))
        .collect::<HashMap<_, _>>();

    let mut results = Vec::new();
    for path in selected_paths {
        let Some(change) = pending.get(path) else {
            let descendant_changes = pending
                .iter()
                .filter(|(candidate, _)| is_descendant_path(candidate, path))
                .map(|(_, change)| change)
                .collect::<Vec<_>>();
            if !descendant_changes.is_empty()
                && descendant_changes
                    .iter()
                    .all(|change| change.change_kind == ChangeKind::Deleted)
            {
                let result = send_delete_to_peer(
                    &task.primary_device_id,
                    task.id,
                    path,
                    None,
                    connections,
                    local_identity,
                )
                .await;
                results.push(result);
                continue;
            }
            if !descendant_changes.is_empty() {
                let planned = planner::PlannedAction {
                    relative_path: path.clone(),
                    decision: SyncDecision::ApplyToSecondary,
                    snapshot: None,
                    baseline: None,
                };
                results.push(
                    send_directory_action(
                        &planned,
                        &task.primary_device_id,
                        task.id,
                        connections,
                        local_identity,
                    )
                    .await,
                );
                continue;
            }
            tracing::warn!(
                task_id = %task.id,
                relative_path = %path,
                "return-sync selected path missing from pending map"
            );
            results.push(network_error(
                path,
                "pending change not found in database",
                false,
            ));
            continue;
        };
        tracing::info!(
            task_id = %task.id,
            relative_path = %path,
            change_kind = ?change.change_kind,
            "secondary return item start"
        );
        if let Some(error) = secondary_return_conflict(path, change, &remote_map, &baseline_map) {
            tracing::warn!(
                task_id = %task.id,
                relative_path = %path,
                error = %error,
                "secondary return item blocked by conflict"
            );
            results.push(network_error(path, &error, false));
            continue;
        }

        if change.change_kind == ChangeKind::Deleted {
            let result = send_delete_to_peer(
                &task.primary_device_id,
                task.id,
                path,
                None,
                connections,
                local_identity,
            )
            .await;
            tracing::info!(
                task_id = %task.id,
                relative_path = %path,
                success = result.success,
                error = result.error.as_deref().unwrap_or(""),
                "secondary return delete ack"
            );
            results.push(result);
            continue;
        }

        if is_path_deferred(deferred_transfers, path, "upload") {
            tracing::warn!(
                task_id = %task.id,
                relative_path = %path,
                "secondary return upload deferred"
            );
            results.push(network_error(path, "transfer deferred by user", false));
            continue;
        }

        let source = sync_root.join(path);
        if source.is_dir() {
            let planned = planner::PlannedAction {
                relative_path: path.clone(),
                decision: SyncDecision::ApplyToSecondary,
                snapshot: None,
                baseline: None,
            };
            results.push(
                send_directory_action(
                    &planned,
                    &task.primary_device_id,
                    task.id,
                    connections,
                    local_identity,
                )
                .await,
            );
            continue;
        }

        match send_file_with_retry(
            connections,
            local_identity,
            &task.primary_device_id,
            task.id.to_string(),
            path.clone(),
            &source,
        )
        .await
        {
            Ok(_outcome) => results.push(executor::ExecutionResult {
                relative_path: path.clone(),
                success: true,
                error: None,
                retryable: false,
            }),
            Err(e) => {
                let retryable = is_retryable_network_error(&e);
                results.push(network_error(
                    path,
                    &format!("network file transfer failed: {}", e),
                    retryable,
                ));
            }
        }
    }
    results
}

fn secondary_return_conflict(
    path: &str,
    change: &PendingReturnChange,
    remote_map: &HashMap<&str, &RemoteFileState>,
    baseline_map: &HashMap<&str, &SyncBaseline>,
) -> Option<String> {
    let current_primary = remote_map
        .get(path)
        .map(|remote| remote_file_state_to_snapshot(change.task_id, remote));
    if change.change_kind == ChangeKind::Deleted
        && current_primary
            .as_ref()
            .is_some_and(|snapshot| snapshot.kind == EntryKind::Directory)
    {
        return None;
    }
    if current_primary
        .as_ref()
        .is_some_and(|snapshot| snapshot.kind == EntryKind::Directory)
        && change.secondary_hash.is_none()
        && change.secondary_hash_status == HashStatus::Unavailable
    {
        return None;
    }
    let baseline = baseline_map.get(path).copied();

    match conflict::detect_conflict(change, current_primary.as_ref(), baseline) {
        conflict::ConflictResult::NoConflict => None,
        conflict::ConflictResult::Conflict { .. } => Some(return_conflict_message(
            change,
            current_primary.as_ref(),
            baseline,
        )),
    }
}

fn remote_file_state_to_snapshot(task_id: Uuid, remote: &RemoteFileState) -> FileSnapshot {
    FileSnapshot {
        task_id,
        relative_path: remote.relative_path.clone(),
        kind: remote.kind,
        size: remote.size,
        modified_unix_ms: remote.modified_unix_ms,
        blake3_hash: remote.blake3_hash.clone(),
        hash_status: remote.hash_status,
        deleted: false,
        is_symlink: false,
    }
}

fn return_conflict_message(
    change: &PendingReturnChange,
    current_primary: Option<&FileSnapshot>,
    baseline: Option<&SyncBaseline>,
) -> String {
    match (current_primary, baseline, change.change_kind) {
        (Some(_), None, _) => "primary file already exists".to_string(),
        (None, Some(_), ChangeKind::Modified) => {
            "primary file was deleted since last sync".to_string()
        }
        _ => "primary file changed since last sync".to_string(),
    }
}

fn persist_return_successes(
    task: &SyncTask,
    pending: &HashMap<String, PendingReturnChange>,
    results: &mut [executor::ExecutionResult],
    sync_root: &Path,
    db: &rusqlite::Connection,
) {
    let now = now_ms();
    let baseline_repo = repository::SyncBaselineRepository::new(db);
    let pending_repo = repository::PendingReturnRepository::new(db);

    for result in results {
        if !result.success {
            continue;
        }
        let Some(change) = pending.get(&result.relative_path) else {
            tracing::warn!(
                task_id = %task.id,
                relative_path = %result.relative_path,
                "return persist skipped because pending change is missing"
            );
            continue;
        };
        if change.change_kind == ChangeKind::Deleted {
            tracing::info!(
                task_id = %task.id,
                relative_path = %result.relative_path,
                "persist return-delete success start"
            );
            if let Err(e) = repository::FileSnapshotRepository::new(db)
                .mark_deleted(&task.id, &result.relative_path)
            {
                tracing::error!(
                    task_id = %task.id,
                    relative_path = %result.relative_path,
                    error = %e,
                    "return-delete snapshot update failed"
                );
                result.success = false;
                result.error = Some(format!("return-delete snapshot update failed: {}", e));
                result.retryable = true;
                continue;
            }
            if let Err(e) = baseline_repo.remove(&task.id, &result.relative_path) {
                tracing::error!(
                    task_id = %task.id,
                    relative_path = %result.relative_path,
                    error = %e,
                    "return-delete baseline remove failed"
                );
                result.success = false;
                result.error = Some(format!("return-delete baseline remove failed: {}", e));
                result.retryable = true;
                continue;
            }
            if let Err(e) = pending_repo.remove(&task.id, &result.relative_path) {
                tracing::error!(
                    task_id = %task.id,
                    relative_path = %result.relative_path,
                    error = %e,
                    "return-delete pending remove failed"
                );
                result.success = false;
                result.error = Some(format!("remove pending delete failed: {}", e));
                result.retryable = true;
            } else {
                tracing::info!(
                    task_id = %task.id,
                    relative_path = %result.relative_path,
                    "persist return-delete success complete"
                );
            }
            continue;
        }

        let primary_size = std::fs::metadata(sync_root.join(&result.relative_path))
            .map(|metadata| {
                if metadata.is_dir() {
                    0
                } else {
                    metadata.len() as i64
                }
            })
            .unwrap_or(0);
        let baseline = SyncBaseline {
            task_id: task.id,
            relative_path: result.relative_path.clone(),
            primary_hash: change.secondary_hash.clone(),
            primary_hash_status: change.secondary_hash_status,
            primary_size,
            secondary_size: primary_size,
            primary_modified_unix_ms: change.secondary_modified_unix_ms,
            secondary_hash: change.secondary_hash.clone(),
            secondary_hash_status: change.secondary_hash_status,
            secondary_modified_unix_ms: change.secondary_modified_unix_ms,
            last_synced_unix_ms: now,
        };
        if let Err(e) = baseline_repo.upsert(&baseline) {
            result.success = false;
            result.error = Some(format!("return-sync baseline update failed: {}", e));
            result.retryable = true;
            continue;
        }
        if let Err(e) = pending_repo.remove(&task.id, &result.relative_path) {
            result.success = false;
            result.error = Some(format!("remove pending change failed: {}", e));
            result.retryable = true;
        }
    }
}

// ─── Conflict ───

#[derive(Debug, Clone, Serialize)]
pub struct ConflictInfo {
    pub relative_path: String,
    pub primary_hash: Option<String>,
    pub primary_modified_unix_ms: i64,
    pub secondary_hash: Option<String>,
    pub secondary_modified_unix_ms: i64,
    pub hash_unverified: bool,
}

#[tauri::command]
pub async fn detect_conflicts(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<ConflictInfo>, String> {
    run_detect_conflicts(state.inner(), task_id).await
}

pub async fn run_detect_conflicts(
    state: &AppState,
    task_id: String,
) -> Result<Vec<ConflictInfo>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let task = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::SyncTaskRepository::new(&db)
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?
    };

    let primary_map = if task.local_role == DeviceRole::Primary {
        refresh_task_snapshots(state, &task)?
            .into_iter()
            .map(|snap| (snap.relative_path.clone(), snap))
            .collect::<HashMap<_, _>>()
    } else {
        request_scan_with_retry(
            &state.connections,
            &state.identity,
            &task.primary_device_id,
            task.id.to_string(),
        )
        .await
        .map_err(|e| format!("remote scan failed: {}", e))?
        .into_iter()
        .map(|remote| {
            let snap = remote_file_state_to_snapshot(task.id, &remote);
            (snap.relative_path.clone(), snap)
        })
        .collect::<HashMap<_, _>>()
    };

    let (pending_list, baselines) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        (
            repository::PendingReturnRepository::new(&db)
                .list_by_task(&id)
                .map_err(|e| e.to_string())?,
            repository::SyncBaselineRepository::new(&db)
                .list_by_task(&id)
                .map_err(|e| e.to_string())?,
        )
    };
    let baseline_map = baselines
        .into_iter()
        .map(|baseline| (baseline.relative_path.clone(), baseline))
        .collect::<HashMap<_, _>>();

    Ok(conflict_infos_from_maps(
        &pending_list,
        &primary_map,
        &baseline_map,
    ))
}

fn conflict_infos_from_maps(
    pending_list: &[PendingReturnChange],
    primary_map: &HashMap<String, FileSnapshot>,
    baseline_map: &HashMap<String, SyncBaseline>,
) -> Vec<ConflictInfo> {
    pending_list
        .iter()
        .filter_map(|pending| {
            let current_primary = primary_map.get(&pending.relative_path);
            let baseline = baseline_map.get(&pending.relative_path);
            match conflict::detect_conflict(pending, current_primary, baseline) {
                conflict::ConflictResult::Conflict {
                    relative_path,
                    primary_hash,
                    primary_hash_status: _,
                    primary_modified_unix_ms,
                    secondary_hash,
                    secondary_hash_status: _,
                    secondary_modified_unix_ms,
                    hash_unverified,
                } => Some(ConflictInfo {
                    relative_path,
                    primary_hash,
                    primary_modified_unix_ms,
                    secondary_hash,
                    secondary_modified_unix_ms,
                    hash_unverified,
                }),
                conflict::ConflictResult::NoConflict => None,
            }
        })
        .collect()
}

#[tauri::command]
pub async fn resolve_conflict_overwrite(
    state: State<'_, AppState>,
    task_id: String,
    relative_path: String,
) -> Result<SyncActionResult, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let task = load_task_for_conflict(&state, id)?;
    if task.local_role == DeviceRole::Secondary {
        return resolve_secondary_conflict_network(&state, &task, &relative_path, "overwrite")
            .await;
    }

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let sync_root = std::path::Path::new(&task.local_path);
    let result = executor::execute_confirmed_overwrite(&task, &relative_path, sync_root, &db);
    if result.success {
        state.file_list_refresh.mark(id, "sync_completed");
    }

    Ok(SyncActionResult {
        relative_path: result.relative_path,
        success: result.success,
        error: result.error,
    })
}

#[tauri::command]
pub async fn resolve_conflict_keep_both(
    state: State<'_, AppState>,
    task_id: String,
    relative_path: String,
) -> Result<SyncActionResult, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let task = load_task_for_conflict(&state, id)?;
    if task.local_role == DeviceRole::Secondary {
        return resolve_secondary_conflict_network(&state, &task, &relative_path, "keep_both")
            .await;
    }

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let sync_root = std::path::Path::new(&task.local_path);
    let result = executor::execute_conflict_keep_both(&task, &relative_path, sync_root, &db);
    if result.success {
        state.file_list_refresh.mark(id, "sync_completed");
    }

    Ok(SyncActionResult {
        relative_path: result.relative_path,
        success: result.success,
        error: result.error,
    })
}

fn load_task_for_conflict(state: &State<'_, AppState>, id: Uuid) -> Result<SyncTask, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    repository::SyncTaskRepository::new(&db)
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found".to_string())
}

async fn resolve_secondary_conflict_network(
    state: &State<'_, AppState>,
    task: &SyncTask,
    relative_path: &str,
    mode: &str,
) -> Result<SyncActionResult, String> {
    let pending = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::PendingReturnRepository::new(&db)
            .get(&task.id, relative_path)
            .map_err(|e| e.to_string())?
            .ok_or("pending change not found".to_string())?
    };

    if pending.change_kind == ChangeKind::Deleted {
        return resolve_secondary_delete_conflict_network(state, task, relative_path, mode).await;
    }

    let source = path_safety::safe_join(Path::new(&task.local_path), relative_path)
        .map_err(|e| e.to_string())?;
    if !source.is_file() {
        return Ok(SyncActionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some("副机文件不存在".to_string()),
        });
    }

    let staged_relative_path = format!(
        ".lanbridge-temp/conflict-{}/{}",
        Uuid::new_v4(),
        relative_path.trim_start_matches('/')
    );
    let upload = send_file_with_retry(
        &state.connections,
        &state.identity,
        &task.primary_device_id,
        task.id.to_string(),
        staged_relative_path.clone(),
        &source,
    )
    .await;
    if let Err(error) = upload {
        return Ok(SyncActionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some(format!("上传冲突文件失败: {}", error)),
        });
    }

    let msg = SyncMessage::ConflictApply {
        task_id: task.id.to_string(),
        relative_path: relative_path.to_string(),
        staged_relative_path,
        mode: mode.to_string(),
    };
    let apply = expect_file_ack(
        &task.primary_device_id,
        relative_path,
        &state.connections,
        &state.identity,
        msg,
    )
    .await;
    if apply.success {
        persist_secondary_conflict_success(state, task, relative_path, mode, &source)?;
        state.file_list_refresh.mark(task.id, "sync_completed");
    }
    Ok(SyncActionResult {
        relative_path: apply.relative_path,
        success: apply.success,
        error: apply.error,
    })
}

async fn resolve_secondary_delete_conflict_network(
    state: &State<'_, AppState>,
    task: &SyncTask,
    relative_path: &str,
    mode: &str,
) -> Result<SyncActionResult, String> {
    if mode == "keep_both" {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::PendingReturnRepository::new(&db)
            .remove(&task.id, relative_path)
            .map_err(|e| e.to_string())?;
        state.file_list_refresh.mark(task.id, "sync_completed");
        return Ok(SyncActionResult {
            relative_path: relative_path.to_string(),
            success: true,
            error: None,
        });
    }

    let result = send_delete_to_peer(
        &task.primary_device_id,
        task.id,
        relative_path,
        None,
        &state.connections,
        &state.identity,
    )
    .await;
    if result.success {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::PendingReturnRepository::new(&db)
            .remove_tree(&task.id, relative_path)
            .map_err(|e| e.to_string())?;
        repository::FileSnapshotRepository::new(&db)
            .remove_tree(&task.id, relative_path)
            .map_err(|e| e.to_string())?;
        repository::SyncBaselineRepository::new(&db)
            .remove_tree(&task.id, relative_path)
            .map_err(|e| e.to_string())?;
        state.file_list_refresh.mark(task.id, "sync_completed");
    }
    Ok(SyncActionResult {
        relative_path: result.relative_path,
        success: result.success,
        error: result.error,
    })
}

fn persist_secondary_conflict_success(
    state: &State<'_, AppState>,
    task: &SyncTask,
    relative_path: &str,
    _mode: &str,
    source: &Path,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    repository::PendingReturnRepository::new(&db)
        .remove(&task.id, relative_path)
        .map_err(|e| e.to_string())?;
    let metadata = std::fs::metadata(source).map_err(|e| e.to_string())?;
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_else(now_ms);
    let hash = scanner::hash_file(source).map_err(|e| e.to_string())?;
    repository::SyncBaselineRepository::new(&db)
        .upsert(&SyncBaseline {
            task_id: task.id,
            relative_path: relative_path.to_string(),
            primary_hash: Some(hash.clone()),
            primary_hash_status: HashStatus::Verified,
            primary_size: metadata.len() as i64,
            secondary_size: metadata.len() as i64,
            primary_modified_unix_ms: modified_unix_ms,
            secondary_hash: Some(hash),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: modified_unix_ms,
            last_synced_unix_ms: now_ms(),
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── History ───

#[tauri::command]
pub fn list_history(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<HistoryEntry>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let task = repository::SyncTaskRepository::new(&db)
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;
    let repo = repository::HistoryRepository::new(&db);
    let entries = repo.list_by_task(&id).map_err(|e| e.to_string())?;
    let mut entries = entries
        .into_iter()
        .filter(|entry| {
            if Path::new(&entry.stored_path).exists() {
                true
            } else {
                let _ = repo.remove(&id, &entry.id);
                false
            }
        })
        .collect::<Vec<_>>();
    let mut known_paths = entries
        .iter()
        .map(|entry| entry.stored_path.clone())
        .collect::<std::collections::HashSet<_>>();
    let discovered = HistoryStore::new(Path::new(&task.local_path))
        .discover_entries(id)
        .map_err(|e| e.to_string())?;
    for entry in discovered {
        if known_paths.insert(entry.stored_path.clone()) {
            entries.push(entry);
        }
    }
    entries = collapse_history_folder_entries(entries);
    entries.sort_by(|a, b| b.created_unix_ms.cmp(&a.created_unix_ms));
    Ok(entries)
}

fn collapse_history_folder_entries(entries: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    let mut sorted = entries;
    sorted.sort_by(|a, b| {
        path_depth(&a.original_relative_path)
            .cmp(&path_depth(&b.original_relative_path))
            .then_with(|| a.original_relative_path.cmp(&b.original_relative_path))
    });

    let mut kept: Vec<HistoryEntry> = Vec::new();
    'entry: for entry in sorted {
        if entry.reason == HistoryReason::Trash {
            for parent in &kept {
                if parent.reason == HistoryReason::Trash
                    && is_descendant_path(
                        &entry.original_relative_path,
                        &parent.original_relative_path,
                    )
                    && history_entries_look_grouped(parent, &entry)
                {
                    continue 'entry;
                }
            }
        }
        kept.push(entry);
    }
    kept
}

fn path_depth(relative_path: &str) -> usize {
    relative_path
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
}

fn history_entries_look_grouped(parent: &HistoryEntry, child: &HistoryEntry) -> bool {
    if parent.created_unix_ms == child.created_unix_ms {
        return true;
    }
    let parent_path = Path::new(&parent.stored_path);
    let child_path = Path::new(&child.stored_path);
    child_path.starts_with(parent_path)
}

#[tauri::command]
pub fn restore_history_entry(
    state: State<'_, AppState>,
    task_id: String,
    entry_id: String,
) -> Result<String, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let eid = Uuid::parse_str(&entry_id).map_err(|e| e.to_string())?;

    let (task, mut entries) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let task = repository::SyncTaskRepository::new(&db)
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?;
        let entries = repository::HistoryRepository::new(&db)
            .list_by_task(&id)
            .map_err(|e| e.to_string())?;
        (task, entries)
    };

    let sync_root = Path::new(&task.local_path);
    entries.extend(
        HistoryStore::new(sync_root)
            .discover_entries(id)
            .map_err(|e| e.to_string())?,
    );
    let entry = entries
        .into_iter()
        .find(|e| e.id == eid)
        .ok_or("history entry not found")?;

    let store = HistoryStore::new(sync_root);
    let restored = store
        .restore(&entry, sync_root, now_ms())
        .map_err(|e| e.to_string())?;

    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::HistoryRepository::new(&db)
            .remove(&id, &eid)
            .map_err(|e| e.to_string())?;
    }

    if task.local_role == DeviceRole::Secondary {
        mark_secondary_history_restore_local(
            &state,
            &task,
            &entry.original_relative_path,
            &restored,
        )?;
    }

    state.file_list_refresh.mark(id, "sync_completed");

    Ok(restored.to_string_lossy().to_string())
}

fn mark_secondary_history_restore_local(
    state: &State<'_, AppState>,
    task: &SyncTask,
    original_relative_path: &str,
    restored_path: &Path,
) -> Result<(), String> {
    let sync_root = Path::new(&task.local_path);
    let restored_relative_path = restored_path
        .strip_prefix(sync_root)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let restored_relative_path = state
        .platform
        .normalize_relative_path(restored_relative_path.trim());
    let snapshots = refresh_task_snapshots(state.inner(), task)?;
    let now = now_ms();

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let pending_repo = repository::PendingReturnRepository::new(&db);
    let baseline_repo = repository::SyncBaselineRepository::new(&db);
    pending_repo
        .remove_tree(&task.id, original_relative_path)
        .map_err(|e| e.to_string())?;
    pending_repo
        .remove_tree(&task.id, &restored_relative_path)
        .map_err(|e| e.to_string())?;
    baseline_repo
        .remove_tree(&task.id, original_relative_path)
        .map_err(|e| e.to_string())?;
    baseline_repo
        .remove_tree(&task.id, &restored_relative_path)
        .map_err(|e| e.to_string())?;

    for snapshot in snapshots.iter().filter(|snapshot| {
        snapshot.relative_path == restored_relative_path
            || is_descendant_path(&snapshot.relative_path, &restored_relative_path)
    }) {
        baseline_repo
            .upsert(&baseline_from_local_restore(snapshot, now))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn baseline_from_local_restore(snapshot: &FileSnapshot, now: i64) -> SyncBaseline {
    SyncBaseline {
        task_id: snapshot.task_id,
        relative_path: snapshot.relative_path.clone(),
        primary_hash: snapshot.blake3_hash.clone(),
        primary_hash_status: snapshot.hash_status,
        primary_size: snapshot.size,
        secondary_size: snapshot.size,
        primary_modified_unix_ms: snapshot.modified_unix_ms,
        secondary_hash: snapshot.blake3_hash.clone(),
        secondary_hash_status: snapshot.hash_status,
        secondary_modified_unix_ms: snapshot.modified_unix_ms,
        last_synced_unix_ms: now,
    }
}

#[tauri::command]
pub fn cleanup_history(state: State<'_, AppState>, task_id: String) -> Result<usize, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let store = HistoryStore::new(sync_root);
    let cutoff = now_ms() - crate::history::store::DEFAULT_RETENTION_DAYS_MS;
    let deleted = store
        .cleanup_old_entries(cutoff)
        .map_err(|e| e.to_string())?;

    // Also clean up database entries
    let history_repo = repository::HistoryRepository::new(&db);
    history_repo
        .delete_older_than(&id, cutoff)
        .map_err(|e| e.to_string())?;
    let existing_paths = store
        .discover_entries(id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|entry| entry.stored_path)
        .collect::<Vec<_>>();
    history_repo
        .remove_missing_stored_paths(&id, &existing_paths)
        .map_err(|e| e.to_string())?;

    Ok(deleted)
}

// ─── Logs ───

#[tauri::command]
pub fn list_logs(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<LogEntry>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::LogRepository::new(&db);
    repo.list_recent(limit.unwrap_or(100))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn write_log(
    state: State<'_, AppState>,
    level: String,
    message: String,
    task_id: Option<String>,
    relative_path: Option<String>,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::LogRepository::new(&db);

    let lvl = match level.as_str() {
        "Warn" => LogLevel::Warn,
        "Error" => LogLevel::Error,
        _ => LogLevel::Info,
    };

    let entry = LogEntry {
        id: None,
        level: lvl,
        task_id: task_id.and_then(|s| Uuid::parse_str(&s).ok()),
        relative_path,
        message,
        created_unix_ms: now_ms(),
    };

    repo.insert(&entry).map_err(|e| e.to_string())?;

    // Enforce retention
    repo.enforce_retention(10_000, now_ms() - 7 * 24 * 60 * 60 * 1000)
        .map_err(|e| e.to_string())?;

    Ok(())
}

// ─── Settings ───

#[derive(Debug, Clone, Serialize)]
pub struct AppSettings {
    pub history_retention_days: i64,
    pub history_size_limit_mb: i64,
}

#[tauri::command]
pub fn get_settings() -> Result<AppSettings, String> {
    Ok(AppSettings {
        history_retention_days: 30,
        history_size_limit_mb: 1024,
    })
}

#[tauri::command]
pub fn hide_main_window_to_tray(window: Window) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) -> Result<(), String> {
    let window = app.get_window("main").ok_or("main window not found")?;
    window.show().map_err(|e| e.to_string())?;
    window.unminimize().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn quit_app(app: AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

// ─── Transfer Progress ───

#[tauri::command]
pub fn get_transfer_progress() -> Result<Vec<connection::TransferProgress>, String> {
    Ok(connection::get_transfer_progress())
}

#[tauri::command]
pub fn has_active_transfers() -> Result<bool, String> {
    Ok(connection::has_active_transfers())
}

#[tauri::command]
pub fn get_sync_progress() -> Result<Vec<SyncProgress>, String> {
    let now = now_ms();
    Ok(SYNC_PROGRESS
        .lock()
        .map(|mut progress| {
            progress.retain(|_, entry| {
                entry
                    .finished_at_unix_ms
                    .map_or(true, |finished_at| now - finished_at <= 2_000)
            });
            progress.values().cloned().collect()
        })
        .unwrap_or_default())
}

#[tauri::command]
pub fn cancel_transfer(
    state: State<'_, AppState>,
    task_id: String,
    relative_path: String,
    direction: Option<String>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let direction = direction.unwrap_or_else(|| "upload".to_string());
    connection::cancel_transfer(&task_id, &relative_path, &direction);
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::DeferredTransferRepository::new(&db)
            .upsert(&DeferredTransferRecord {
                task_id: id,
                relative_path: relative_path.clone(),
                direction: direction.clone(),
                reason: "cancelled by user".to_string(),
                created_unix_ms: now_ms(),
            })
            .map_err(|e| e.to_string())?;
    }
    if direction == "receive" {
        if let Some(server) = &state._server {
            server
                .cancel_incoming_transfer(&task_id, &relative_path)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn resume_transfer(
    state: State<'_, AppState>,
    task_id: String,
    relative_path: String,
    direction: Option<String>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let still_deferred = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let repo = repository::DeferredTransferRepository::new(&db);
        repo.remove(&id, &relative_path, direction.as_deref())
            .map_err(|e| e.to_string())?;
        repo.exists(&id, &relative_path, None)
            .map_err(|e| e.to_string())?
    };
    if let Some(direction) = direction.as_deref() {
        connection::resume_deferred_transfer(&task_id, &relative_path, Some(direction));
        if still_deferred {
            let db = state.db.lock().map_err(|e| e.to_string())?;
            for record in repository::DeferredTransferRepository::new(&db)
                .list_all()
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|record| record.task_id == id && record.relative_path == relative_path)
            {
                connection::defer_transfer(&task_id, &relative_path, &record.direction);
            }
        }
    } else if still_deferred {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        for record in repository::DeferredTransferRepository::new(&db)
            .list_all()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|record| record.task_id == id && record.relative_path == relative_path)
        {
            connection::defer_transfer(&task_id, &relative_path, &record.direction);
        }
    } else {
        connection::resume_deferred_transfer(&task_id, &relative_path, None);
    }
    Ok(())
}

#[tauri::command]
pub fn list_deferred_transfers(
    state: State<'_, AppState>,
) -> Result<Vec<DeferredTransfer>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    Ok(repository::DeferredTransferRepository::new(&db)
        .list_all()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|record| DeferredTransfer {
            task_id: record.task_id.to_string(),
            relative_path: record.relative_path,
            direction: record.direction,
            reason: record.reason,
            created_unix_ms: record.created_unix_ms,
        })
        .collect())
}

#[tauri::command]
pub async fn get_task_peer_status(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<TaskPeerStatus, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let task = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::SyncTaskRepository::new(&db)
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?
    };
    let peer_device_id = if task.local_role == DeviceRole::Primary {
        task.secondary_device_id.clone()
    } else {
        task.primary_device_id.clone()
    };
    let peer = state.connections.get_peer(&peer_device_id);
    let Some(peer) = peer else {
        return Ok(TaskPeerStatus {
            task_id,
            peer_device_id,
            address: None,
            connected: false,
            last_seen_unix_ms: 0,
            error: Some("peer has no known address".to_string()),
        });
    };
    let address = peer.address.clone();
    if state.connections.is_manually_disconnected(&peer_device_id) {
        return Ok(TaskPeerStatus {
            task_id,
            peer_device_id,
            address: Some(address),
            connected: false,
            last_seen_unix_ms: peer.last_seen_unix_ms,
            error: Some("manually disconnected".to_string()),
        });
    }
    match connection::ping_known_peer(&state.connections, &peer_device_id).await {
        Ok(()) => {
            let refreshed = state.connections.get_peer(&peer_device_id).unwrap_or(peer);
            Ok(TaskPeerStatus {
                task_id,
                peer_device_id,
                address: Some(refreshed.address),
                connected: true,
                last_seen_unix_ms: refreshed.last_seen_unix_ms,
                error: None,
            })
        }
        Err(error) => Ok(TaskPeerStatus {
            task_id,
            peer_device_id,
            address: Some(address),
            connected: false,
            last_seen_unix_ms: peer.last_seen_unix_ms,
            error: Some(error.to_string()),
        }),
    }
}

#[tauri::command]
pub fn disconnect_task_peer(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<TaskPeerStatus, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let task = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::SyncTaskRepository::new(&db)
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?
    };
    let peer_device_id = if task.local_role == DeviceRole::Primary {
        task.secondary_device_id.clone()
    } else {
        task.primary_device_id.clone()
    };
    let peer = state.connections.get_peer(&peer_device_id);
    state.connections.manual_disconnect(&peer_device_id);

    Ok(TaskPeerStatus {
        task_id,
        peer_device_id,
        address: peer.as_ref().map(|item| item.address.clone()),
        connected: false,
        last_seen_unix_ms: peer.map_or(0, |item| item.last_seen_unix_ms),
        error: Some("manually disconnected".to_string()),
    })
}

// ─── Transfer Speed Limit ───

#[tauri::command]
pub fn set_transfer_speed_limit(
    _state: State<'_, AppState>,
    bytes_per_sec: u64,
) -> Result<(), String> {
    connection::set_transfer_speed_limit(bytes_per_sec);
    tracing::info!("transfer speed limit set to {} bytes/sec", bytes_per_sec);
    Ok(())
}

#[tauri::command]
pub fn get_transfer_speed_limit() -> Result<u64, String> {
    Ok(connection::get_transfer_speed_limit())
}

// ─── File Manager ───

#[tauri::command]
pub fn open_in_file_manager(path: String) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("failed to open file manager: {}", e))?;
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("failed to open file manager: {}", e))?;
    } else {
        return Err("unsupported platform".to_string());
    }
    Ok(())
}

// ─── Local Network Info ───

#[derive(Debug, Clone, Serialize)]
pub struct InterfaceInfo {
    pub name: String,
    pub ip: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalNetworkInfo {
    pub interfaces: Vec<InterfaceInfo>,
    pub tcp_port: u16,
}

#[tauri::command]
pub fn get_local_network_info(state: State<'_, AppState>) -> Result<LocalNetworkInfo, String> {
    let tcp_port = state._server.as_ref().map_or(0, |s| s.port());
    let interfaces = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, ip)| match ip {
            IpAddr::V4(ip) if !ip.is_loopback() => Some(InterfaceInfo {
                name,
                ip: ip.to_string(),
            }),
            _ => None,
        })
        .collect();
    Ok(LocalNetworkInfo {
        interfaces,
        tcp_port,
    })
}

// ─── Delete Task ───

#[tauri::command]
pub fn delete_task_entry(
    state: State<'_, AppState>,
    task_id: String,
    relative_path: String,
    destination: DeleteDestination,
) -> Result<Vec<DeleteEntryResult>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let relative_path = state.platform.normalize_relative_path(relative_path.trim());
    if relative_path.is_empty() {
        return Err("relative path cannot be empty".to_string());
    }
    state
        .platform
        .validate_target_relative_path(&relative_path)
        .map_err(|e| e.to_string())?;

    let task = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::SyncTaskRepository::new(&db)
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?
    };
    let root = Path::new(&task.local_path);
    let target = path_safety::safe_join(root, &relative_path).map_err(|e| e.to_string())?;
    if !target.exists() {
        return Ok(vec![DeleteEntryResult {
            relative_path,
            success: false,
            error: Some("文件不存在".to_string()),
        }]);
    }

    let delete_result = match destination {
        DeleteDestination::LanBridgeHistory => {
            delete_entry_to_lanbridge_history(&task, root, &target, &relative_path, &state)
        }
        DeleteDestination::SystemTrash => {
            trash::delete(&target).map_err(|e| format!("移入系统回收站失败: {}", e))
        }
    };

    if let Err(error) = delete_result {
        return Ok(vec![DeleteEntryResult {
            relative_path,
            success: false,
            error: Some(error),
        }]);
    }

    if task.local_role == DeviceRole::Secondary {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::FileSnapshotRepository::new(&db)
            .remove_tree(&task.id, &relative_path)
            .map_err(|e| e.to_string())?;
        repository::SyncBaselineRepository::new(&db)
            .remove_tree(&task.id, &relative_path)
            .map_err(|e| e.to_string())?;
        repository::PendingReturnRepository::new(&db)
            .remove_tree(&task.id, &relative_path)
            .map_err(|e| e.to_string())?;
    }
    state.file_list_refresh.mark(id, "entry_deleted");
    Ok(vec![DeleteEntryResult {
        relative_path,
        success: true,
        error: None,
    }])
}

#[tauri::command]
pub fn import_task_entries(
    state: State<'_, AppState>,
    task_id: String,
    source_paths: Vec<String>,
    target_relative_dir: String,
    collision_policy: ImportCollisionPolicy,
) -> Result<ImportTaskEntriesResult, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let task = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        repository::SyncTaskRepository::new(&db)
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?
    };
    let root = Path::new(&task.local_path);
    if !root.is_dir() {
        return Err("任务文件夹不存在".to_string());
    }

    let target_relative_dir = state
        .platform
        .normalize_relative_path(target_relative_dir.trim());
    if !target_relative_dir.is_empty() {
        state
            .platform
            .validate_target_relative_path(&target_relative_dir)
            .map_err(|e| e.to_string())?;
    }
    let target_dir = if target_relative_dir.is_empty() {
        root.to_path_buf()
    } else {
        path_safety::safe_join(root, &target_relative_dir).map_err(|e| e.to_string())?
    };
    std::fs::create_dir_all(&target_dir).map_err(|e| format!("创建目标文件夹失败: {}", e))?;

    let mut planned: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    let mut conflicts = Vec::new();
    let mut failed = Vec::new();

    for source in source_paths {
        match plan_import_entry(
            &state,
            root,
            &target_dir,
            &target_relative_dir,
            &source,
            &collision_policy,
        ) {
            Ok(Some(item)) => planned.push(item),
            Ok(None) => {}
            Err((relative_path, error)) => {
                let result = ImportEntryResult {
                    source_path: source,
                    relative_path,
                    success: false,
                    error: Some(error),
                };
                if collision_policy == ImportCollisionPolicy::Cancel {
                    conflicts.push(result);
                } else {
                    failed.push(result);
                }
            }
        }
    }

    if !conflicts.is_empty() {
        return Ok(ImportTaskEntriesResult {
            imported: Vec::new(),
            conflicts,
            failed,
        });
    }

    let mut imported = Vec::new();
    for (source, destination, relative_path) in planned {
        match copy_import_entry(&state, &source, &destination) {
            Ok(()) => imported.push(ImportEntryResult {
                source_path: source.to_string_lossy().to_string(),
                relative_path,
                success: true,
                error: None,
            }),
            Err(error) => failed.push(ImportEntryResult {
                source_path: source.to_string_lossy().to_string(),
                relative_path,
                success: false,
                error: Some(error),
            }),
        }
    }

    if !imported.is_empty() {
        state.file_list_refresh.mark(id, "entries_imported");
    }

    Ok(ImportTaskEntriesResult {
        imported,
        conflicts,
        failed,
    })
}

#[tauri::command]
pub fn get_window_cursor_position(window: Window) -> Result<Option<WindowCursorPosition>, String> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

        let mut point = POINT { x: 0, y: 0 };
        let ok = unsafe { GetCursorPos(&mut point) };
        if ok == 0 {
            return Ok(None);
        }

        let inner_position = window.inner_position().map_err(|e| e.to_string())?;
        let scale_factor = window.scale_factor().map_err(|e| e.to_string())?;
        let scale_factor = if scale_factor <= 0.0 { 1.0 } else { scale_factor };

        return Ok(Some(WindowCursorPosition {
            x: (point.x - inner_position.x) as f64 / scale_factor,
            y: (point.y - inner_position.y) as f64 / scale_factor,
        }));
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        Ok(None)
    }
}

fn plan_import_entry(
    state: &State<'_, AppState>,
    root: &Path,
    target_dir: &Path,
    target_relative_dir: &str,
    source_path: &str,
    collision_policy: &ImportCollisionPolicy,
) -> Result<Option<(PathBuf, PathBuf, String)>, (String, String)> {
    let source = PathBuf::from(source_path);
    if !source.exists() {
        return Err(("".to_string(), "源文件不存在".to_string()));
    }
    let metadata = source
        .metadata()
        .map_err(|e| ("".to_string(), format!("读取源文件失败: {}", e)))?;
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ("".to_string(), "文件名无效".to_string()))?;
    if should_ignore_import_entry(&*state.platform, name, metadata.is_dir()) {
        return Ok(None);
    }

    let initial_relative = join_relative_path(target_relative_dir, name);
    state
        .platform
        .validate_target_relative_path(&initial_relative)
        .map_err(|e| (initial_relative.clone(), e.to_string()))?;
    let initial_destination = path_safety::safe_join(root, &initial_relative)
        .map_err(|e| (initial_relative.clone(), e.to_string()))?;
    reject_import_cycle(&source, &initial_destination)
        .map_err(|e| (initial_relative.clone(), e))?;

    let (destination, relative_path) = if initial_destination.exists() {
        match collision_policy {
            ImportCollisionPolicy::Cancel => {
                return Err((initial_relative, "目标已存在".to_string()));
            }
            ImportCollisionPolicy::Overwrite => (initial_destination, initial_relative),
            ImportCollisionPolicy::KeepBoth => {
                unique_import_destination(root, target_dir, target_relative_dir, name)
                    .map_err(|e| (initial_relative.clone(), e))?
            }
        }
    } else {
        (initial_destination, initial_relative)
    };

    Ok(Some((source, destination, relative_path)))
}

fn join_relative_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

fn should_ignore_import_entry(
    platform: &dyn crate::platform::traits::Platform,
    name: &str,
    is_dir: bool,
) -> bool {
    if name == ".lanbridge-history" || name == ".lanbridge-tmp" {
        return true;
    }
    matches!(
        platform.classify_ignored_entry(name, is_dir),
        crate::platform::traits::IgnoreDecision::Ignored(_)
    )
}

fn unique_import_destination(
    root: &Path,
    target_dir: &Path,
    target_relative_dir: &str,
    name: &str,
) -> Result<(PathBuf, String), String> {
    let source_name = Path::new(name);
    let stem = source_name
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(name);
    let extension = source_name.extension().and_then(|value| value.to_str());
    for index in 1..=999 {
        let candidate_name = match extension {
            Some(ext) if !ext.is_empty() => format!("{} ({}).{}", stem, index, ext),
            _ => format!("{} ({})", name, index),
        };
        let relative_path = join_relative_path(target_relative_dir, &candidate_name);
        let destination =
            path_safety::safe_join(root, &relative_path).map_err(|e| e.to_string())?;
        if !destination.exists() && !target_dir.join(&candidate_name).exists() {
            return Ok((destination, relative_path));
        }
    }
    Err("无法生成可用文件名".to_string())
}

fn reject_import_cycle(source: &Path, destination: &Path) -> Result<(), String> {
    let source_canonical = source
        .canonicalize()
        .map_err(|e| format!("读取源路径失败: {}", e))?;
    if let Ok(destination_canonical) = destination.canonicalize() {
        if source_canonical == destination_canonical {
            return Err("源文件和目标文件相同".to_string());
        }
        if source_canonical.is_dir() && destination_canonical.starts_with(&source_canonical) {
            return Err("不能把文件夹导入到自身内部".to_string());
        }
    } else if let Some(parent) = destination.parent() {
        if let Ok(parent_canonical) = parent.canonicalize() {
            if source_canonical.is_dir() && parent_canonical.starts_with(&source_canonical) {
                return Err("不能把文件夹导入到自身内部".to_string());
            }
        }
    }
    Ok(())
}

fn copy_import_entry(
    state: &State<'_, AppState>,
    source: &Path,
    destination: &Path,
) -> Result<(), String> {
    if destination.exists() {
        let source_canonical = source
            .canonicalize()
            .map_err(|e| format!("读取源路径失败: {}", e))?;
        let destination_canonical = destination
            .canonicalize()
            .map_err(|e| format!("读取目标路径失败: {}", e))?;
        if source_canonical == destination_canonical {
            return Ok(());
        }
    }

    let metadata = source
        .metadata()
        .map_err(|e| format!("读取源文件失败: {}", e))?;
    if metadata.is_dir() {
        if destination.exists() && !destination.is_dir() {
            return Err("目标已存在且不是文件夹".to_string());
        }
        std::fs::create_dir_all(destination).map_err(|e| format!("创建文件夹失败: {}", e))?;
        copy_import_directory(state, source, destination)
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目标文件夹失败: {}", e))?;
        }
        std::fs::copy(source, destination)
            .map(|_| ())
            .map_err(|e| format!("复制文件失败: {}", e))
    } else {
        Err("不支持导入此类型".to_string())
    }
}

fn copy_import_directory(
    state: &State<'_, AppState>,
    source: &Path,
    destination: &Path,
) -> Result<(), String> {
    for entry in std::fs::read_dir(source).map_err(|e| format!("读取文件夹失败: {}", e))? {
        let entry = entry.map_err(|e| format!("读取文件夹失败: {}", e))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|e| format!("读取条目失败: {}", e))?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| "文件名无效".to_string())?;
        if should_ignore_import_entry(&*state.platform, name, metadata.is_dir()) {
            continue;
        }
        let next_destination = destination.join(name);
        if metadata.is_dir() {
            std::fs::create_dir_all(&next_destination)
                .map_err(|e| format!("创建文件夹失败: {}", e))?;
            copy_import_directory(state, &path, &next_destination)?;
        } else if metadata.is_file() {
            if let Some(parent) = next_destination.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建目标文件夹失败: {}", e))?;
            }
            std::fs::copy(&path, &next_destination).map_err(|e| format!("复制文件失败: {}", e))?;
        }
    }
    Ok(())
}

fn delete_entry_to_lanbridge_history(
    task: &SyncTask,
    root: &Path,
    target: &Path,
    relative_path: &str,
    state: &State<'_, AppState>,
) -> Result<(), String> {
    let history = HistoryStore::new(root);
    let now = now_ms();
    history
        .check_storage_blocked(now)
        .map_err(|e| e.to_string())?;
    let mut entry = history
        .move_to_trash(target, relative_path, now)
        .map_err(|e| format!("移入 LanBridge 历史失败: {}", e))?;
    entry.task_id = task.id;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    repository::HistoryRepository::new(&db)
        .insert(&entry)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn delete_sync_task(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    // Verify task exists
    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    // Unregister from server
    if let Some(server) = &state._server {
        server
            .unregister_task_root(&task_id)
            .map_err(|e| e.to_string())?;
    }

    // Delete from DB (cascade removes associated snapshots, baselines, pending, history)
    db.execute(
        "DELETE FROM sync_tasks WHERE id = ?1",
        rusqlite::params![task_id],
    )
    .map_err(|e| e.to_string())?;

    // Remove watcher if present
    {
        let mut watchers = state._watchers.lock().map_err(|e| e.to_string())?;
        watchers.retain(|(tid, _)| tid != &task_id);
    }

    tracing::info!("deleted sync task '{}' at {}", task.name, task.local_path);
    Ok(())
}

#[cfg(test)]
mod return_sync_tests {
    use super::*;

    fn pending_return(change_kind: ChangeKind) -> PendingReturnChange {
        PendingReturnChange {
            task_id: Uuid::nil(),
            relative_path: "file.txt".to_string(),
            change_kind,
            secondary_hash: Some("secondary_hash".to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: 2_000,
            created_unix_ms: 2_000,
        }
    }

    fn remote_primary(
        hash: Option<&str>,
        hash_status: HashStatus,
        size: i64,
        modified_unix_ms: i64,
    ) -> RemoteFileState {
        RemoteFileState {
            relative_path: "file.txt".to_string(),
            kind: EntryKind::File,
            blake3_hash: hash.map(str::to_string),
            hash_status,
            size,
            modified_unix_ms,
        }
    }

    fn baseline(
        hash: Option<&str>,
        hash_status: HashStatus,
        size: i64,
        modified_unix_ms: i64,
    ) -> SyncBaseline {
        SyncBaseline {
            task_id: Uuid::nil(),
            relative_path: "file.txt".to_string(),
            primary_hash: hash.map(str::to_string),
            primary_hash_status: hash_status,
            primary_size: size,
            secondary_size: size,
            primary_modified_unix_ms: modified_unix_ms,
            secondary_hash: Some("baseline_secondary_hash".to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: 1_000,
            last_synced_unix_ms: 1_000,
        }
    }

    fn task_with_id(task_id: Uuid, local_path: String) -> SyncTask {
        SyncTask {
            id: task_id,
            name: "return-delete-test".to_string(),
            primary_device_id: "primary".to_string(),
            secondary_device_id: "secondary".to_string(),
            local_path,
            remote_path: "remote".to_string(),
            local_role: DeviceRole::Secondary,
            enabled: true,
            created_unix_ms: 1_000,
            updated_unix_ms: 1_000,
        }
    }

    fn deleted_pending_for(task_id: Uuid, relative_path: &str) -> PendingReturnChange {
        PendingReturnChange {
            task_id,
            relative_path: relative_path.to_string(),
            change_kind: ChangeKind::Deleted,
            secondary_hash: None,
            secondary_hash_status: HashStatus::Unavailable,
            secondary_modified_unix_ms: 2_000,
            created_unix_ms: 2_000,
        }
    }

    fn snapshot_for(task_id: Uuid, relative_path: &str, size: i64) -> FileSnapshot {
        FileSnapshot {
            task_id,
            relative_path: relative_path.to_string(),
            kind: EntryKind::File,
            size,
            modified_unix_ms: 1_000,
            blake3_hash: Some(format!("hash-{relative_path}")),
            hash_status: HashStatus::Verified,
            deleted: false,
            is_symlink: false,
        }
    }

    fn baseline_for(task_id: Uuid, relative_path: &str, size: i64) -> SyncBaseline {
        SyncBaseline {
            task_id,
            relative_path: relative_path.to_string(),
            primary_hash: Some(format!("hash-{relative_path}")),
            primary_hash_status: HashStatus::Verified,
            primary_size: size,
            secondary_size: size,
            primary_modified_unix_ms: 1_000,
            secondary_hash: Some(format!("hash-{relative_path}")),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: 1_000,
            last_synced_unix_ms: 1_000,
        }
    }

    fn snapshot_from_remote(remote: RemoteFileState) -> FileSnapshot {
        remote_file_state_to_snapshot(Uuid::nil(), &remote)
    }

    #[test]
    fn secondary_network_return_uses_fallback_when_primary_hash_unavailable() {
        let change = pending_return(ChangeKind::Modified);
        let remote = remote_primary(None, HashStatus::Unavailable, 200, 2_000);
        let base = baseline(None, HashStatus::Unavailable, 100, 1_000);
        let remote_map = HashMap::from([("file.txt", &remote)]);
        let baseline_map = HashMap::from([("file.txt", &base)]);

        let result = secondary_return_conflict("file.txt", &change, &remote_map, &baseline_map);

        assert_eq!(
            result.as_deref(),
            Some("primary file changed since last sync")
        );
    }

    #[test]
    fn secondary_network_return_allows_unverified_primary_when_size_and_mtime_match() {
        let change = pending_return(ChangeKind::Modified);
        let remote = remote_primary(None, HashStatus::Unavailable, 100, 1_000);
        let base = baseline(None, HashStatus::Unavailable, 100, 1_000);
        let remote_map = HashMap::from([("file.txt", &remote)]);
        let baseline_map = HashMap::from([("file.txt", &base)]);

        let result = secondary_return_conflict("file.txt", &change, &remote_map, &baseline_map);

        assert_eq!(result, None);
    }

    #[test]
    fn secondary_network_return_reports_primary_deleted_for_modified_pending() {
        let change = pending_return(ChangeKind::Modified);
        let base = baseline(Some("base_hash"), HashStatus::Verified, 100, 1_000);
        let remote_map = HashMap::new();
        let baseline_map = HashMap::from([("file.txt", &base)]);

        let result = secondary_return_conflict("file.txt", &change, &remote_map, &baseline_map);

        assert_eq!(
            result.as_deref(),
            Some("primary file was deleted since last sync")
        );
    }

    #[test]
    fn conflict_infos_allow_secondary_created_file_when_primary_missing() {
        let change = pending_return(ChangeKind::Created);
        let conflicts = conflict_infos_from_maps(&[change], &HashMap::new(), &HashMap::new());

        assert!(conflicts.is_empty());
    }

    #[test]
    fn conflict_infos_report_secondary_created_file_when_primary_exists() {
        let change = pending_return(ChangeKind::Created);
        let primary = snapshot_from_remote(remote_primary(
            Some("primary_hash"),
            HashStatus::Verified,
            100,
            1_000,
        ));
        let primary_map = HashMap::from([("file.txt".to_string(), primary)]);

        let conflicts = conflict_infos_from_maps(&[change], &primary_map, &HashMap::new());

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].relative_path, "file.txt");
    }

    #[test]
    fn conflict_infos_report_modified_file_when_primary_deleted() {
        let change = pending_return(ChangeKind::Modified);
        let base = baseline(Some("base_hash"), HashStatus::Verified, 100, 1_000);
        let baseline_map = HashMap::from([("file.txt".to_string(), base)]);

        let conflicts = conflict_infos_from_maps(&[change], &HashMap::new(), &baseline_map);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].relative_path, "file.txt");
    }

    #[test]
    fn persist_return_successes_clears_deleted_pending_assets() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::state::db::migrate(&conn).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let task_id = Uuid::new_v4();
        let task = task_with_id(task_id, dir.path().to_string_lossy().to_string());
        repository::SyncTaskRepository::new(&conn)
            .insert(&task)
            .unwrap();

        let paths = ["Frame_2004.svg", "Frame_1980.png", "folder.svg"];
        let mut pending_map = HashMap::new();
        for (index, path) in paths.iter().enumerate() {
            let change = deleted_pending_for(task_id, path);
            repository::PendingReturnRepository::new(&conn)
                .upsert(&change)
                .unwrap();
            repository::FileSnapshotRepository::new(&conn)
                .upsert(&snapshot_for(task_id, path, (index + 1) as i64))
                .unwrap();
            repository::SyncBaselineRepository::new(&conn)
                .upsert(&baseline_for(task_id, path, (index + 1) as i64))
                .unwrap();
            pending_map.insert((*path).to_string(), change);
        }

        let mut results = paths
            .iter()
            .map(|path| executor::ExecutionResult {
                relative_path: (*path).to_string(),
                success: true,
                error: None,
                retryable: false,
            })
            .collect::<Vec<_>>();

        persist_return_successes(&task, &pending_map, &mut results, dir.path(), &conn);

        assert!(results.iter().all(|result| result.success));
        let pending_left = repository::PendingReturnRepository::new(&conn)
            .list_by_task(&task_id)
            .unwrap();
        assert!(pending_left.is_empty());
        for path in paths {
            let snap = repository::FileSnapshotRepository::new(&conn)
                .get(&task_id, path)
                .unwrap()
                .expect("snapshot should remain as a deleted marker");
            assert!(snap.deleted);
            assert!(repository::SyncBaselineRepository::new(&conn)
                .get(&task_id, path)
                .unwrap()
                .is_none());
        }
    }
}
