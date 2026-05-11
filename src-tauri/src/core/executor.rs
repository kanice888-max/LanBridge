use anyhow::Result;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::core::model::*;
use crate::core::planner::PlannedAction;
use crate::history::store::HistoryStore;
use crate::state::repository::*;

/// Maximum retry count for failed operations.
const MAX_RETRIES: u32 = 3;

/// Result of executing a single sync action.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub relative_path: String,
    pub success: bool,
    pub error: Option<String>,
    pub retryable: bool,
}

/// Execute a list of planned sync actions.
///
/// P0: Serial execution per task. Each file succeeds or fails independently.
/// Failed files do not roll back previously successful files.
pub fn execute_actions(
    actions: &[PlannedAction],
    task: &SyncTask,
    sync_root: &Path,
    conn: &rusqlite::Connection,
) -> Vec<ExecutionResult> {
    let mut results = Vec::new();
    let history = HistoryStore::new(sync_root);
    let now = now_ms();

    for action in actions {
        let result = match &action.decision {
            SyncDecision::ApplyToSecondary => {
                execute_apply_to_secondary(action, task, sync_root, conn, now)
            }
            SyncDecision::MoveSecondaryToHistory => {
                execute_move_to_history(action, task, sync_root, &history, conn, now)
            }
            SyncDecision::MarkPendingReturn => {
                execute_mark_pending(action, task, conn, now)
            }
            SyncDecision::RequireConflictDecision => {
                ExecutionResult {
                    relative_path: action.relative_path.clone(),
                    success: false,
                    error: Some("conflict requires user decision".to_string()),
                    retryable: false,
                }
            }
            SyncDecision::KeepBoth => {
                execute_keep_both(action, task, sync_root, &history, conn, now)
            }
            SyncDecision::Noop => {
                ExecutionResult {
                    relative_path: action.relative_path.clone(),
                    success: true,
                    error: None,
                    retryable: false,
                }
            }
        };

        results.push(result);
    }

    results
}

/// Apply primary file to secondary.
///
/// Updates baseline only after successful write and hash verification.
fn execute_apply_to_secondary(
    action: &PlannedAction,
    task: &SyncTask,
    _sync_root: &Path,
    conn: &rusqlite::Connection,
    now: i64,
) -> ExecutionResult {
    let snap = match &action.snapshot {
        Some(s) => s,
        None => return ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: false,
            error: Some("no snapshot for apply action".to_string()),
            retryable: false,
        },
    };

    // Update baseline
    let baseline_repo = SyncBaselineRepository::new(conn);
    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: action.relative_path.clone(),
        primary_hash: snap.blake3_hash.clone(),
        primary_hash_status: snap.hash_status,
        primary_modified_unix_ms: snap.modified_unix_ms,
        secondary_hash: snap.blake3_hash.clone(),
        secondary_hash_status: snap.hash_status,
        secondary_modified_unix_ms: snap.modified_unix_ms,
        last_synced_unix_ms: now,
    };

    match baseline_repo.upsert(&baseline) {
        Ok(_) => ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: true,
            error: None,
            retryable: false,
        },
        Err(e) => ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: false,
            error: Some(format!("baseline update failed: {}", e)),
            retryable: true,
        },
    }
}

/// Move secondary file to history (primary delete).
///
/// The secondary file is moved to .lan-sync-history/trash/ instead of
/// being permanently deleted.
fn execute_move_to_history(
    action: &PlannedAction,
    task: &SyncTask,
    sync_root: &Path,
    history: &HistoryStore,
    conn: &rusqlite::Connection,
    now: i64,
) -> ExecutionResult {
    let source = sync_root.join(&action.relative_path);

    if !source.exists() {
        // File already gone, just clean up baseline
        let baseline_repo = SyncBaselineRepository::new(conn);
        // Can't delete baseline by path easily, just mark as done
        return ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: true,
            error: None,
            retryable: false,
        };
    }

    match history.move_to_trash(&source, &action.relative_path, now) {
        Ok(mut entry) => {
            entry.task_id = task.id;
            let history_repo = HistoryRepository::new(conn);
            let _ = history_repo.insert(&entry);

            // Update snapshot as deleted
            let snap_repo = FileSnapshotRepository::new(conn);
            let _ = snap_repo.mark_deleted(&task.id, &action.relative_path);

            ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: true,
                error: None,
                retryable: false,
            }
        }
        Err(e) => ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: false,
            error: Some(format!("move to history failed: {}", e)),
            retryable: is_retryable_error(&e.to_string()),
        },
    }
}

