#![cfg(target_os = "windows")]

use lanbridge::core::executor::*;
use lanbridge::core::model::*;
use lanbridge::core::planner::PlannedAction;
use lanbridge::state::db;
use lanbridge::state::repository::*;
use rusqlite::Connection;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use uuid::Uuid;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn setup_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    db::migrate(&conn).unwrap();
    conn
}

fn create_task(conn: &Connection) -> SyncTask {
    let task = SyncTask {
        id: Uuid::new_v4(),
        name: "Test Sync".to_string(),
        primary_device_id: "device-a".to_string(),
        secondary_device_id: "device-b".to_string(),
        local_path: "/tmp/primary".to_string(),
        remote_path: "/tmp/secondary".to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };
    let repo = SyncTaskRepository::new(conn);
    repo.insert(&task).unwrap();
    task
}

// ===== Executor Tests =====

#[test]
fn test_execute_apply_to_secondary() {
    let conn = setup_db();
    let mut task = create_task(&conn);
    let dir = TempDir::new().unwrap();
    let remote_dir = TempDir::new().unwrap();
    task.remote_path = remote_dir.path().to_string_lossy().to_string();
    std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
    let hash = blake3::hash(b"hello").to_hex().to_string();

    let snap = FileSnapshot {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        kind: EntryKind::File,
        size: 5,
        modified_unix_ms: now_ms(),
        blake3_hash: Some(hash.clone()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let action = PlannedAction {
        relative_path: "file.txt".to_string(),
        decision: SyncDecision::ApplyToSecondary,
        snapshot: Some(snap),
        baseline: None,
    };

    let results = execute_actions(&[action], &task, dir.path(), &conn);
    assert_eq!(results.len(), 1);
    assert!(results[0].success);
    assert!(results[0].error.is_none());

    // Verify baseline was created
    let baseline_repo = SyncBaselineRepository::new(&conn);
    let baseline = baseline_repo.get(&task.id, "file.txt").unwrap();
    assert!(baseline.is_some());
    assert_eq!(baseline.unwrap().primary_hash, Some(hash));
    assert_eq!(
        std::fs::read_to_string(remote_dir.path().join("file.txt")).unwrap(),
        "hello"
    );
}

#[test]
fn test_execute_apply_to_secondary_fails_when_source_missing() {
    let conn = setup_db();
    let mut task = create_task(&conn);
    let dir = TempDir::new().unwrap();
    let remote_dir = TempDir::new().unwrap();
    task.remote_path = remote_dir.path().to_string_lossy().to_string();

    let snap = FileSnapshot {
        task_id: task.id,
        relative_path: "missing.txt".to_string(),
        kind: EntryKind::File,
        size: 100,
        modified_unix_ms: now_ms(),
        blake3_hash: Some("hash123".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let action = PlannedAction {
        relative_path: "missing.txt".to_string(),
        decision: SyncDecision::ApplyToSecondary,
        snapshot: Some(snap),
        baseline: None,
    };

    let results = execute_actions(&[action], &task, dir.path(), &conn);
    assert_eq!(results.len(), 1);
    assert!(!results[0].success);

    let baseline_repo = SyncBaselineRepository::new(&conn);
    assert!(baseline_repo
        .get(&task.id, "missing.txt")
        .unwrap()
        .is_none());
}

#[test]
fn test_execute_apply_to_secondary_rejects_hash_mismatch() {
    let conn = setup_db();
    let mut task = create_task(&conn);
    let dir = TempDir::new().unwrap();
    let remote_dir = TempDir::new().unwrap();
    task.remote_path = remote_dir.path().to_string_lossy().to_string();
    std::fs::write(dir.path().join("file.txt"), "actual content").unwrap();

    let snap = FileSnapshot {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        kind: EntryKind::File,
        size: 14,
        modified_unix_ms: now_ms(),
        blake3_hash: Some("not_the_actual_hash".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let action = PlannedAction {
        relative_path: "file.txt".to_string(),
        decision: SyncDecision::ApplyToSecondary,
        snapshot: Some(snap),
        baseline: None,
    };

    let results = execute_actions(&[action], &task, dir.path(), &conn);
    assert_eq!(results.len(), 1);
    assert!(!results[0].success);
    assert!(results[0].error.as_ref().unwrap().contains("hash"));
    assert!(!remote_dir.path().join("file.txt").exists());

    let baseline_repo = SyncBaselineRepository::new(&conn);
    assert!(baseline_repo.get(&task.id, "file.txt").unwrap().is_none());
}

#[test]
fn test_execute_mark_pending_return() {
    let conn = setup_db();
    let mut task = create_task(&conn);
    task.local_role = DeviceRole::Secondary;
    let dir = TempDir::new().unwrap();

    let snap = FileSnapshot {
        task_id: task.id,
        relative_path: "new_file.txt".to_string(),
        kind: EntryKind::File,
        size: 50,
        modified_unix_ms: now_ms(),
        blake3_hash: Some("hash456".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let action = PlannedAction {
        relative_path: "new_file.txt".to_string(),
        decision: SyncDecision::MarkPendingReturn,
        snapshot: Some(snap),
        baseline: None,
    };

    let results = execute_actions(&[action], &task, dir.path(), &conn);
    assert!(results[0].success);

    // Verify pending return was recorded
    let pending_repo = PendingReturnRepository::new(&conn);
    let pending = pending_repo.list_by_task(&task.id).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].relative_path, "new_file.txt");
    assert_eq!(pending[0].change_kind, ChangeKind::Created);
}

#[test]
fn test_execute_move_to_history() {
    let conn = setup_db();
    let task = create_task(&conn);
    let dir = TempDir::new().unwrap();

    // Create a file to be moved to history
    let file_path = dir.path().join("delete_me.txt");
    std::fs::write(&file_path, "important data").unwrap();

    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: "delete_me.txt".to_string(),
        primary_hash: Some("hash789".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_size: 100,
        secondary_size: 100,
        primary_modified_unix_ms: now_ms(),
        secondary_hash: Some("hash789".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: now_ms(),
        last_synced_unix_ms: now_ms(),
    };

    let action = PlannedAction {
        relative_path: "delete_me.txt".to_string(),
        decision: SyncDecision::MoveSecondaryToHistory,
        snapshot: None,
        baseline: Some(baseline),
    };

    let results = execute_actions(&[action], &task, dir.path(), &conn);
    assert!(results[0].success);

    // Verify file was moved (source should no longer exist)
    assert!(!file_path.exists());

    // Verify history entry was recorded
    let history_repo = HistoryRepository::new(&conn);
    let entries = history_repo.list_by_task(&task.id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].reason, HistoryReason::Trash);
}

#[test]
fn test_secondary_delete_does_not_affect_primary() {
    let conn = setup_db();
    let mut task = create_task(&conn);
    task.local_role = DeviceRole::Secondary;

    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        primary_hash: Some("hash".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_size: 100,
        secondary_size: 100,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("hash".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };

    // Secondary deletes the file — should record an explicit pending delete request.
    let actions = lanbridge::core::planner::plan_sync(
        &[], // empty snapshots = deleted
        &[baseline.clone()],
        DeviceRole::Secondary,
    );

    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].decision, SyncDecision::MarkPendingReturn);

    let results = execute_actions(&actions, &task, std::path::Path::new("."), &conn);
    assert_eq!(results.len(), 1);
    assert!(results[0].success);

    let pending = PendingReturnRepository::new(&conn)
        .list_by_task(&task.id)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].relative_path, "file.txt");
    assert_eq!(pending[0].change_kind, ChangeKind::Deleted);
}

#[test]
fn test_confirmed_overwrite_backs_up() {
    let conn = setup_db();
    let mut task = create_task(&conn);
    let dir = TempDir::new().unwrap();
    let remote_dir = TempDir::new().unwrap();
    task.remote_path = remote_dir.path().to_string_lossy().to_string();

    // Create an existing file
    let file_path = dir.path().join("overwrite_me.txt");
    std::fs::write(&file_path, "original content").unwrap();
    std::fs::write(
        remote_dir.path().join("overwrite_me.txt"),
        "incoming content",
    )
    .unwrap();

    let result = execute_confirmed_overwrite(&task, "overwrite_me.txt", dir.path(), &conn);

    assert!(result.success);
    assert_eq!(
        std::fs::read_to_string(&file_path).unwrap(),
        "incoming content"
    );

    // Verify history entry for backup
    let history_repo = HistoryRepository::new(&conn);
    let entries = history_repo.list_by_task(&task.id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].reason, HistoryReason::Overwritten);
}

#[test]
fn test_confirmed_overwrite_fails_before_backup_when_secondary_missing() {
    let conn = setup_db();
    let mut task = create_task(&conn);
    let dir = TempDir::new().unwrap();
    let remote_dir = TempDir::new().unwrap();
    task.remote_path = remote_dir.path().to_string_lossy().to_string();

    let file_path = dir.path().join("overwrite_me.txt");
    std::fs::write(&file_path, "original content").unwrap();

    let result = execute_confirmed_overwrite(&task, "overwrite_me.txt", dir.path(), &conn);

    assert!(!result.success);
    assert_eq!(
        std::fs::read_to_string(&file_path).unwrap(),
        "original content"
    );

    let history_repo = HistoryRepository::new(&conn);
    assert!(history_repo.list_by_task(&task.id).unwrap().is_empty());
}

#[test]
fn test_return_sync_conflict_blocked() {
    let conn = setup_db();
    let task = create_task(&conn);
    let dir = TempDir::new().unwrap();

    // Setup: file exists on primary with changed hash
    let mut current_primary = HashMap::new();
    current_primary.insert(
        "file.txt".to_string(),
        FileSnapshot {
            task_id: task.id,
            relative_path: "file.txt".to_string(),
            kind: EntryKind::File,
            size: 200,
            modified_unix_ms: now_ms(),
            blake3_hash: Some("changed_hash".to_string()),
            hash_status: HashStatus::Verified,
            deleted: false,
            is_symlink: false,
        },
    );

    // Baseline has different hash
    let mut baselines = HashMap::new();
    baselines.insert(
        "file.txt".to_string(),
        SyncBaseline {
            task_id: task.id,
            relative_path: "file.txt".to_string(),
            primary_hash: Some("original_hash".to_string()),
            primary_hash_status: HashStatus::Verified,
            primary_size: 100,
            secondary_size: 100,
            primary_modified_unix_ms: 1000,
            secondary_hash: Some("original_hash".to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: 1000,
            last_synced_unix_ms: 1000,
        },
    );

    // Record pending return
    let pending_repo = PendingReturnRepository::new(&conn);
    pending_repo
        .upsert(&PendingReturnChange {
            task_id: task.id,
            relative_path: "file.txt".to_string(),
            change_kind: ChangeKind::Modified,
            secondary_hash: Some("sec_hash".to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: now_ms(),
            created_unix_ms: now_ms(),
        })
        .unwrap();

    let results = execute_return_sync(
        &task,
        &["file.txt".to_string()],
        &current_primary,
        &baselines,
        dir.path(),
        &conn,
    );

    assert_eq!(results.len(), 1);
    assert!(!results[0].success, "conflict should block return-sync");
    assert!(results[0].error.as_ref().unwrap().contains("conflict"));
}

#[test]
fn test_return_sync_blocks_secondary_modify_when_primary_deleted_after_baseline() {
    let conn = setup_db();
    let task = create_task(&conn);
    let dir = TempDir::new().unwrap();
    let current_primary = HashMap::new();

    let mut baselines = HashMap::new();
    baselines.insert(
        "file.txt".to_string(),
        SyncBaseline {
            task_id: task.id,
            relative_path: "file.txt".to_string(),
            primary_hash: Some("original_hash".to_string()),
            primary_hash_status: HashStatus::Verified,
            primary_size: 100,
            secondary_size: 100,
            primary_modified_unix_ms: 1000,
            secondary_hash: Some("original_hash".to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: 1000,
            last_synced_unix_ms: 1000,
        },
    );

    let pending_repo = PendingReturnRepository::new(&conn);
    pending_repo
        .upsert(&PendingReturnChange {
            task_id: task.id,
            relative_path: "file.txt".to_string(),
            change_kind: ChangeKind::Modified,
            secondary_hash: Some("secondary_hash".to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: now_ms(),
            created_unix_ms: now_ms(),
        })
        .unwrap();

    let results = execute_return_sync(
        &task,
        &["file.txt".to_string()],
        &current_primary,
        &baselines,
        dir.path(),
        &conn,
    );

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].success,
        "primary deletion plus secondary modification should require user decision"
    );
    assert!(results[0].error.as_ref().unwrap().contains("conflict"));
    assert_eq!(pending_repo.count_by_task(&task.id).unwrap(), 1);
}

#[test]
fn test_return_sync_no_conflict_succeeds() {
    let conn = setup_db();
    let mut task = create_task(&conn);
    let dir = TempDir::new().unwrap();
    let remote_dir = TempDir::new().unwrap();
    task.remote_path = remote_dir.path().to_string_lossy().to_string();
    std::fs::write(remote_dir.path().join("file.txt"), "new secondary").unwrap();
    let secondary_hash = blake3::hash(b"new secondary").to_hex().to_string();

    // No change on primary — same hash as baseline
    let mut current_primary = HashMap::new();
    current_primary.insert(
        "file.txt".to_string(),
        FileSnapshot {
            task_id: task.id,
            relative_path: "file.txt".to_string(),
            kind: EntryKind::File,
            size: 100,
            modified_unix_ms: 1000,
            blake3_hash: Some("same_hash".to_string()),
            hash_status: HashStatus::Verified,
            deleted: false,
            is_symlink: false,
        },
    );

    let mut baselines = HashMap::new();
    baselines.insert(
        "file.txt".to_string(),
        SyncBaseline {
            task_id: task.id,
            relative_path: "file.txt".to_string(),
            primary_hash: Some("same_hash".to_string()),
            primary_hash_status: HashStatus::Verified,
            primary_size: 100,
            secondary_size: 100,
            primary_modified_unix_ms: 1000,
            secondary_hash: Some("same_hash".to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: 1000,
            last_synced_unix_ms: 1000,
        },
    );

    let pending_repo = PendingReturnRepository::new(&conn);
    pending_repo
        .upsert(&PendingReturnChange {
            task_id: task.id,
            relative_path: "file.txt".to_string(),
            change_kind: ChangeKind::Modified,
            secondary_hash: Some(secondary_hash),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: now_ms(),
            created_unix_ms: now_ms(),
        })
        .unwrap();

    let results = execute_return_sync(
        &task,
        &["file.txt".to_string()],
        &current_primary,
        &baselines,
        dir.path(),
        &conn,
    );

    assert!(results[0].success, "no conflict should allow return-sync");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "new secondary"
    );

    // Pending should be removed
    let count = pending_repo.count_by_task(&task.id).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_return_sync_delete_moves_primary_to_history_and_clears_baseline() {
    let conn = setup_db();
    let mut task = create_task(&conn);
    let dir = TempDir::new().unwrap();
    task.remote_path = dir.path().join("secondary").to_string_lossy().to_string();
    std::fs::write(dir.path().join("file.txt"), "primary copy").unwrap();
    let hash = blake3::hash(b"primary copy").to_hex().to_string();

    let mut current_primary = HashMap::new();
    current_primary.insert(
        "file.txt".to_string(),
        FileSnapshot {
            task_id: task.id,
            relative_path: "file.txt".to_string(),
            kind: EntryKind::File,
            size: "primary copy".len() as i64,
            modified_unix_ms: 1000,
            blake3_hash: Some(hash.clone()),
            hash_status: HashStatus::Verified,
            deleted: false,
            is_symlink: false,
        },
    );

    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        primary_hash: Some(hash.clone()),
        primary_hash_status: HashStatus::Verified,
        primary_size: "primary copy".len() as i64,
        secondary_size: "primary copy".len() as i64,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some(hash),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };
    let mut baselines = HashMap::new();
    baselines.insert("file.txt".to_string(), baseline.clone());
    SyncBaselineRepository::new(&conn)
        .upsert(&baseline)
        .unwrap();

    let pending_repo = PendingReturnRepository::new(&conn);
    pending_repo
        .upsert(&PendingReturnChange {
            task_id: task.id,
            relative_path: "file.txt".to_string(),
            change_kind: ChangeKind::Deleted,
            secondary_hash: None,
            secondary_hash_status: HashStatus::Unavailable,
            secondary_modified_unix_ms: now_ms(),
            created_unix_ms: now_ms(),
        })
        .unwrap();

    let results = execute_return_sync(
        &task,
        &["file.txt".to_string()],
        &current_primary,
        &baselines,
        dir.path(),
        &conn,
    );

    assert_eq!(results.len(), 1);
    assert!(results[0].success);
    assert!(!dir.path().join("file.txt").exists());
    assert!(dir.path().join(".lanbridge-history").exists());
    assert_eq!(pending_repo.count_by_task(&task.id).unwrap(), 0);
    assert!(SyncBaselineRepository::new(&conn)
        .get(&task.id, "file.txt")
        .unwrap()
        .is_none());
}
