use lan_folder_sync::core::conflict::{conflict_filename, detect_conflict, ConflictResult};
use lan_folder_sync::core::model::*;
use lan_folder_sync::core::planner::{plan_sync, PlannedAction};
use lan_folder_sync::core::scanner::scan_root;
use lan_folder_sync::history::store::HistoryStore;
use lan_folder_sync::platform::macos::MacPlatform;
use std::path::PathBuf;
use tempfile::TempDir;
use uuid::Uuid;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn setup_test_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    // Create some test files
    std::fs::write(dir.path().join("readme.txt"), "hello world").unwrap();
    std::fs::write(dir.path().join("data.csv"), "a,b,c\n1,2,3").unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("docs").join("guide.md"), "# Guide").unwrap();
    // Create ignored files
    std::fs::write(dir.path().join(".DS_Store"), "ignored").unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".git").join("config"), "git config").unwrap();
    dir
}

// ===== Scanner Tests =====

#[test]
fn test_scanner_finds_files() {
    let dir = setup_test_dir();
    let platform = MacPlatform::with_data_dir(PathBuf::from("/tmp/test"));
    let results = scan_root(dir.path(), &platform).unwrap();

    let paths: Vec<String> = results
        .iter()
        .map(|r| r.snapshot.relative_path.clone())
        .collect();

    assert!(paths.contains(&"readme.txt".to_string()));
    assert!(paths.contains(&"data.csv".to_string()));
    assert!(paths.contains(&"docs/guide.md".to_string()));
}

#[test]
fn test_scanner_skips_ignored() {
    let dir = setup_test_dir();
    let platform = MacPlatform::with_data_dir(PathBuf::from("/tmp/test"));
    let results = scan_root(dir.path(), &platform).unwrap();

    let paths: Vec<String> = results
        .iter()
        .map(|r| r.snapshot.relative_path.clone())
        .collect();

    assert!(!paths.contains(&".DS_Store".to_string()));
    assert!(!paths.contains(&".git".to_string()));
    assert!(!paths.contains(&".git/config".to_string()));
}

#[test]
fn test_scanner_hashes_small_files() {
    let dir = setup_test_dir();
    let platform = MacPlatform::with_data_dir(PathBuf::from("/tmp/test"));
    let results = scan_root(dir.path(), &platform).unwrap();

    let readme = results
        .iter()
        .find(|r| r.snapshot.relative_path == "readme.txt")
        .unwrap();

    assert_eq!(readme.snapshot.hash_status, HashStatus::Verified);
    assert!(readme.snapshot.blake3_hash.is_some());
    assert_eq!(readme.snapshot.size, 11); // "hello world"
}

// ===== Planner Tests =====

#[test]
fn test_planner_new_primary_file() {
    let snap = FileSnapshot {
        task_id: Uuid::nil(),
        relative_path: "new.txt".to_string(),
        kind: EntryKind::File,
        size: 100,
        modified_unix_ms: now_ms(),
        blake3_hash: Some("hash1".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let actions = plan_sync(&[snap], &[], DeviceRole::Primary);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].decision, SyncDecision::ApplyToSecondary);
}

#[test]
fn test_planner_secondary_new_file() {
    let snap = FileSnapshot {
        task_id: Uuid::nil(),
        relative_path: "new.txt".to_string(),
        kind: EntryKind::File,
        size: 100,
        modified_unix_ms: now_ms(),
        blake3_hash: Some("hash1".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let actions = plan_sync(&[snap], &[], DeviceRole::Secondary);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].decision, SyncDecision::MarkPendingReturn);
}

#[test]
fn test_planner_primary_delete() {
    let baseline = SyncBaseline {
        task_id: Uuid::nil(),
        relative_path: "old.txt".to_string(),
        primary_hash: Some("hash1".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("hash1".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };

    // Empty snapshots = file was deleted
    let actions = plan_sync(&[], &[baseline], DeviceRole::Primary);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].decision, SyncDecision::MoveSecondaryToHistory);
}

