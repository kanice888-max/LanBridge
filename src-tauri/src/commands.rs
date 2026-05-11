use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::core::conflict;
use crate::core::executor;
use crate::core::model::*;
use crate::core::planner;
use crate::core::scanner;
use crate::history::store::HistoryStore;
use crate::pairing;
use crate::state::repository;
use crate::platform::traits::Platform;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ─── Identity ───

#[derive(Debug, Clone, Serialize)]
pub struct IdentityInfo {
    pub device_id: String,
    pub display_name: String,
}

#[tauri::command]
pub fn get_identity(state: State<'_, AppState>) -> Result<IdentityInfo, String> {
    let pub_id = state.identity.public();
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "Device".to_string());
    Ok(IdentityInfo {
        device_id: pub_id.device_id,
        display_name: hostname,
    })
}

// ─── Pairing ───

#[derive(Debug, Clone, Serialize)]
pub struct PairingStatus {
    pub code: Option<String>,
    pub peer_device_id: Option<String>,
    pub confirmed: bool,
}

#[tauri::command]
pub fn start_pairing(state: State<'_, AppState>) -> Result<String, String> {
    let nonce = pairing::generate_nonce();
    let local_pub = state.identity.public();
    Ok(hex::encode(&nonce))
}

#[tauri::command]
pub fn confirm_pairing_code(
    state: State<'_, AppState>,
    peer_device_id: String,
    peer_public_key: Vec<u8>,
    nonce_hex: String,
) -> Result<String, String> {
    let nonce = hex::decode(&nonce_hex).map_err(|e| e.to_string())?;
    let local_pub = state.identity.public();
    let code = pairing::derive_pairing_code(&local_pub.public_key, &peer_public_key, &nonce);

    // Store the peer as pinned
    state.connections.pin_peer(pairing::PublicIdentity {
        device_id: peer_device_id.clone(),
        public_key: peer_public_key,
    });

    Ok(code)
}

