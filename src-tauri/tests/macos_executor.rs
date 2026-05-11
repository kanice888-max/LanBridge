use lan_folder_sync::core::executor::*;
use lan_folder_sync::core::model::*;
use lan_folder_sync::core::planner::PlannedAction;
use lan_folder_sync::state::db;
use lan_folder_sync::state::repository::*;
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
    let task = create_task(&conn);
    let dir = TempDir::new().unwrap();

    let snap = FileSnapshot {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        kind: EntryKind::File,
        size: 100,
        modified_unix_ms: now_ms(),
        blake3_hash: Some("hash123".to_string()),
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
    assert_eq!(baseline.unwrap().primary_hash, Some("hash123".to_string()));
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
    let dir = TempDir::new().unwrap();

    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        primary_hash: Some("hash".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("hash".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };

    // Secondary deletes the file — should result in Noop
    let actions = lan_folder_sync::core::planner::plan_sync(
        &[], // empty snapshots = deleted
        &[baseline],
        DeviceRole::Secondary,
    );

    // Secondary delete produces Noop
    assert!(actions.is_empty(), "secondary delete should produce no actions");
}

#[test]
fn test_confirmed_overwrite_backs_up() {
    let conn = setup_db();
    let task = create_task(&conn);
    let dir = TempDir::new().unwrap();

    // Create an existing file
    let file_path = dir.path().join("overwrite_me.txt");
    std::fs::write(&file_path, "original content").unwrap();

    let current = FileSnapshot {
        task_id: task.id,
        relative_path: "overwrite_me.txt".to_string(),
        kind: EntryKind::File,
        size: 100,
        modified_unix_ms: now_ms(),
        blake3_hash: Some("new_hash".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let result = execute_confirmed_overwrite(
        &task,
        "overwrite_me.txt",
        &current,
        dir.path(),
        &conn,
    );

    assert!(result.success);

    // Verify history entry for backup
    let history_repo = HistoryRepository::new(&conn);
    let entries = history_repo.list_by_task(&task.id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].reason, HistoryReason::Overwritten);
}

#[test]
fn test_return_sync_conflict_blocked() {
    let conn = setup_db();
    let task = create_task(&conn);
    let dir = TempDir::new().unwrap();

    // Setup: file exists on primary with changed hash
    let mut current_primary = HashMap::new();
    current_primary.insert("file.txt".to_string(), FileSnapshot {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        kind: EntryKind::File,
        size: 200,
        modified_unix_ms: now_ms(),
        blake3_hash: Some("changed_hash".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    });

    // Baseline has different hash
    let mut baselines = HashMap::new();
    baselines.insert("file.txt".to_string(), SyncBaseline {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        primary_hash: Some("original_hash".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("original_hash".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    });

    // Record pending return
    let pending_repo = PendingReturnRepository::new(&conn);
    pending_repo.upsert(&PendingReturnChange {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        change_kind: ChangeKind::Modified,
        secondary_hash: Some("sec_hash".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: now_ms(),
        created_unix_ms: now_ms(),
    }).unwrap();

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
fn test_return_sync_no_conflict_succeeds() {
    let conn = setup_db();
    let task = create_task(&conn);
    let dir = TempDir::new().unwrap();

    // No change on primary — same hash as baseline
    let mut current_primary = HashMap::new();
    current_primary.insert("file.txt".to_string(), FileSnapshot {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        kind: EntryKind::File,
        size: 100,
        modified_unix_ms: 1000,
        blake3_hash: Some("same_hash".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    });

    let mut baselines = HashMap::new();
    baselines.insert("file.txt".to_string(), SyncBaseline {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        primary_hash: Some("same_hash".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("same_hash".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    });

    let pending_repo = PendingReturnRepository::new(&conn);
    pending_repo.upsert(&PendingReturnChange {
        task_id: task.id,
        relative_path: "file.txt".to_string(),
        change_kind: ChangeKind::Modified,
        secondary_hash: Some("new_sec_hash".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: now_ms(),
        created_unix_ms: now_ms(),
    }).unwrap();

    let results = execute_return_sync(
        &task,
        &["file.txt".to_string()],
        &current_primary,
        &baselines,
        dir.path(),
        &conn,
    );

    assert!(results[0].success, "no conflict should allow return-sync");

    // Pending should be removed
    let count = pending_repo.count_by_task(&task.id).unwrap();
    assert_eq!(count, 0);
}