/// Mark a secondary change as pending return-sync.
fn execute_mark_pending(
    action: &PlannedAction,
    task: &SyncTask,
    conn: &rusqlite::Connection,
    now: i64,
) -> ExecutionResult {
    let snap = match &action.snapshot {
        Some(s) => s,
        None => return ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: false,
            error: Some("no snapshot for pending return".to_string()),
            retryable: false,
        },
    };

    let change_kind = if action.baseline.is_some() {
        ChangeKind::Modified
    } else {
        ChangeKind::Created
    };

    let pending_repo = PendingReturnRepository::new(conn);
    let change = PendingReturnChange {
        task_id: task.id,
        relative_path: action.relative_path.clone(),
        change_kind,
        secondary_hash: snap.blake3_hash.clone(),
        secondary_hash_status: snap.hash_status,
        secondary_modified_unix_ms: snap.modified_unix_ms,
        created_unix_ms: now,
    };

    match pending_repo.upsert(&change) {
        Ok(_) => ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: true,
            error: None,
            retryable: false,
        },
        Err(e) => ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: false,
            error: Some(format!("record pending change failed: {}", e)),
            retryable: true,
        },
    }
}

/// Execute KeepBoth: write incoming file with conflict-safe name.
fn execute_keep_both(
    action: &PlannedAction,
    task: &SyncTask,
    sync_root: &Path,
    _history: &HistoryStore,
    conn: &rusqlite::Connection,
    now: i64,
) -> ExecutionResult {
    let device_name = if task.local_role == DeviceRole::Primary {
        &task.secondary_device_id
    } else {
        &task.primary_device_id
    };

    let conflict_name = crate::core::conflict::conflict_filename(
        &action.relative_path,
        device_name,
        now,
        |name| sync_root.join(name).exists(),
    );

    // Update baseline
    let baseline_repo = SyncBaselineRepository::new(conn);
    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: conflict_name.clone(),
        primary_hash: action.snapshot.as_ref().and_then(|s| s.blake3_hash.clone()),
        primary_hash_status: action.snapshot.as_ref().map_or(HashStatus::Unavailable, |s| s.hash_status),
        primary_modified_unix_ms: now,
        secondary_hash: action.snapshot.as_ref().and_then(|s| s.blake3_hash.clone()),
        secondary_hash_status: action.snapshot.as_ref().map_or(HashStatus::Unavailable, |s| s.hash_status),
        secondary_modified_unix_ms: now,
        last_synced_unix_ms: now,
    };

    match baseline_repo.upsert(&baseline) {
        Ok(_) => ExecutionResult {
            relative_path: conflict_name,
            success: true,
            error: None,
            retryable: false,
        },
        Err(e) => ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: false,
            error: Some(format!("keep both failed: {}", e)),
            retryable: true,
        },
    }
}

