#![cfg(target_os = "macos")]

use lanbridge::core::conflict;
use lanbridge::core::executor;
use lanbridge::core::model::*;
use lanbridge::core::planner;
use lanbridge::core::scanner::scan_root;
use lanbridge::history::store::HistoryStore;
use lanbridge::platform::macos::MacPlatform;
use lanbridge::state::db;
use lanbridge::state::repository::*;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;
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

fn create_task(conn: &Connection, primary_dir: &Path, secondary_dir: &Path) -> SyncTask {
    let task = SyncTask {
        id: Uuid::new_v4(),
        name: "Integration Test Sync".to_string(),
        primary_device_id: "device-primary".to_string(),
        secondary_device_id: "device-secondary".to_string(),
        local_path: primary_dir.to_string_lossy().to_string(),
        remote_path: secondary_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };
    let repo = SyncTaskRepository::new(conn);
    repo.insert(&task).unwrap();
    task
}

/// Helper: scan a directory, update snapshots in DB, return scan results
fn scan_and_store(
    task: &SyncTask,
    sync_root: &Path,
    conn: &Connection,
    platform: &MacPlatform,
) -> Vec<FileSnapshot> {
    let results = scan_root(sync_root, platform).unwrap();
    let snap_repo = FileSnapshotRepository::new(conn);
    let mut snapshots = Vec::new();
    for r in &results {
        let mut snap = r.snapshot.clone();
        snap.task_id = task.id;
        snap_repo.upsert(&snap).unwrap();
        snapshots.push(snap);
    }
    snapshots
}

/// Helper: plan and execute actions, simulating primary-to-secondary sync
fn plan_and_execute_primary(
    task: &SyncTask,
    snapshots: &[FileSnapshot],
    conn: &Connection,
    sync_root: &Path,
) -> Vec<executor::ExecutionResult> {
    let baseline_repo = SyncBaselineRepository::new(conn);
    let mut baselines = Vec::new();
    for snap in snapshots {
        if let Some(b) = baseline_repo.get(&task.id, &snap.relative_path).unwrap() {
            baselines.push(b);
        }
    }
    // Also get baselines for files no longer in snapshots (detect deletes)
    let all_baselines_in_db = get_all_baselines_for_task(conn, &task.id);
    for b in &all_baselines_in_db {
        if !baselines
            .iter()
            .any(|existing| existing.relative_path == b.relative_path)
        {
            baselines.push(b.clone());
        }
    }

    let actions = planner::plan_sync(snapshots, &baselines, task.local_role);
    executor::execute_actions(&actions, task, sync_root, conn)
}