#[tauri::command]
pub fn approve_pairing(
    state: State<'_, AppState>,
    peer_device_id: String,
    display_name: String,
) -> Result<(), String> {
    let pinned = state
        .connections
        .get_pinned(&peer_device_id)
        .ok_or("peer not found")?;

    let device = PairedDevice {
        device_id: peer_device_id,
        display_name,
        public_key: pinned.public_key,
        last_seen_unix_ms: now_ms(),
        trusted: true,
    };

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::PairedDeviceRepository::new(&db);
    repo.upsert(&device).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_paired_devices(state: State<'_, AppState>) -> Result<Vec<PairedDevice>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    // PairedDeviceRepository doesn't have list_all, so we query manually
    let mut stmt = db
        .prepare("SELECT device_id, display_name, public_key, last_seen_unix_ms, trusted FROM paired_devices")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(PairedDevice {
                device_id: row.get(0)?,
                display_name: row.get(1)?,
                public_key: row.get(2)?,
                last_seen_unix_ms: row.get(3)?,
                trusted: row.get::<_, i32>(4)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut devices = Vec::new();
    for row in rows {
        devices.push(row.map_err(|e| e.to_string())?);
    }
    Ok(devices)
}

// ─── Manual Connection ───

#[tauri::command]
pub async fn connect_peer(
    state: State<'_, AppState>,
    address: String,
    port: u16,
) -> Result<(), String> {
    crate::transport::connection::connect_to_peer(&address, port)
        .await
        .map_err(|e| e.to_string())?;
    // In a real implementation, we'd store the stream and do TLS handshake
    Ok(())
}

// ─── Sync Tasks ───

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTaskRequest {
    pub name: String,
    pub local_path: String,
    pub remote_path: String,
    pub peer_device_id: String,
    pub local_role: String, // "Primary" or "Secondary"
}

#[tauri::command]
pub fn create_sync_task(
    state: State<'_, AppState>,
    request: CreateTaskRequest,
) -> Result<SyncTask, String> {
    let local_role = match request.local_role.as_str() {
        "Primary" => DeviceRole::Primary,
        _ => DeviceRole::Secondary,
    };

    let identity = state.identity.public();
    let peer = state
        .connections
        .get_pinned(&request.peer_device_id)
        .ok_or("peer device not pinned")?;

    let (primary_id, secondary_id) = match local_role {
        DeviceRole::Primary => (identity.device_id, request.peer_device_id),
        DeviceRole::Secondary => (request.peer_device_id, identity.device_id),
    };

    let task = SyncTask {
        id: Uuid::new_v4(),
        name: request.name,
        primary_device_id: primary_id,
        secondary_device_id: secondary_id,
        local_path: request.local_path,
        remote_path: request.remote_path,
        local_role,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::SyncTaskRepository::new(&db);
    repo.insert(&task).map_err(|e| e.to_string())?;
    Ok(task)
}

#[tauri::command]
pub fn list_sync_tasks(state: State<'_, AppState>) -> Result<Vec<SyncTask>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::SyncTaskRepository::new(&db);
    repo.list_all().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_sync_task(state: State<'_, AppState>, task_id: String) -> Result<Option<SyncTask>, String> {
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
    repo.update_enabled(&id, enabled, now_ms())
        .map_err(|e| e.to_string())
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
    let results = scanner::scan_root(sync_root, &state.platform).map_err(|e| e.to_string())?;

    let mut snapshots = Vec::new();
    let snap_repo = repository::FileSnapshotRepository::new(&db);
    for result in &results {
        let mut snap = result.snapshot.clone();
        snap.task_id = id;
        snap_repo.upsert(&snap).map_err(|e| e.to_string())?;
        snapshots.push(snap);
    }

    Ok(snapshots)
}

// ─── Sync ───

#[derive(Debug, Clone, Serialize)]
pub struct SyncActionResult {
    pub relative_path: String,
    pub success: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub fn sync_now(state: State<'_, AppState>, task_id: String) -> Result<Vec<SyncActionResult>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let sync_root = std::path::Path::new(&task.local_path);

    // Get current snapshots
    let snap_repo = repository::FileSnapshotRepository::new(&db);
    let snapshots = snap_repo.list_by_task(&id).map_err(|e| e.to_string())?;

    // Get baselines
    let baseline_repo = repository::SyncBaselineRepository::new(&db);
    let mut baselines = Vec::new();
    for snap in &snapshots {
        if let Some(b) = baseline_repo
            .get(&id, &snap.relative_path)
            .map_err(|e| e.to_string())?
        {
            baselines.push(b);
        }
    }

    // Plan
    let actions = planner::plan_sync(&snapshots, &baselines, task.local_role);

    // Execute
    let results = executor::execute_actions(&actions, &task, sync_root, &db);

    Ok(results
        .into_iter()
        .map(|r| SyncActionResult {
            relative_path: r.relative_path,
            success: r.success,
            error: r.error,
        })
        .collect())
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
pub fn get_pending_count(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<i64, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::PendingReturnRepository::new(&db);
    repo.count_by_task(&id).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct ReturnSyncResult {
    pub relative_path: String,
    pub success: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub fn execute_return_sync(
    state: State<'_, AppState>,
    task_id: String,
    selected_paths: Vec<String>,
) -> Result<Vec<ReturnSyncResult>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let sync_root = std::path::Path::new(&task.local_path);

    // Get current primary snapshots for conflict detection
    let snap_repo = repository::FileSnapshotRepository::new(&db);
    let all_snaps = snap_repo.list_by_task(&id).map_err(|e| e.to_string())?;
    let primary_map: HashMap<String, FileSnapshot> = all_snaps
        .into_iter()
        .map(|s| (s.relative_path.clone(), s))
        .collect();

    // Get baselines
    let baseline_repo = repository::SyncBaselineRepository::new(&db);
    let mut baseline_map = HashMap::new();
    for path in &selected_paths {
        if let Some(b) = baseline_repo.get(&id, path).map_err(|e| e.to_string())? {
            baseline_map.insert(path.clone(), b);
        }
    }

    let results =
        executor::execute_return_sync(&task, &selected_paths, &primary_map, &baseline_map, sync_root, &db);

    Ok(results
        .into_iter()
        .map(|r| ReturnSyncResult {
            relative_path: r.relative_path,
            success: r.success,
            error: r.error,
        })
        .collect())
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
pub fn detect_conflicts(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<ConflictInfo>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    let pending_repo = repository::PendingReturnRepository::new(&db);
    let pending_list = pending_repo.list_by_task(&id).map_err(|e| e.to_string())?;

    let snap_repo = repository::FileSnapshotRepository::new(&db);
    let baseline_repo = repository::SyncBaselineRepository::new(&db);

    let mut conflicts = Vec::new();
    for pending in &pending_list {
        let current_primary = snap_repo
            .get(&id, &pending.relative_path)
            .map_err(|e| e.to_string())?;
        let baseline = baseline_repo
            .get(&id, &pending.relative_path)
            .map_err(|e| e.to_string())?;

        match conflict::detect_conflict(pending, current_primary.as_ref(), baseline.as_ref()) {
            conflict::ConflictResult::Conflict {
                relative_path,
                primary_hash,
                primary_hash_status: _,
                primary_modified_unix_ms,
                secondary_hash,
                secondary_hash_status: _,
                secondary_modified_unix_ms,
                hash_unverified,
            } => {
                conflicts.push(ConflictInfo {
                    relative_path,
                    primary_hash,
                    primary_modified_unix_ms,
                    secondary_hash,
                    secondary_modified_unix_ms,
                    hash_unverified,
                });
            }
            _ => {}
        }
    }

    Ok(conflicts)
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

    let snap_repo = repository::FileSnapshotRepository::new(&db);
    let current = snap_repo
        .get(&id, &relative_path)
        .map_err(|e| e.to_string())?
        .ok_or("file snapshot not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let result = executor::execute_confirmed_overwrite(&task, &relative_path, &current, sync_root, &db);

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
    let repo = repository::HistoryRepository::new(&db);
    repo.list_by_task(&id).map_err(|e| e.to_string())
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
    let entries = history_repo.list_by_task(&id).map_err(|e| e.to_string())?;
    let entry = entries
        .into_iter()
        .find(|e| e.id == eid)
        .ok_or("history entry not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let store = HistoryStore::new(sync_root);
    let restored = store
        .restore(&entry, sync_root, now_ms())
        .map_err(|e| e.to_string())?;

    Ok(restored.to_string_lossy().to_string())
}

#[tauri::command]
pub fn cleanup_history(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<usize, String> {
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
    let deleted = store.cleanup_old_entries(cutoff).map_err(|e| e.to_string())?;

    // Also clean up database entries
    let history_repo = repository::HistoryRepository::new(&db);
    history_repo
        .delete_older_than(&id, cutoff)
        .map_err(|e| e.to_string())?;

    Ok(deleted)
}

// ─── Logs ───

#[tauri::command]
pub fn list_logs(state: State<'_, AppState>, limit: Option<usize>) -> Result<Vec<LogEntry>, String> {
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

// Helper for hex encoding/decoding
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        if s.len() % 2 != 0 {
            return Err("invalid hex length".to_string());
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
            .collect()
    }
}
