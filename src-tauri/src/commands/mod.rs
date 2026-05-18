pub mod pairing;

// Re-export all public items so main.rs can reference commands::* unchanged.
pub use pairing::*;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use uuid::Uuid;

use crate::app_state::{AppState, PendingOutgoingTaskInvite, SyncRunAdmission};
use crate::core::conflict;
use crate::core::executor;
use crate::core::model::*;
use crate::core::planner;
use crate::core::scanner;
use crate::history::store::HistoryStore;
use crate::state::repository;
use crate::transport::protocol::RemoteFileState;
use crate::transport::{connection, SyncMessage};

const MAX_NETWORK_ATTEMPTS: usize = 3;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
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

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::SyncTaskRepository::new(&db);
    repo.insert(&task).map_err(|e| e.to_string())?;
    Ok(task)
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

        let (hash, status) = verified_hash_for_successful_apply(&task, &action, &snap);

        assert_eq!(status, HashStatus::Verified);
        assert_eq!(hash, Some(blake3::hash(b"contents").to_hex().to_string()));
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

    let db = state.db.lock().map_err(|e| e.to_string())?;
    repository::SyncTaskRepository::new(&db)
        .insert(&task)
        .map_err(|e| e.to_string())?;
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
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::SyncTaskRepository::new(&db);
    let task = repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;
    if enabled {
        if let Some(server) = &state._server {
            server
                .register_task_root(task.id.to_string(), &task.local_path)
                .map_err(|e| e.to_string())?;
        }
    }
    repo.update_enabled(&id, enabled, now_ms())
        .map_err(|e| e.to_string())?;
    if !enabled {
        if let Some(server) = &state._server {
            server
                .unregister_task_root(&task_id)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ─── Scan ───

#[tauri::command]
pub fn scan_task(state: State<'_, AppState>, task_id: String) -> Result<Vec<FileSnapshot>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let results = scanner::scan_root(sync_root, &*state.platform).map_err(|e| e.to_string())?;

    let mut snapshots = Vec::new();
    let snap_repo = repository::FileSnapshotRepository::new(&db);
    for result in &results {
        let mut snap = result.snapshot.clone();
        snap.task_id = id;
        snapshots.push(snap);
    }
    snap_repo
        .replace_for_task(&id, &snapshots)
        .map_err(|e| e.to_string())?;

    Ok(snapshots)
}

fn refresh_task_snapshots(state: &AppState, task: &SyncTask) -> Result<Vec<FileSnapshot>, String> {
    let sync_root = std::path::Path::new(&task.local_path);
    let results = scanner::scan_root(sync_root, &*state.platform).map_err(|e| e.to_string())?;

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
}

static SYNC_PROGRESS: std::sync::LazyLock<Mutex<HashMap<String, SyncProgress>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn record_sync_progress(task_id: Uuid, phase: &str, detail: Option<String>) {
    if let Ok(mut progress) = SYNC_PROGRESS.lock() {
        let key = task_id.to_string();
        progress.insert(
            key.clone(),
            SyncProgress {
                task_id: key,
                phase: phase.to_string(),
                detail,
            },
        );
    }
}

fn finish_sync_progress(task_id: Uuid) {
    if let Ok(mut progress) = SYNC_PROGRESS.lock() {
        progress.remove(&task_id.to_string());
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
    finish_sync_progress(id);
    Ok(all_results)
}

async fn run_sync_now_once(state: &AppState, id: Uuid) -> Result<Vec<SyncActionResult>, String> {
    record_sync_progress(id, "扫描本机", None);
    let (task, actions, snapshots, baselines) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;

        let task_repo = repository::SyncTaskRepository::new(&db);
        let task = task_repo
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?;
        if !task.enabled {
            return Err("task is paused".to_string());
        }

        let sync_root = Path::new(&task.local_path);

        // Scan the current filesystem state so deletes are visible even when the
        // UI did not call scan_task immediately before sync_now.
        let scan_results =
            scanner::scan_root(sync_root, &*state.platform).map_err(|e| e.to_string())?;
        let mut snapshots = Vec::new();
        let snap_repo = repository::FileSnapshotRepository::new(&db);
        for result in &scan_results {
            let mut snap = result.snapshot.clone();
            snap.task_id = id;
            snapshots.push(snap);
        }
        snap_repo
            .replace_for_task(&id, &snapshots)
            .map_err(|e| e.to_string())?;

        // Get all baselines so files missing from current snapshots become delete actions.
        let baseline_repo = repository::SyncBaselineRepository::new(&db);
        let baselines = baseline_repo.list_by_task(&id).map_err(|e| e.to_string())?;

        let mut actions = planner::plan_sync(&snapshots, &baselines, task.local_role);
        sort_actions_by_priority(&mut actions);
        (task, actions, snapshots, baselines)
    };

    record_sync_progress(
        id,
        "请求对端状态",
        Some(format!("{} 个本机变更待检查", actions.len())),
    );
    let results = if task.local_role == DeviceRole::Primary {
        let connections = state.connections.clone();
        let sync_root = Path::new(&task.local_path);
        let remote_scan = request_scan_with_retry(
            &connections,
            &state.identity,
            &task.secondary_device_id,
            task.id.to_string(),
        )
        .await;
        let mut results = match remote_scan {
            Ok(remote_files) => {
                record_sync_progress(id, "传输中", Some(format!("{} 个动作", actions.len())));
                execute_primary_actions_over_network(
                    &actions,
                    &task,
                    sync_root,
                    &connections,
                    &state.identity,
                    &remote_files,
                )
                .await
            }
            Err(e) => actions
                .iter()
                .map(|action| {
                    network_error(
                        &action.relative_path,
                        &format!("remote scan failed: {}", e),
                        true,
                    )
                })
                .collect(),
        };
        let db = state.db.lock().map_err(|e| e.to_string())?;
        persist_network_successes(&actions, &task, &mut results, &db);
        results
    } else {
        let connections = state.connections.clone();
        let sync_root = Path::new(&task.local_path);
        match request_scan_with_retry(
            &connections,
            &state.identity,
            &task.primary_device_id,
            task.id.to_string(),
        )
        .await
        {
            Ok(mut remote_files) => {
                sort_remote_files_by_priority(&mut remote_files);
                record_sync_progress(
                    id,
                    "处理本机变更",
                    Some(format!("{} 个动作", actions.len())),
                );
                let mut local_results = {
                    let db = state.db.lock().map_err(|e| e.to_string())?;
                    executor::execute_actions(&actions, &task, sync_root, &db)
                };
                record_sync_progress(
                    id,
                    "拉取主机变更",
                    Some(format!("{} 个远端条目", remote_files.len())),
                );
                let mut results = execute_secondary_pull_over_network(
                    &task,
                    sync_root,
                    &connections,
                    &state.identity,
                    &remote_files,
                    &snapshots,
                    &baselines,
                )
                .await;
                let db = state.db.lock().map_err(|e| e.to_string())?;
                persist_secondary_pull_successes(&task, &mut results, sync_root, &db);
                local_results.extend(results);
                let results = local_results;
                results
            }
            Err(e) => {
                let mut local_results = {
                    let db = state.db.lock().map_err(|e| e.to_string())?;
                    executor::execute_actions(&actions, &task, sync_root, &db)
                };
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

    Ok(results
        .into_iter()
        .map(|r| SyncActionResult {
            relative_path: r.relative_path,
            success: r.success,
            error: r.error,
        })
        .collect())
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
) -> Vec<executor::ExecutionResult> {
    let remote_map = remote_files
        .iter()
        .map(|file| (file.relative_path.as_str(), file))
        .collect::<HashMap<_, _>>();
    let mut results = Vec::new();
    for action in actions {
        let result = match action.decision {
            SyncDecision::ApplyToSecondary => match remote_conflict(action, &remote_map) {
                Some(error) => executor::ExecutionResult {
                    relative_path: action.relative_path.clone(),
                    success: false,
                    error: Some(error),
                    retryable: false,
                },
                None => match action.snapshot.as_ref().map(|snapshot| snapshot.kind) {
                    Some(EntryKind::Directory) => {
                        send_directory_action(
                            action,
                            &task.secondary_device_id,
                            task.id,
                            connections,
                            local_identity,
                        )
                        .await
                    }
                    _ => {
                        send_file_action(action, task, sync_root, connections, local_identity).await
                    }
                },
            },
            SyncDecision::MoveSecondaryToHistory => {
                send_delete_action(action, task, connections, local_identity).await
            }
            SyncDecision::RequireConflictDecision => executor::ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: false,
                error: Some("conflict requires user decision".to_string()),
                retryable: false,
            },
            SyncDecision::KeepBoth | SyncDecision::MarkPendingReturn => executor::ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: false,
                error: Some("unsupported network action for primary sync".to_string()),
                retryable: false,
            },
            SyncDecision::Noop => executor::ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: true,
                error: None,
                retryable: false,
            },
        };
        results.push(result);
    }
    results
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
) -> executor::ExecutionResult {
    if action.snapshot.is_none() {
        return network_error(&action.relative_path, "no snapshot for apply action", false);
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
        Ok(()) => executor::ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: true,
            error: None,
            retryable: false,
        },
        Err(e) => {
            return network_error(
                &action.relative_path,
                &format!("network file transfer failed: {}", e),
                true,
            )
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
) -> anyhow::Result<()> {
    connection::clear_transfer_cancel(&task_id, &relative_path);
    if connection::is_transfer_deferred(&task_id, &relative_path) {
        return Err(anyhow::anyhow!("transfer deferred by user"));
    }
    let total = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);
    connection::record_transfer_progress(connection::TransferProgress {
        task_id: task_id.clone(),
        relative_path: relative_path.clone(),
        direction: "upload".to_string(),
        bytes_done: 0,
        bytes_total: total,
        mbps: 0.0,
        wire_bytes: 0,
        protocol_version: String::new(),
        finished: false,
    });

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
            Ok(()) => {
                connection::finish_transfer_progress(&task_id, &relative_path);
                return Ok(());
            }
            Err(e) => {
                if connection::is_transfer_cancelled(&task_id, &relative_path) {
                    connection::finish_transfer_progress(&task_id, &relative_path);
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

    connection::finish_transfer_progress(&task_id, &relative_path);
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("network file transfer failed")))
}

async fn download_file_with_retry(
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
    peer_device_id: &str,
    task_id: String,
    relative_path: String,
    target: &Path,
    total_bytes: u64,
) -> anyhow::Result<()> {
    connection::clear_transfer_cancel(&task_id, &relative_path);
    if connection::is_transfer_deferred(&task_id, &relative_path) {
        return Err(anyhow::anyhow!("transfer deferred by user"));
    }
    connection::record_transfer_progress(connection::TransferProgress {
        task_id: task_id.clone(),
        relative_path: relative_path.clone(),
        direction: "download".to_string(),
        bytes_done: 0,
        bytes_total: total_bytes,
        mbps: 0.0,
        wire_bytes: 0,
        protocol_version: String::new(),
        finished: false,
    });

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
            Ok(()) => {
                connection::finish_transfer_progress(&task_id, &relative_path);
                return Ok(());
            }
            Err(error) => {
                if connection::is_transfer_cancelled(&task_id, &relative_path) {
                    connection::finish_transfer_progress(&task_id, &relative_path);
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

    connection::finish_transfer_progress(&task_id, &relative_path);
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
        connections,
        local_identity,
    )
    .await
}

async fn send_delete_to_peer(
    peer_device_id: &str,
    task_id: Uuid,
    relative_path: &str,
    connections: &connection::ConnectionManager,
    local_identity: &crate::pairing::DeviceIdentity,
) -> executor::ExecutionResult {
    let msg = SyncMessage::FileDelete {
        task_id: task_id.to_string(),
        relative_path: relative_path.to_string(),
    };
    expect_file_ack(
        peer_device_id,
        relative_path,
        connections,
        local_identity,
        msg,
    )
    .await
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
        Ok(SyncMessage::FileAck { success, .. }) if success => executor::ExecutionResult {
            relative_path: relative_path.to_string(),
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
    results: &mut [executor::ExecutionResult],
    db: &rusqlite::Connection,
) {
    let now = now_ms();
    let baseline_repo = repository::SyncBaselineRepository::new(db);
    let snap_repo = repository::FileSnapshotRepository::new(db);

    for (action, result) in actions.iter().zip(results.iter_mut()) {
        if !result.success {
            continue;
        }
        match action.decision {
            SyncDecision::ApplyToSecondary => {
                let Some(snap) = &action.snapshot else {
                    continue;
                };
                let (hash, hash_status) = verified_hash_for_successful_apply(task, action, snap);
                let baseline = SyncBaseline {
                    task_id: task.id,
                    relative_path: action.relative_path.clone(),
                    primary_hash: hash.clone(),
                    primary_hash_status: hash_status,
                    primary_size: snap.size,
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
                if let Err(e) = snap_repo.mark_deleted(&task.id, &action.relative_path) {
                    result.success = false;
                    result.error = Some(format!("snapshot delete marker failed: {}", e));
                    result.retryable = true;
                    continue;
                }
                if let Err(e) = baseline_repo.remove(&task.id, &action.relative_path) {
                    result.success = false;
                    result.error = Some(format!("baseline remove failed: {}", e));
                    result.retryable = true;
                }
            }
            _ => {}
        }
    }
}

fn verified_hash_for_successful_apply(
    task: &SyncTask,
    action: &planner::PlannedAction,
    snap: &FileSnapshot,
) -> (Option<String>, HashStatus) {
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
) -> Vec<executor::ExecutionResult> {
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
            results.push(network_error(&remote.relative_path, &error, false));
            continue;
        }

        let target = sync_root.join(&remote.relative_path);
        if remote.kind == EntryKind::Directory {
            match std::fs::create_dir_all(&target) {
                Ok(()) => results.push(executor::ExecutionResult {
                    relative_path: remote.relative_path.clone(),
                    success: true,
                    error: None,
                    retryable: false,
                }),
                Err(e) => results.push(network_error(
                    &remote.relative_path,
                    &format!("directory create failed: {}", e),
                    true,
                )),
            }
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
            Ok(()) => results.push(executor::ExecutionResult {
                relative_path: remote.relative_path.clone(),
                success: true,
                error: None,
                retryable: false,
            }),
            Err(e) => results.push(network_error(
                &remote.relative_path,
                &format!("network file download failed: {}", e),
                true,
            )),
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
    results: &mut [executor::ExecutionResult],
    sync_root: &Path,
    db: &rusqlite::Connection,
) {
    let now = now_ms();
    let snap_repo = repository::FileSnapshotRepository::new(db);
    let baseline_repo = repository::SyncBaselineRepository::new(db);
    let pending_repo = repository::PendingReturnRepository::new(db);

    for result in results {
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
        if let Err(e) = baseline_repo.upsert(&SyncBaseline {
            task_id: task.id,
            relative_path: result.relative_path.clone(),
            primary_hash: hash.clone(),
            primary_hash_status: hash_status,
            primary_size: size,
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
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let task = repository::SyncTaskRepository::new(&db)
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    if !task.enabled {
        return Err("task is paused".to_string());
    }
    if task.local_role != DeviceRole::Secondary {
        return Ok(Vec::new());
    }

    let sync_root = Path::new(&task.local_path);
    let scan_results =
        scanner::scan_root(sync_root, &*state.platform).map_err(|e| e.to_string())?;
    let snapshots = scan_results
        .into_iter()
        .map(|result| {
            let mut snap = result.snapshot;
            snap.task_id = id;
            snap
        })
        .collect::<Vec<_>>();

    repository::FileSnapshotRepository::new(&db)
        .replace_for_task(&id, &snapshots)
        .map_err(|e| e.to_string())?;

    let baselines = repository::SyncBaselineRepository::new(&db)
        .list_by_task(&id)
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
    let task = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let task_repo = repository::SyncTaskRepository::new(&db);
        task_repo
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?
    };

    let results = if task.local_role == DeviceRole::Secondary {
        let (pending, baselines) = {
            let db = state.db.lock().map_err(|e| e.to_string())?;
            let pending_repo = repository::PendingReturnRepository::new(&db);
            let baseline_repo = repository::SyncBaselineRepository::new(&db);
            (
                pending_repo.list_by_task(&id).map_err(|e| e.to_string())?,
                baseline_repo.list_by_task(&id).map_err(|e| e.to_string())?,
            )
        };
        let pending_map: HashMap<String, PendingReturnChange> = pending
            .into_iter()
            .map(|change| (change.relative_path.clone(), change))
            .collect();
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

    Ok(results
        .into_iter()
        .map(|r| ReturnSyncResult {
            relative_path: r.relative_path,
            success: r.success,
            error: r.error,
        })
        .collect())
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
            results.push(network_error(
                path,
                "pending change not found in database",
                false,
            ));
            continue;
        };
        if let Some(error) = secondary_return_conflict(path, change, &remote_map, &baseline_map) {
            results.push(network_error(path, &error, false));
            continue;
        }

        if change.change_kind == ChangeKind::Deleted {
            results.push(
                send_delete_to_peer(
                    &task.primary_device_id,
                    task.id,
                    path,
                    connections,
                    local_identity,
                )
                .await,
            );
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
            Ok(()) => results.push(executor::ExecutionResult {
                relative_path: path.clone(),
                success: true,
                error: None,
                retryable: false,
            }),
            Err(e) => results.push(network_error(
                path,
                &format!("network file transfer failed: {}", e),
                true,
            )),
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
            continue;
        };
        if change.change_kind == ChangeKind::Deleted {
            if let Err(e) = repository::FileSnapshotRepository::new(db)
                .mark_deleted(&task.id, &result.relative_path)
            {
                result.success = false;
                result.error = Some(format!("return-delete snapshot update failed: {}", e));
                result.retryable = true;
                continue;
            }
            if let Err(e) = baseline_repo.remove(&task.id, &result.relative_path) {
                result.success = false;
                result.error = Some(format!("return-delete baseline remove failed: {}", e));
                result.retryable = true;
                continue;
            }
            if let Err(e) = pending_repo.remove(&task.id, &result.relative_path) {
                result.success = false;
                result.error = Some(format!("remove pending delete failed: {}", e));
                result.retryable = true;
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
pub fn resolve_conflict_overwrite(
    state: State<'_, AppState>,
    task_id: String,
    relative_path: String,
) -> Result<SyncActionResult, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let result = executor::execute_confirmed_overwrite(&task, &relative_path, sync_root, &db);

    Ok(SyncActionResult {
        relative_path: result.relative_path,
        success: result.success,
        error: result.error,
    })
}

#[tauri::command]
pub fn resolve_conflict_keep_both(
    state: State<'_, AppState>,
    task_id: String,
    relative_path: String,
) -> Result<SyncActionResult, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let result = executor::execute_conflict_keep_both(&task, &relative_path, sync_root, &db);

    Ok(SyncActionResult {
        relative_path: result.relative_path,
        success: result.success,
        error: result.error,
    })
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
    let mut entries = repo.list_by_task(&id).map_err(|e| e.to_string())?;
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
    entries.sort_by(|a, b| b.created_unix_ms.cmp(&a.created_unix_ms));
    Ok(entries)
}

#[tauri::command]
pub fn restore_history_entry(
    state: State<'_, AppState>,
    task_id: String,
    entry_id: String,
) -> Result<String, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let eid = Uuid::parse_str(&entry_id).map_err(|e| e.to_string())?;

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let history_repo = repository::HistoryRepository::new(&db);
    let mut entries = history_repo.list_by_task(&id).map_err(|e| e.to_string())?;
    let sync_root = std::path::Path::new(&task.local_path);
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
    history_repo.remove(&id, &eid).map_err(|e| e.to_string())?;

    Ok(restored.to_string_lossy().to_string())
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

// ─── Transfer Progress ───

#[tauri::command]
pub fn get_transfer_progress() -> Result<Vec<connection::TransferProgress>, String> {
    Ok(connection::get_transfer_progress())
}

#[tauri::command]
pub fn get_sync_progress() -> Result<Vec<SyncProgress>, String> {
    Ok(SYNC_PROGRESS
        .lock()
        .map(|progress| progress.values().cloned().collect())
        .unwrap_or_default())
}

#[tauri::command]
pub fn cancel_transfer(
    state: State<'_, AppState>,
    task_id: String,
    relative_path: String,
) -> Result<(), String> {
    connection::cancel_transfer(&task_id, &relative_path);
    if let Some(server) = &state._server {
        server
            .cancel_incoming_transfer(&task_id, &relative_path)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn resume_transfer(task_id: String, relative_path: String) -> Result<(), String> {
    connection::resume_deferred_transfer(&task_id, &relative_path);
    Ok(())
}

#[tauri::command]
pub fn list_deferred_transfers() -> Result<Vec<DeferredTransfer>, String> {
    Ok(connection::list_deferred_transfers()
        .into_iter()
        .map(|(task_id, relative_path)| DeferredTransfer {
            task_id,
            relative_path,
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
            primary_modified_unix_ms: modified_unix_ms,
            secondary_hash: Some("baseline_secondary_hash".to_string()),
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
}