fn get_all_baselines_for_task(conn: &Connection, task_id: &Uuid) -> Vec<SyncBaseline> {
    let mut stmt = conn
        .prepare("SELECT task_id, relative_path, primary_hash, primary_hash_status, primary_size, secondary_size, primary_modified_unix_ms, secondary_hash, secondary_hash_status, secondary_modified_unix_ms, last_synced_unix_ms FROM sync_baselines WHERE task_id = ?1")
        .unwrap();
    let rows = stmt
        .query_map(rusqlite::params![task_id.to_string()], |row| {
            let parse_hs = |s: String| match s.as_str() {
                "Verified" => HashStatus::Verified,
                "UnverifiedLargeFile" => HashStatus::UnverifiedLargeFile,
                _ => HashStatus::Unavailable,
            };
            Ok(SyncBaseline {
                task_id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                relative_path: row.get(1)?,
                primary_hash: row.get(2)?,
                primary_hash_status: parse_hs(row.get(3)?),
                primary_size: row.get(4)?,
                secondary_size: row.get(5)?,
                primary_modified_unix_ms: row.get(6)?,
                secondary_hash: row.get(7)?,
                secondary_hash_status: parse_hs(row.get(8)?),
                secondary_modified_unix_ms: row.get(9)?,
                last_synced_unix_ms: row.get(10)?,
            })
        })
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

// ═══════════════════════════════════════════════════════════════
// Test 1: Primary create flows to secondary
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_primary_create_syncs_to_secondary() {
    let conn = setup_db();
    let primary_dir = TempDir::new().unwrap();
    let secondary_dir = TempDir::new().unwrap();
    let platform = MacPlatform::with_data_dir(std::env::temp_dir().join("test_primary_create"));
    let task = create_task(&conn, primary_dir.path(), secondary_dir.path());

    // Step 1: Create a file on primary
    std::fs::write(primary_dir.path().join("hello.txt"), "hello world").unwrap();

    // Step 2: Scan primary
    let snaps = scan_and_store(&task, primary_dir.path(), &conn, &platform);

    // Step 3: Plan and execute — should ApplyToSecondary
    let results = plan_and_execute_primary(&task, &snaps, &conn, primary_dir.path());
    assert_eq!(results.len(), 1);
    assert!(
        results[0].success,
        "apply should succeed: {:?}",
        results[0].error
    );

    // Step 4: Verify baseline was created
    let baseline_repo = SyncBaselineRepository::new(&conn);
    let baseline = baseline_repo.get(&task.id, "hello.txt").unwrap();
    assert!(baseline.is_some(), "baseline should exist after sync");
    assert!(
        baseline.unwrap().primary_hash.is_some(),
        "baseline should have hash"
    );
}

// ═══════════════════════════════════════════════════════════════
// Test 2: Primary update syncs to secondary
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_primary_update_syncs_to_secondary() {
    let conn = setup_db();
    let primary_dir = TempDir::new().unwrap();
    let secondary_dir = TempDir::new().unwrap();
    let platform = MacPlatform::with_data_dir(std::env::temp_dir().join("test_primary_update"));
    let task = create_task(&conn, primary_dir.path(), secondary_dir.path());

    // Initial sync: create file
    std::fs::write(primary_dir.path().join("doc.txt"), "version 1").unwrap();
    let snaps = scan_and_store(&task, primary_dir.path(), &conn, &platform);
    let results = plan_and_execute_primary(&task, &snaps, &conn, primary_dir.path());
    assert!(results[0].success);

    // Simulate secondary receiving the file
    std::fs::write(secondary_dir.path().join("doc.txt"), "version 1").unwrap();

    // Update file on primary
    std::fs::write(primary_dir.path().join("doc.txt"), "version 2 updated").unwrap();

    // Re-scan primary — should detect change
    let snaps2 = scan_and_store(&task, primary_dir.path(), &conn, &platform);
    let changed_snap = snaps2
        .iter()
        .find(|s| s.relative_path == "doc.txt")
        .unwrap();

    // Hash should differ because content changed
    assert_ne!(
        changed_snap.blake3_hash, snaps[0].blake3_hash,
        "content changed so hash should differ"
    );

    // Planner should detect the change and emit ApplyToSecondary
    let baseline_repo = SyncBaselineRepository::new(&conn);
    let mut baselines = Vec::new();
    if let Some(b) = baseline_repo.get(&task.id, "doc.txt").unwrap() {
        baselines.push(b);
    }
    let actions = planner::plan_sync(&snaps2, &baselines, DeviceRole::Primary);
    assert_eq!(actions.len(), 1, "should detect file change");
    assert_eq!(actions[0].decision, SyncDecision::ApplyToSecondary);
}

// ═══════════════════════════════════════════════════════════════
// Test 3: Primary delete moves secondary file to history
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_primary_delete_moves_secondary_to_history() {
    let conn = setup_db();
    let primary_dir = TempDir::new().unwrap();
    let secondary_dir = TempDir::new().unwrap();
    let platform = MacPlatform::with_data_dir(std::env::temp_dir().join("test_primary_delete"));
    let task = create_task(&conn, primary_dir.path(), secondary_dir.path());

    // Step 1: Create and sync
    std::fs::write(primary_dir.path().join("delete_me.txt"), "important data").unwrap();
    let snaps = scan_and_store(&task, primary_dir.path(), &conn, &platform);
    let results = plan_and_execute_primary(&task, &snaps, &conn, primary_dir.path());
    assert!(results[0].success);

    // Simulate secondary has the file
    std::fs::write(secondary_dir.path().join("delete_me.txt"), "important data").unwrap();

    // Step 2: Delete on primary
    std::fs::remove_file(primary_dir.path().join("delete_me.txt")).unwrap();

    // Step 3: Re-scan primary (empty snapshots = deleted)
    let snaps2 = scan_and_store(&task, primary_dir.path(), &conn, &platform);
    assert!(snaps2.iter().all(|s| s.relative_path != "delete_me.txt"));

    // Step 4: Plan — should be MoveSecondaryToHistory
    let baseline_repo = SyncBaselineRepository::new(&conn);
    let mut baselines = Vec::new();
    if let Some(b) = baseline_repo.get(&task.id, "delete_me.txt").unwrap() {
        baselines.push(b);
    }
    let actions = planner::plan_sync(&snaps2, &baselines, DeviceRole::Primary);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].decision, SyncDecision::MoveSecondaryToHistory);

    // Step 5: Execute — should move secondary file to history
    let results = executor::execute_actions(&actions, &task, secondary_dir.path(), &conn);
    assert!(results[0].success);

    // File should be gone from secondary
    assert!(!secondary_dir.path().join("delete_me.txt").exists());

    // History entry should exist
    let history_repo = HistoryRepository::new(&conn);
    let entries = history_repo.list_by_task(&task.id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].reason, HistoryReason::Trash);
    assert_eq!(entries[0].original_relative_path, "delete_me.txt");
}

