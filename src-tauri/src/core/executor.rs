use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::model::*;
use crate::core::planner::PlannedAction;
use crate::history::store::HistoryStore;
use crate::state::repository::*;

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
            SyncDecision::MarkPendingReturn => execute_mark_pending(action, task, conn, now),
            SyncDecision::RequireConflictDecision => ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: false,
                error: Some("conflict requires user decision".to_string()),
                retryable: false,
            },
            SyncDecision::KeepBoth => {
                execute_keep_both(action, task, sync_root, &history, conn, now)
            }
            SyncDecision::Noop => ExecutionResult {
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

/// Apply primary file to secondary.
///
/// Copies the primary file to the secondary (remote_path) location,
/// then updates baseline only after successful write.
fn execute_apply_to_secondary(
    action: &PlannedAction,
    task: &SyncTask,
    sync_root: &Path,
    conn: &rusqlite::Connection,
    now: i64,
) -> ExecutionResult {
    let snap = match &action.snapshot {
        Some(s) => s,
        None => {
            return ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: false,
                error: Some("no snapshot for apply action".to_string()),
                retryable: false,
            }
        }
    };

    // Copy file from primary (local_path) to secondary (remote_path)
    let source = sync_root.join(&action.relative_path);
    let remote_root = Path::new(&task.remote_path);
    let dest = remote_root.join(&action.relative_path);

    if !source.is_file() {
        return ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: false,
            error: Some("source file missing".to_string()),
            retryable: true,
        };
    }

    if let Err(e) = copy_file_verified(
        &source,
        &dest,
        snap.blake3_hash.as_deref(),
        snap.hash_status,
        Some(snap.size),
    ) {
        return ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: false,
            error: Some(e),
            retryable: true,
        };
    }

    // Update baseline
    let baseline_repo = SyncBaselineRepository::new(conn);
    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: action.relative_path.clone(),
        primary_hash: snap.blake3_hash.clone(),
        primary_hash_status: snap.hash_status,
        primary_size: snap.size,
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
/// The secondary file is moved to .lanbridge-history/trash/ instead of
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
        // File already gone, nothing to move
        return ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: true,
            error: None,
            retryable: false,
        };
    }

    if let Err(e) = history.check_storage_blocked(now) {
        return ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: false,
            error: Some(e.to_string()),
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
    let change_kind = match (&action.snapshot, &action.baseline) {
        (None, Some(_)) => ChangeKind::Deleted,
        (Some(_), Some(_)) => ChangeKind::Modified,
        (Some(_), None) => ChangeKind::Created,
        (None, None) => {
            return ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: false,
                error: Some("no snapshot or baseline for pending return".to_string()),
                retryable: false,
            }
        }
    };

    let pending_repo = PendingReturnRepository::new(conn);
    let change = PendingReturnChange {
        task_id: task.id,
        relative_path: action.relative_path.clone(),
        change_kind,
        secondary_hash: action
            .snapshot
            .as_ref()
            .and_then(|snap| snap.blake3_hash.clone()),
        secondary_hash_status: action
            .snapshot
            .as_ref()
            .map_or(HashStatus::Unavailable, |snap| snap.hash_status),
        secondary_modified_unix_ms: action
            .snapshot
            .as_ref()
            .map_or(now, |snap| snap.modified_unix_ms),
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

/// Execute KeepBoth: copy incoming file with conflict-safe name.
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

    let conflict_name =
        crate::core::conflict::conflict_filename(&action.relative_path, device_name, now, |name| {
            sync_root.join(name).exists()
        });

    // Copy incoming file to conflict-safe name
    let secondary_source = sync_root
        .join(&task.remote_path)
        .join(&action.relative_path);
    let dest = sync_root.join(&conflict_name);

    let expected_hash = action
        .snapshot
        .as_ref()
        .and_then(|s| s.blake3_hash.as_deref());
    let expected_status = action
        .snapshot
        .as_ref()
        .map_or(HashStatus::Unavailable, |s| s.hash_status);
    let expected_size = action.snapshot.as_ref().map(|s| s.size);

    if let Err(e) = copy_file_verified(
        &secondary_source,
        &dest,
        expected_hash,
        expected_status,
        expected_size,
    ) {
        return ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: false,
            error: Some(e),
            retryable: true,
        };
    }

    // Get actual size after copy
    let conflict_size = std::fs::metadata(&dest).map_or(0, |m| m.len() as i64);

    // Update baseline
    let baseline_repo = SyncBaselineRepository::new(conn);
    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: conflict_name.clone(),
        primary_hash: action.snapshot.as_ref().and_then(|s| s.blake3_hash.clone()),
        primary_hash_status: action
            .snapshot
            .as_ref()
            .map_or(HashStatus::Unavailable, |s| s.hash_status),
        primary_size: conflict_size,
        primary_modified_unix_ms: now,
        secondary_hash: action.snapshot.as_ref().and_then(|s| s.blake3_hash.clone()),
        secondary_hash_status: action
            .snapshot
            .as_ref()
            .map_or(HashStatus::Unavailable, |s| s.hash_status),
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
    let baseline_repo = SyncBaselineRepository::new(conn);
    let history = HistoryStore::new(sync_root);
    let now = now_ms();

    // Load all pending changes for this task from DB
    let all_pending = match pending_repo.list_by_task(&task.id) {
        Ok(v) => v,
        Err(e) => {
            return vec![ExecutionResult {
                relative_path: String::new(),
                success: false,
                error: Some(format!("failed to load pending changes: {}", e)),
                retryable: true,
            }];
        }
    };

    // Index pending by relative_path
    let pending_map: std::collections::HashMap<&str, &PendingReturnChange> = all_pending
        .iter()
        .map(|p| (p.relative_path.as_str(), p))
        .collect();

    for path in pending_paths {
        let pending = match pending_map.get(path.as_str()) {
            Some(p) => *p,
            None => {
                results.push(ExecutionResult {
                    relative_path: path.clone(),
                    success: false,
                    error: Some("pending change not found in database".to_string()),
                    retryable: false,
                });
                continue;
            }
        };

        // Check for conflict using real pending data
        let conflict = crate::core::conflict::detect_conflict(
            pending,
            current_primary.get(path),
            baselines.get(path),
        );

        match conflict {
            crate::core::conflict::ConflictResult::NoConflict => {
                if pending.change_kind == ChangeKind::Deleted {
                    let target = sync_root.join(path);
                    if target.exists() {
                        if let Err(e) = history.check_storage_blocked(now) {
                            results.push(ExecutionResult {
                                relative_path: path.clone(),
                                success: false,
                                error: Some(e.to_string()),
                                retryable: false,
                            });
                            continue;
                        }
                        match history.move_to_trash(&target, path, now) {
                            Ok(mut entry) => {
                                entry.task_id = task.id;
                                let _ = HistoryRepository::new(conn).insert(&entry);
                            }
                            Err(e) => {
                                results.push(ExecutionResult {
                                    relative_path: path.clone(),
                                    success: false,
                                    error: Some(format!(
                                        "return-delete move to history failed: {}",
                                        e
                                    )),
                                    retryable: is_retryable_error(&e.to_string()),
                                });
                                continue;
                            }
                        }
                    }
                    let _ = baseline_repo.remove(&task.id, path);
                    let _ = pending_repo.remove(&task.id, path);
                    results.push(ExecutionResult {
                        relative_path: path.clone(),
                        success: true,
                        error: None,
                        retryable: false,
                    });
                    continue;
                }

                // Copy secondary file to primary location
                let source = sync_root.join(&task.remote_path).join(path);
                let dest = sync_root.join(path);
                let mut copied_size: i64 = 0;

                match copy_file_verified(
                    &source,
                    &dest,
                    pending.secondary_hash.as_deref(),
                    pending.secondary_hash_status,
                    None,
                ) {
                    Ok(()) => {
                        if let Ok(meta) = std::fs::metadata(&dest) {
                            copied_size = meta.len() as i64;
                        }
                    }
                    Err(e) => {
                        results.push(ExecutionResult {
                            relative_path: path.clone(),
                            success: false,
                            error: Some(e),
                            retryable: true,
                        });
                        continue;
                    }
                }

                // Update baseline with actual file size
                let baseline = SyncBaseline {
                    task_id: task.id,
                    relative_path: path.clone(),
                    primary_hash: pending.secondary_hash.clone(),
                    primary_hash_status: pending.secondary_hash_status,
                    primary_size: copied_size,
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
                            error: Some(format!("return-sync baseline update failed: {}", e)),
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
/// .lanbridge-history/overwritten/.
pub fn execute_confirmed_overwrite(
    task: &SyncTask,
    relative_path: &str,
    sync_root: &Path,
    conn: &rusqlite::Connection,
) -> ExecutionResult {
    let history = HistoryStore::new(sync_root);
    let source = sync_root.join(relative_path);
    let now = now_ms();

    let secondary_source = sync_root.join(&task.remote_path).join(relative_path);
    if !secondary_source.is_file() {
        return ExecutionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some("secondary file missing".to_string()),
            retryable: true,
        };
    }

    if source.exists() {
        if let Err(e) = history.check_storage_blocked(now) {
            return ExecutionResult {
                relative_path: relative_path.to_string(),
                success: false,
                error: Some(e.to_string()),
                retryable: false,
            };
        }
    }

    let pending_repo = PendingReturnRepository::new(conn);
    let pending = pending_repo
        .list_by_task(&task.id)
        .ok()
        .and_then(|items| items.into_iter().find(|p| p.relative_path == relative_path));
    let expected_hash = pending.as_ref().and_then(|p| p.secondary_hash.as_deref());
    let expected_status = pending
        .as_ref()
        .map_or(HashStatus::Unavailable, |p| p.secondary_hash_status);

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

    if let Err(e) = copy_file_verified(
        &secondary_source,
        &source,
        expected_hash,
        expected_status,
        None,
    ) {
        return ExecutionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some(e),
            retryable: true,
        };
    }

    let (new_size, new_hash, new_hash_status) = file_state(&source);

    // Update baseline with actual new file state
    let baseline_repo = SyncBaselineRepository::new(conn);
    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: relative_path.to_string(),
        primary_hash: new_hash.clone(),
        primary_hash_status: new_hash_status,
        primary_size: new_size,
        primary_modified_unix_ms: now,
        secondary_hash: new_hash,
        secondary_hash_status: new_hash_status,
        secondary_modified_unix_ms: now,
        last_synced_unix_ms: now,
    };

    match baseline_repo.upsert(&baseline) {
        Ok(_) => {
            // Remove pending return
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

/// Execute conflict KeepBoth: copy secondary file with conflict-safe name.
///
/// The primary file is kept as-is. The secondary file is copied to
/// primary with a conflict-safe filename. The pending return is removed.
pub fn execute_conflict_keep_both(
    task: &SyncTask,
    relative_path: &str,
    sync_root: &Path,
    conn: &rusqlite::Connection,
) -> ExecutionResult {
    let now = now_ms();

    // Generate conflict-safe name
    let conflict_name = crate::core::conflict::conflict_filename(
        relative_path,
        &task.secondary_device_id,
        now,
        |name| sync_root.join(name).exists(),
    );

    // Copy secondary file to primary with conflict name
    let secondary_source = sync_root.join(&task.remote_path).join(relative_path);
    let dest = sync_root.join(&conflict_name);

    let pending_repo = PendingReturnRepository::new(conn);
    let pending = pending_repo
        .list_by_task(&task.id)
        .ok()
        .and_then(|items| items.into_iter().find(|p| p.relative_path == relative_path));
    let expected_hash = pending.as_ref().and_then(|p| p.secondary_hash.as_deref());
    let expected_status = pending
        .as_ref()
        .map_or(HashStatus::Unavailable, |p| p.secondary_hash_status);

    if let Err(e) = copy_file_verified(
        &secondary_source,
        &dest,
        expected_hash,
        expected_status,
        None,
    ) {
        return ExecutionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some(e),
            retryable: true,
        };
    }

    let (conflict_size, conflict_hash, conflict_hash_status) = file_state(&dest);

    // Update baseline for the conflict copy
    let baseline_repo = SyncBaselineRepository::new(conn);
    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: conflict_name.clone(),
        primary_hash: conflict_hash.clone(),
        primary_hash_status: conflict_hash_status,
        primary_size: conflict_size,
        primary_modified_unix_ms: now,
        secondary_hash: conflict_hash,
        secondary_hash_status: conflict_hash_status,
        secondary_modified_unix_ms: now,
        last_synced_unix_ms: now,
    };

    match baseline_repo.upsert(&baseline) {
        Ok(_) => {
            // Remove pending return
            let _ = pending_repo.remove(&task.id, relative_path);

            ExecutionResult {
                relative_path: conflict_name,
                success: true,
                error: None,
                retryable: false,
            }
        }
        Err(e) => ExecutionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some(format!("keep-both baseline update failed: {}", e)),
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

fn copy_file_verified(
    source: &Path,
    dest: &Path,
    expected_hash: Option<&str>,
    expected_status: HashStatus,
    expected_size: Option<i64>,
) -> Result<(), String> {
    if !source.is_file() {
        return Err("source file missing".to_string());
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory: {}", e))?;
    }

    let tmp_path = partial_path(dest);

    // Use block_in_place to avoid blocking the tokio runtime during file I/O.
    // This lets tokio run other tasks on different threads while this one blocks.
    tokio::task::block_in_place(|| {
        std::fs::copy(source, &tmp_path).map_err(|e| format!("failed to copy file: {}", e))?;

        if let Err(e) = verify_copied_file(&tmp_path, expected_hash, expected_status, expected_size)
        {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }

        if let Err(e) = std::fs::rename(&tmp_path, dest) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("failed to finalize file: {}", e));
        }

        Ok(())
    })
}

fn verify_copied_file(
    path: &Path,
    expected_hash: Option<&str>,
    expected_status: HashStatus,
    expected_size: Option<i64>,
) -> Result<(), String> {
    if expected_status == HashStatus::Verified {
        let expected_hash = expected_hash.ok_or_else(|| "missing expected hash".to_string())?;
        let actual_hash = crate::core::scanner::hash_file(path)
            .map_err(|e| format!("failed to hash copied file: {}", e))?;
        if actual_hash != expected_hash {
            return Err("hash mismatch after copy".to_string());
        }
    }

    if let Some(size) = expected_size {
        let actual_size = std::fs::metadata(path)
            .map_err(|e| format!("failed to inspect copied file: {}", e))?
            .len() as i64;
        if actual_size != size {
            return Err("size mismatch after copy".to_string());
        }
    }

    Ok(())
}

fn partial_path(dest: &Path) -> std::path::PathBuf {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".lanbridge-partial");
    std::path::PathBuf::from(tmp)
}

fn file_state(path: &Path) -> (i64, Option<String>, HashStatus) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (0, None, HashStatus::Unavailable);
    };
    let size = meta.len() as i64;
    if size <= crate::core::scanner::EAGER_HASH_LIMIT {
        match crate::core::scanner::hash_file(path) {
            Ok(hash) => (size, Some(hash), HashStatus::Verified),
            Err(_) => (size, None, HashStatus::Unavailable),
        }
    } else {
        (size, None, HashStatus::UnverifiedLargeFile)
    }
}

/// Get current timestamp in milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