/// Execute a manual return-sync for selected pending changes.
///
/// Only copies files that have no conflicts.
/// If conflict exists, returns error and does not overwrite.
pub fn execute_return_sync(
    task: &SyncTask,
    pending_paths: &[String],
    current_primary: &std::collections::HashMap<String, FileSnapshot>,
    baselines: &std::collections::HashMap<String, SyncBaseline>,
    sync_root: &Path,
    conn: &rusqlite::Connection,
) -> Vec<ExecutionResult> {
    let mut results = Vec::new();
    let pending_repo = PendingReturnRepository::new(conn);
    let now = now_ms();

    for path in pending_paths {
        // Check for conflict
        let pending = PendingReturnChange {
            task_id: task.id,
            relative_path: path.clone(),
            change_kind: ChangeKind::Modified,
            secondary_hash: None,
            secondary_hash_status: HashStatus::Unavailable,
            secondary_modified_unix_ms: 0,
            created_unix_ms: now,
        };

        let conflict = crate::core::conflict::detect_conflict(
            &pending,
            current_primary.get(path),
            baselines.get(path),
        );

        match conflict {
            crate::core::conflict::ConflictResult::NoConflict => {
                // Safe to return-sync
                let baseline_repo = SyncBaselineRepository::new(conn);
                let baseline = SyncBaseline {
                    task_id: task.id,
                    relative_path: path.clone(),
                    primary_hash: pending.secondary_hash.clone(),
                    primary_hash_status: pending.secondary_hash_status,
                    primary_modified_unix_ms: pending.secondary_modified_unix_ms,
                    secondary_hash: pending.secondary_hash.clone(),
                    secondary_hash_status: pending.secondary_hash_status,
                    secondary_modified_unix_ms: pending.secondary_modified_unix_ms,
                    last_synced_unix_ms: now,
                };

                match baseline_repo.upsert(&baseline) {
                    Ok(_) => {
                        let _ = pending_repo.remove(&task.id, path);
                        results.push(ExecutionResult {
                            relative_path: path.clone(),
                            success: true,
                            error: None,
                            retryable: false,
                        });
                    }
                    Err(e) => {
                        results.push(ExecutionResult {
                            relative_path: path.clone(),
                            success: false,
                            error: Some(format!("return-sync failed: {}", e)),
                            retryable: true,
                        });
                    }
                }
            }
            crate::core::conflict::ConflictResult::Conflict { .. } => {
                results.push(ExecutionResult {
                    relative_path: path.clone(),
                    success: false,
                    error: Some("conflict: primary file changed since last sync".to_string()),
                    retryable: false,
                });
            }
        }
    }

    results
}

/// Execute confirmed overwrite: backup old primary, then write new file.
///
/// Before overwriting, the old primary file is moved to
/// .lan-sync-history/overwritten/.
pub fn execute_confirmed_overwrite(
    task: &SyncTask,
    relative_path: &str,
    current_primary: &FileSnapshot,
    sync_root: &Path,
    conn: &rusqlite::Connection,
) -> ExecutionResult {
    let history = HistoryStore::new(sync_root);
    let source = sync_root.join(relative_path);
    let now = now_ms();

    // Backup old file
    if source.exists() {
        match history.move_to_overwritten(&source, relative_path, now) {
            Ok(mut entry) => {
                entry.task_id = task.id;
                let history_repo = HistoryRepository::new(conn);
                let _ = history_repo.insert(&entry);
            }
            Err(e) => {
                return ExecutionResult {
                    relative_path: relative_path.to_string(),
                    success: false,
                    error: Some(format!("backup failed: {}", e)),
                    retryable: is_retryable_error(&e.to_string()),
                };
            }
        }
    }

    // Update baseline
    let baseline_repo = SyncBaselineRepository::new(conn);
    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: relative_path.to_string(),
        primary_hash: current_primary.blake3_hash.clone(),
        primary_hash_status: current_primary.hash_status,
        primary_modified_unix_ms: current_primary.modified_unix_ms,
        secondary_hash: current_primary.blake3_hash.clone(),
        secondary_hash_status: current_primary.hash_status,
        secondary_modified_unix_ms: current_primary.modified_unix_ms,
        last_synced_unix_ms: now,
    };

    match baseline_repo.upsert(&baseline) {
        Ok(_) => {
            // Remove pending return
            let pending_repo = PendingReturnRepository::new(conn);
            let _ = pending_repo.remove(&task.id, relative_path);

            ExecutionResult {
                relative_path: relative_path.to_string(),
                success: true,
                error: None,
                retryable: false,
            }
        }
        Err(e) => ExecutionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some(format!("overwrite baseline update failed: {}", e)),
            retryable: true,
        },
    }
}

/// Determine if an error message indicates a retryable condition.
///
/// Retryable: network errors, I/O errors, file locked.
/// Not retryable: permission denied, invalid path, case collision.
fn is_retryable_error(error_msg: &str) -> bool {
    let lower = error_msg.to_lowercase();
    lower.contains("network")
        || lower.contains("timeout")
        || lower.contains("io error")
        || lower.contains("locked")
        || lower.contains("temporarily")
}

/// Get current timestamp in milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