// ═══════════════════════════════════════════════════════════════
// Test 4: Secondary create becomes pending return-sync
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_secondary_create_becomes_pending_return() {
    let conn = setup_db();
    let primary_dir = TempDir::new().unwrap();
    let secondary_dir = TempDir::new().unwrap();
    let platform = MacPlatform::with_data_dir(std::env::temp_dir().join("test_sec_create"));
    let mut task = create_task(&conn, primary_dir.path(), secondary_dir.path());
    task.local_role = DeviceRole::Secondary;

    // Step 1: Create file on secondary
    std::fs::write(
        secondary_dir.path().join("new_on_secondary.txt"),
        "secondary content",
    )
    .unwrap();

    // Step 2: Scan secondary
    let snaps = scan_and_store(&task, secondary_dir.path(), &conn, &platform);

    // Step 3: Plan as secondary — should be MarkPendingReturn
    let actions = planner::plan_sync(&snaps, &[], DeviceRole::Secondary);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].decision, SyncDecision::MarkPendingReturn);

    // Step 4: Execute
    let results = executor::execute_actions(&actions, &task, primary_dir.path(), &conn);
    assert!(results[0].success);

    // Step 5: Verify pending return was recorded
    let pending_repo = PendingReturnRepository::new(&conn);
    let pending = pending_repo.list_by_task(&task.id).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].relative_path, "new_on_secondary.txt");
    assert_eq!(pending[0].change_kind, ChangeKind::Created);
}

// ═══════════════════════════════════════════════════════════════
// Test 5: Secondary delete does NOT affect primary
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_secondary_delete_does_not_affect_primary() {
    let conn = setup_db();
    let primary_dir = TempDir::new().unwrap();
    let secondary_dir = TempDir::new().unwrap();
    let platform = MacPlatform::with_data_dir(std::env::temp_dir().join("test_sec_delete"));
    let mut task = create_task(&conn, primary_dir.path(), secondary_dir.path());
    task.local_role = DeviceRole::Secondary;

    // Setup: both sides have the file (simulating previous primary sync)
    let baseline = SyncBaseline {
        task_id: task.id,
        relative_path: "shared_file.txt".to_string(),
        primary_hash: Some("hash1".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_size: 100,
        secondary_size: 100,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("hash1".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };

    // Step 1: Delete on secondary — empty snapshots
    let snaps = scan_and_store(&task, secondary_dir.path(), &conn, &platform);

    // Step 2: Plan as secondary — should become an explicit pending delete request.
    let actions = planner::plan_sync(&snaps, &[baseline], DeviceRole::Secondary);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].decision, SyncDecision::MarkPendingReturn);

    // Primary file should be untouched
    assert!(primary_dir.path().exists());
}

