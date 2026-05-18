use lanbridge::core::model::*;
use lanbridge::state::db;
use lanbridge::state::repository::*;
use rusqlite::Connection;
use std::time::{SystemTime, UNIX_EPOCH};
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

#[test]
fn test_sync_task_crud() {
    let conn = setup_db();
    let repo = SyncTaskRepository::new(&conn);

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

    repo.insert(&task).unwrap();

    let loaded = repo.get(&task.id).unwrap().unwrap();
    assert_eq!(loaded.name, "Test Sync");
    assert_eq!(loaded.local_role, DeviceRole::Primary);
    assert!(loaded.enabled);

    let all = repo.list_all().unwrap();
    assert_eq!(all.len(), 1);

    repo.update_enabled(&task.id, false, now_ms()).unwrap();
    let updated = repo.get(&task.id).unwrap().unwrap();
    assert!(!updated.enabled);
}

#[test]
fn test_file_snapshot_upsert() {
    let conn = setup_db();
    let task_repo = SyncTaskRepository::new(&conn);
    let snap_repo = FileSnapshotRepository::new(&conn);

    let task = SyncTask {
        id: Uuid::new_v4(),
        name: "Snap Test".to_string(),
        primary_device_id: "a".to_string(),
        secondary_device_id: "b".to_string(),
        local_path: "/tmp/a".to_string(),
        remote_path: "/tmp/b".to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };
    task_repo.insert(&task).unwrap();

    let snap = FileSnapshot {
        task_id: task.id,
        relative_path: "docs/readme.txt".to_string(),
        kind: EntryKind::File,
        size: 1024,
        modified_unix_ms: now_ms(),
        blake3_hash: Some("abc123".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };
    snap_repo.upsert(&snap).unwrap();

    let loaded = snap_repo.get(&task.id, "docs/readme.txt").unwrap().unwrap();
    assert_eq!(loaded.blake3_hash, Some("abc123".to_string()));
    assert_eq!(loaded.hash_status, HashStatus::Verified);

    // Upsert updates
    let mut updated = snap.clone();
    updated.size = 2048;
    updated.blake3_hash = Some("def456".to_string());
    snap_repo.upsert(&updated).unwrap();

    let loaded2 = snap_repo.get(&task.id, "docs/readme.txt").unwrap().unwrap();
    assert_eq!(loaded2.size, 2048);
    assert_eq!(loaded2.blake3_hash, Some("def456".to_string()));

    let all = snap_repo.list_by_task(&task.id).unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn test_file_snapshot_replace_for_task_removes_stale_paths() {
    let conn = setup_db();
    let task_repo = SyncTaskRepository::new(&conn);
    let snap_repo = FileSnapshotRepository::new(&conn);

    let task = SyncTask {
        id: Uuid::new_v4(),
        name: "Replace Snapshot Test".to_string(),
        primary_device_id: "a".to_string(),
        secondary_device_id: "b".to_string(),
        local_path: "/tmp/a".to_string(),
        remote_path: "/tmp/b".to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };
    task_repo.insert(&task).unwrap();

    let stale = FileSnapshot {
        task_id: task.id,
        relative_path: "deleted.txt".to_string(),
        kind: EntryKind::File,
        size: 1,
        modified_unix_ms: 1,
        blake3_hash: Some("old".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };
    snap_repo.upsert(&stale).unwrap();

    let current = FileSnapshot {
        task_id: task.id,
        relative_path: "current.txt".to_string(),
        kind: EntryKind::File,
        size: 2,
        modified_unix_ms: 2,
        blake3_hash: Some("new".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };
    snap_repo.replace_for_task(&task.id, &[current]).unwrap();

    assert!(snap_repo.get(&task.id, "deleted.txt").unwrap().is_none());
    assert!(snap_repo.get(&task.id, "current.txt").unwrap().is_some());
}

#[test]
fn test_sync_baseline_list_by_task_includes_paths_without_snapshots() {
    let conn = setup_db();
    let task_repo = SyncTaskRepository::new(&conn);
    let baseline_repo = SyncBaselineRepository::new(&conn);

    let task = SyncTask {
        id: Uuid::new_v4(),
        name: "Baseline List Test".to_string(),
        primary_device_id: "a".to_string(),
        secondary_device_id: "b".to_string(),
        local_path: "/tmp/a".to_string(),
        remote_path: "/tmp/b".to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };
    task_repo.insert(&task).unwrap();

    baseline_repo
        .upsert(&SyncBaseline {
            task_id: task.id,
            relative_path: "deleted.txt".to_string(),
            primary_hash: Some("hash".to_string()),
            primary_hash_status: HashStatus::Verified,
            primary_size: 10,
            primary_modified_unix_ms: 100,
            secondary_hash: Some("hash".to_string()),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: 100,
            last_synced_unix_ms: 100,
        })
        .unwrap();

    let baselines = baseline_repo.list_by_task(&task.id).unwrap();
    assert_eq!(baselines.len(), 1);
    assert_eq!(baselines[0].relative_path, "deleted.txt");
}

#[test]
fn test_pending_return_operations() {
    let conn = setup_db();
    let task_repo = SyncTaskRepository::new(&conn);
    let pending_repo = PendingReturnRepository::new(&conn);

    let task = SyncTask {
        id: Uuid::new_v4(),
        name: "Pending Test".to_string(),
        primary_device_id: "a".to_string(),
        secondary_device_id: "b".to_string(),
        local_path: "/tmp/a".to_string(),
        remote_path: "/tmp/b".to_string(),
        local_role: DeviceRole::Secondary,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };
    task_repo.insert(&task).unwrap();

    let change = PendingReturnChange {
        task_id: task.id,
        relative_path: "new_file.txt".to_string(),
        change_kind: ChangeKind::Created,
        secondary_hash: Some("hash1".to_string()),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: now_ms(),
        created_unix_ms: now_ms(),
    };
    pending_repo.upsert(&change).unwrap();

    let count = pending_repo.count_by_task(&task.id).unwrap();
    assert_eq!(count, 1);

    let list = pending_repo.list_by_task(&task.id).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].change_kind, ChangeKind::Created);

    pending_repo.remove(&task.id, "new_file.txt").unwrap();
    let count2 = pending_repo.count_by_task(&task.id).unwrap();
    assert_eq!(count2, 0);
}

#[test]
fn test_history_operations() {
    let conn = setup_db();
    let task_repo = SyncTaskRepository::new(&conn);
    let history_repo = HistoryRepository::new(&conn);

    let task = SyncTask {
        id: Uuid::new_v4(),
        name: "History Test".to_string(),
        primary_device_id: "a".to_string(),
        secondary_device_id: "b".to_string(),
        local_path: "/tmp/a".to_string(),
        remote_path: "/tmp/b".to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };
    task_repo.insert(&task).unwrap();

    let entry = HistoryEntry {
        id: Uuid::new_v4(),
        task_id: task.id,
        original_relative_path: "deleted.txt".to_string(),
        stored_path: ".lanbridge-history/trash/123456/deleted.txt".to_string(),
        reason: HistoryReason::Trash,
        created_unix_ms: now_ms(),
        size: 512,
    };
    history_repo.insert(&entry).unwrap();

    let list = history_repo.list_by_task(&task.id).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].reason, HistoryReason::Trash);

    let total = history_repo.total_size_by_task(&task.id).unwrap();
    assert_eq!(total, 512);
}