#[test]
fn test_planner_secondary_delete_ignored() {
    let baseline = SyncBaseline {
        task_id: Uuid::nil(),
        relative_path: "old.txt".to_string(),
        primary_hash: Some("hash1".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("hash1".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };

    // Secondary delete does NOT affect primary
    let actions = plan_sync(&[], &[baseline], DeviceRole::Secondary);
    assert!(actions.is_empty());
}

#[test]
fn test_planner_unchanged_file_noop() {
    let snap = FileSnapshot {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        kind: EntryKind::File,
        size: 100,
        modified_unix_ms: 1000,
        blake3_hash: Some("same_hash".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let baseline = SyncBaseline {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        primary_hash: Some("same_hash".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("same_hash".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };

    let actions = plan_sync(&[snap], &[baseline], DeviceRole::Primary);
    assert!(actions.is_empty());
}

// ===== Conflict Tests =====

#[test]
fn test_conflict_no_baseline() {
    let pending = PendingReturnChange {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        change_kind: ChangeKind::Modified,
        secondary_hash: Some("hash2".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 2000,
        created_unix_ms: now_ms(),
    };

    // No baseline = no conflict (new file)
    let result = detect_conflict(&pending, None, None);
    assert!(matches!(result, ConflictResult::NoConflict));
}

#[test]
fn test_conflict_primary_changed() {
    let pending = PendingReturnChange {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        change_kind: ChangeKind::Modified,
        secondary_hash: Some("hash2".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 2000,
        created_unix_ms: now_ms(),
    };

    let primary = FileSnapshot {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        kind: EntryKind::File,
        size: 200,
        modified_unix_ms: 3000,
        blake3_hash: Some("hash_changed".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let baseline = SyncBaseline {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        primary_hash: Some("hash_original".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("hash1".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };

    let result = detect_conflict(&pending, Some(&primary), Some(&baseline));
    assert!(matches!(result, ConflictResult::Conflict { .. }));
}

#[test]
fn test_conflict_mtime_changed_but_hash_same() {
    let pending = PendingReturnChange {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        change_kind: ChangeKind::Modified,
        secondary_hash: Some("hash2".to_string()),
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
        blake3_hash: Some("same_hash".to_string()), // but hash is same
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    let baseline = SyncBaseline {
        task_id: Uuid::nil(),
        relative_path: "file.txt".to_string(),
        primary_hash: Some("same_hash".to_string()),
        primary_hash_status: HashStatus::Verified,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some("hash1".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };

    // Hash same = NOT a conflict even though mtime changed
    let result = detect_conflict(&pending, Some(&primary), Some(&baseline));
    assert!(matches!(result, ConflictResult::NoConflict));
}

// ===== History Store Tests =====

#[test]
fn test_history_move_to_trash() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("delete_me.txt");
    std::fs::write(&source, "important data").unwrap();

    let store = HistoryStore::new(dir.path());
    let entry = store.move_to_trash(&source, "delete_me.txt", now_ms()).unwrap();

    assert!(!source.exists()); // Source moved
    assert_eq!(entry.reason, HistoryReason::Trash);
    assert!(entry.stored_path.contains("trash") || entry.stored_path.contains("trash\\"));
    assert!(std::path::Path::new(&entry.stored_path).exists());
}

#[test]
fn test_history_move_to_overwritten() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("old_version.txt");
    std::fs::write(&source, "old content").unwrap();

    let store = HistoryStore::new(dir.path());
    let entry = store.move_to_overwritten(&source, "old_version.txt", now_ms()).unwrap();

    assert!(!source.exists());
    assert_eq!(entry.reason, HistoryReason::Overwritten);
    assert!(entry.stored_path.contains("overwritten") || entry.stored_path.contains("overwritten\\"));
}

#[test]
fn test_history_restore_to_original() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("file.txt");
    std::fs::write(&source, "data").unwrap();

    let store = HistoryStore::new(dir.path());
    let entry = store.move_to_trash(&source, "file.txt", now_ms()).unwrap();

    // Restore when original path is free
    let restored = store.restore(&entry, dir.path(), now_ms()).unwrap();
    assert!(restored.exists());
    assert_eq!(std::fs::read_to_string(&restored).unwrap(), "data");
}

#[test]
fn test_history_restore_conflict_name() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("file.txt");
    std::fs::write(&source, "old data").unwrap();

    let store = HistoryStore::new(dir.path());
    let entry = store.move_to_trash(&source, "file.txt", now_ms()).unwrap();

    // Create a new file at the original path
    std::fs::write(dir.path().join("file.txt"), "new data").unwrap();

    // Restore should use timestamped name
    let restored = store.restore(&entry, dir.path(), now_ms()).unwrap();
    assert!(restored.exists());
    assert!(restored.to_string_lossy().contains("restored"));
    assert_eq!(std::fs::read_to_string(&restored).unwrap(), "old data");
    // Original file unchanged
    assert_eq!(std::fs::read_to_string(dir.path().join("file.txt")).unwrap(), "new data");
}

#[test]
fn test_conflict_filename() {
    let result = conflict_filename("doc.txt", "MacBook", 1715400000000, |_| false);
    assert!(result.starts_with("doc (conflict from MacBook"));
    assert!(result.ends_with(").txt"));
}