// ═══════════════════════════════════════════════════════════════
// Test 6: Return-sync conflict detection and resolution
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_return_sync_conflict_blocks_overwrite() {
    let conn = setup_db();
    let primary_dir = TempDir::new().unwrap();
    let task = create_task(&conn, primary_dir.path(), primary_dir.path());

    // Setup: file exists on primary with a changed hash since baseline
    let mut current_primary = HashMap::new();
    current_primary.insert(
        "conflicted.txt".to_string(),
        FileSnapshot {
            task_id: task.id,
            relative_path: "conflicted.txt".to_string(),
            kind: EntryKind::File,
            size: 200,
            modified_unix_ms: now_ms(),
            blake3_hash: Some("primary_changed_hash".to_string()),
            hash_status: HashStatus::Verified,
            deleted: false,
            is_symlink: false,
        },
    );

    let mut baselines = HashMap::new();
    baselines.insert(
        "conflicted.txt".to_string(),
        SyncBaseline {
            task_id: task.id,
            relative_path: "conflicted.txt".to_string(),
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

    // Record pending return from secondary
    let pending_repo = PendingReturnRepository::new(&conn);
    pending_repo
        .upsert(&PendingReturnChange {
            task_id: task.id,
            relative_path: "conflicted.txt".to_string(),
            change_kind: ChangeKind::Modified,
            secondary_hash: Some("sec_hash".to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: now_ms(),
            created_unix_ms: now_ms(),
        })
        .unwrap();

    // Try return-sync — should be blocked
    let results = executor::execute_return_sync(
        &task,
        &["conflicted.txt".to_string()],
        &current_primary,
        &baselines,
        primary_dir.path(),
        &conn,
    );

    assert_eq!(results.len(), 1);
    assert!(!results[0].success, "conflict should block return-sync");
    assert!(results[0].error.as_ref().unwrap().contains("conflict"));
}

// ═══════════════════════════════════════════════════════════════
// Test 7: Confirmed overwrite backs up old primary file
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_confirmed_overwrite_creates_backup() {
    let conn = setup_db();
    let primary_dir = TempDir::new().unwrap();
    let secondary_dir = TempDir::new().unwrap();
    let task = create_task(&conn, primary_dir.path(), secondary_dir.path());

    // Create existing primary file
    let file_path = primary_dir.path().join("overwrite.txt");
    std::fs::write(&file_path, "original primary content").unwrap();
    std::fs::write(
        secondary_dir.path().join("overwrite.txt"),
        "incoming content",
    )
    .unwrap();

    // Execute confirmed overwrite
    let result =
        executor::execute_confirmed_overwrite(&task, "overwrite.txt", primary_dir.path(), &conn);

    assert!(result.success, "overwrite should succeed");

    // Verify history backup was created
    let history_repo = HistoryRepository::new(&conn);
    let entries = history_repo.list_by_task(&task.id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].reason, HistoryReason::Overwritten);
    assert_eq!(entries[0].original_relative_path, "overwrite.txt");

    // Verify the backup file exists on disk
    assert!(Path::new(&entries[0].stored_path).exists());
}

// ═══════════════════════════════════════════════════════════════
// Test 8: History restore to original path
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_history_restore_to_original_path() {
    let dir = TempDir::new().unwrap();
    let store = HistoryStore::new(dir.path());

    // Create and trash a file
    let source = dir.path().join("restore_me.txt");
    std::fs::write(&source, "important data").unwrap();
    let entry = store
        .move_to_trash(&source, "restore_me.txt", now_ms())
        .unwrap();

    // Source should be gone
    assert!(!source.exists());

    // Restore to original path
    let restored = store.restore(&entry, dir.path(), now_ms()).unwrap();
    assert!(restored.exists());
    assert_eq!(
        std::fs::read_to_string(&restored).unwrap(),
        "important data"
    );
}

// ═══════════════════════════════════════════════════════════════
// Test 9: History restore when original path is occupied
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_history_restore_creates_conflict_safe_name() {
    let dir = TempDir::new().unwrap();
    let store = HistoryStore::new(dir.path());

    // Create and trash a file
    let source = dir.path().join("conflict.txt");
    std::fs::write(&source, "old version").unwrap();
    let entry = store
        .move_to_trash(&source, "conflict.txt", now_ms())
        .unwrap();

    // Create a new file at the original path
    std::fs::write(dir.path().join("conflict.txt"), "new version").unwrap();

    // Restore — should create timestamped name
    let restored = store.restore(&entry, dir.path(), now_ms()).unwrap();
    assert!(restored.exists());
    assert!(restored.to_string_lossy().contains("restored"));
    assert_eq!(std::fs::read_to_string(&restored).unwrap(), "old version");

    // Original file unchanged
    assert_eq!(
        std::fs::read_to_string(dir.path().join("conflict.txt")).unwrap(),
        "new version"
    );
}

// ═══════════════════════════════════════════════════════════════
// Test 10: Full round-trip — create, sync, delete, history
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_full_roundtrip_create_sync_delete_restore() {
    let conn = setup_db();
    let primary_dir = TempDir::new().unwrap();
    let secondary_dir = TempDir::new().unwrap();
    let platform = MacPlatform::with_data_dir(std::env::temp_dir().join("test_roundtrip"));
    let task = create_task(&conn, primary_dir.path(), secondary_dir.path());

    // 1. Create file on primary
    std::fs::write(primary_dir.path().join("roundtrip.txt"), "step 1").unwrap();

    // 2. Scan and sync to secondary
    let snaps = scan_and_store(&task, primary_dir.path(), &conn, &platform);
    let results = plan_and_execute_primary(&task, &snaps, &conn, primary_dir.path());
    assert!(results[0].success);

    // 3. Simulate secondary receiving the file
    std::fs::write(secondary_dir.path().join("roundtrip.txt"), "step 1").unwrap();

    // 4. Delete on primary
    std::fs::remove_file(primary_dir.path().join("roundtrip.txt")).unwrap();

    // 5. Re-scan and plan delete
    let snaps2 = scan_and_store(&task, primary_dir.path(), &conn, &platform);
    let baseline_repo = SyncBaselineRepository::new(&conn);
    let mut baselines = Vec::new();
    if let Some(b) = baseline_repo.get(&task.id, "roundtrip.txt").unwrap() {
        baselines.push(b);
    }
    let actions = planner::plan_sync(&snaps2, &baselines, DeviceRole::Primary);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].decision, SyncDecision::MoveSecondaryToHistory);

    // 6. Execute delete — moves to history
    let results = executor::execute_actions(&actions, &task, secondary_dir.path(), &conn);
    assert!(results[0].success);
    assert!(!secondary_dir.path().join("roundtrip.txt").exists());

    // 7. Verify and restore from history
    let history_repo = HistoryRepository::new(&conn);
    let entries = history_repo.list_by_task(&task.id).unwrap();
    assert_eq!(entries.len(), 1);

    let store = HistoryStore::new(secondary_dir.path());
    let restored = store
        .restore(&entries[0], secondary_dir.path(), now_ms())
        .unwrap();
    assert!(restored.exists());
    assert_eq!(std::fs::read_to_string(&restored).unwrap(), "step 1");
}

// ═══════════════════════════════════════════════════════════════
// Test 11: Hash-verified conflict detection — same hash, no conflict
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_same_hash_not_a_conflict() {
    let pending = PendingReturnChange {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        change_kind: ChangeKind::Modified,
        secondary_hash: Some("hash_new".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 2000,
        created_unix_ms: now_ms(),
    };

    let primary = FileSnapshot {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        kind: EntryKind::File,
        size: 100,
        modified_unix_ms: 5000, // mtime changed
        blake3_hash: Some("same_hash".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let baseline = SyncBaseline {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        primary_hash: Some("same_hash".to_string()), // same hash as current
        primary_hash_status: HashStatus::Verified,
        primary_size: 100,
        secondary_size: 100,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("old_hash".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };

    // Same hash = NOT a conflict even though mtime changed
    let result = conflict::detect_conflict(&pending, Some(&primary), Some(&baseline));
    assert!(
        matches!(result, conflict::ConflictResult::NoConflict),
        "same hash should not be a conflict"
    );
}