#[test]
fn test_paired_device() {
    let conn = setup_db();
    let repo = PairedDeviceRepository::new(&conn);

    let device = PairedDevice {
        device_id: "dev-001".to_string(),
        display_name: "My Mac".to_string(),
        public_key: vec![1, 2, 3, 4],
        last_seen_unix_ms: now_ms(),
        trusted: true,
        last_address: Some("192.168.1.20:9527".to_string()),
    };
    repo.upsert(&device).unwrap();

    let loaded = repo.get("dev-001").unwrap().unwrap();
    assert_eq!(loaded.display_name, "My Mac");
    assert!(loaded.trusted);
    assert_eq!(loaded.last_address.as_deref(), Some("192.168.1.20:9527"));
}

#[test]
fn test_log_retention() {
    let conn = setup_db();
    let repo = LogRepository::new(&conn);

    let t = now_ms();
    for i in 0..15 {
        let entry = LogEntry {
            id: None,
            level: LogLevel::Info,
            task_id: None,
            relative_path: None,
            message: format!("log {}", i),
            created_unix_ms: t + i,
        };
        repo.insert(&entry).unwrap();
    }

    let recent = repo.list_recent(10).unwrap();
    assert_eq!(recent.len(), 10);
    // Most recent first
    assert_eq!(recent[0].message, "log 14");

    // Retention: keep 5 entries, cutoff = t+10
    let deleted = repo.enforce_retention(5, t + 10).unwrap();
    assert!(deleted > 0);

    let after = repo.list_recent(100).unwrap();
    assert!(after.len() <= 5);
}
