use lanbridge::core::model::*;
use lanbridge::state::db;
use lanbridge::state::repository::*;
use rusqlite::{params, Connection};
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

#[test]
fn peer_connection_state_persists_and_rejects_stale_revisions() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.db");
    {
        let conn = db::open_db(&path).unwrap();
        db::migrate(&conn).unwrap();
        PairedDeviceRepository::new(&conn)
            .upsert(&PairedDevice {
                device_id: "peer-a".to_string(),
                display_name: "Peer A".to_string(),
                public_key: vec![1, 2, 3],
                last_seen_unix_ms: 1,
                trusted: true,
                last_address: Some("192.168.1.5:9527".to_string()),
            })
            .unwrap();
        let repo = PeerConnectionStateRepository::new(&conn);
        let disconnected = repo.set_local_disconnected("peer-a", true, 10).unwrap();
        assert!(disconnected.local_disconnected);
        assert_eq!(disconnected.local_revision, 1);

        let duplicate = repo.set_local_disconnected("peer-a", true, 11).unwrap();
        assert_eq!(duplicate.local_revision, 1);

        assert!(repo
            .apply_remote_disconnected("peer-a", true, 5, 12)
            .unwrap());
        assert!(!repo
            .apply_remote_disconnected("peer-a", false, 4, 13)
            .unwrap());
    }

    let conn = db::open_db(&path).unwrap();
    db::migrate(&conn).unwrap();
    let restored = PeerConnectionStateRepository::new(&conn)
        .get("peer-a")
        .unwrap()
        .unwrap();
    assert!(restored.local_disconnected);
    assert_eq!(restored.local_revision, 1);
    assert!(restored.remote_disconnected);
    assert_eq!(restored.remote_revision, Some(5));

    let reconnected = PeerConnectionStateRepository::new(&conn)
        .set_local_disconnected("peer-a", false, 14)
        .unwrap();
    assert!(!reconnected.local_disconnected);
    assert_eq!(reconnected.local_revision, 2);
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
        last_transfer_activity_unix_ms: 0,
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
fn test_sync_task_bad_uuid_returns_error_without_panic() {
    let conn = setup_db();
    conn.execute(
        "INSERT INTO sync_tasks (id, name, primary_device_id, secondary_device_id, local_path, remote_path, local_role, enabled, created_unix_ms, updated_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            "not-a-uuid",
            "Bad UUID",
            "a",
            "b",
            "/tmp/a",
            "/tmp/b",
            "Primary",
            1,
            now_ms(),
            now_ms(),
        ],
    )
    .unwrap();

    let result = SyncTaskRepository::new(&conn).list_all();
    assert!(result.is_err());
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
        last_transfer_activity_unix_ms: 0,
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
        last_transfer_activity_unix_ms: 0,
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
fn test_file_snapshot_replace_for_task_rolls_back_on_failure() {
    let conn = setup_db();
    let task_repo = SyncTaskRepository::new(&conn);
    let snap_repo = FileSnapshotRepository::new(&conn);

    let task = SyncTask {
        id: Uuid::new_v4(),
        name: "Replace Snapshot Rollback Test".to_string(),
        primary_device_id: "a".to_string(),
        secondary_device_id: "b".to_string(),
        local_path: "/tmp/a".to_string(),
        remote_path: "/tmp/b".to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
        last_transfer_activity_unix_ms: 0,
    };
    task_repo.insert(&task).unwrap();

    let stale = FileSnapshot {
        task_id: task.id,
        relative_path: "keep.txt".to_string(),
        kind: EntryKind::File,
        size: 1,
        modified_unix_ms: 1,
        blake3_hash: Some("old".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };
    snap_repo.upsert(&stale).unwrap();

    let invalid = FileSnapshot {
        task_id: Uuid::new_v4(),
        relative_path: "invalid.txt".to_string(),
        kind: EntryKind::File,
        size: 2,
        modified_unix_ms: 2,
        blake3_hash: Some("new".to_string()),
        hash_status: HashStatus::Verified,
        deleted: false,
        is_symlink: false,
    };

    assert!(snap_repo.replace_for_task(&task.id, &[invalid]).is_err());
    assert!(snap_repo.get(&task.id, "keep.txt").unwrap().is_some());
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
        last_transfer_activity_unix_ms: 0,
    };
    task_repo.insert(&task).unwrap();

    baseline_repo
        .upsert(&SyncBaseline {
            task_id: task.id,
            relative_path: "deleted.txt".to_string(),
            primary_hash: Some("hash".to_string()),
            primary_hash_status: HashStatus::Verified,
            primary_size: 10,
            secondary_size: 10,
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
        last_transfer_activity_unix_ms: 0,
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
fn test_remove_tree_escapes_like_wildcards() {
    let conn = setup_db();
    let task_repo = SyncTaskRepository::new(&conn);
    let task = SyncTask {
        id: Uuid::new_v4(),
        name: "LIKE escaping".to_string(),
        primary_device_id: "a".to_string(),
        secondary_device_id: "b".to_string(),
        local_path: "/tmp/a".to_string(),
        remote_path: "/tmp/b".to_string(),
        local_role: DeviceRole::Secondary,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
        last_transfer_activity_unix_ms: 0,
    };
    task_repo.insert(&task).unwrap();

    let snapshot_repo = FileSnapshotRepository::new(&conn);
    let baseline_repo = SyncBaselineRepository::new(&conn);
    let pending_repo = PendingReturnRepository::new(&conn);
    for path in ["a%b/child.txt", "aXb/child.txt", "a_b/child.txt"] {
        snapshot_repo
            .upsert(&FileSnapshot {
                task_id: task.id,
                relative_path: path.to_string(),
                kind: EntryKind::File,
                size: 1,
                modified_unix_ms: 1,
                blake3_hash: Some("hash".to_string()),
                hash_status: HashStatus::Verified,
                deleted: false,
                is_symlink: false,
            })
            .unwrap();
        baseline_repo
            .upsert(&SyncBaseline {
                task_id: task.id,
                relative_path: path.to_string(),
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
        pending_repo
            .upsert(&PendingReturnChange {
                task_id: task.id,
                relative_path: path.to_string(),
                change_kind: ChangeKind::Modified,
                secondary_hash: Some("hash".to_string()),
                secondary_hash_status: HashStatus::Verified,
                secondary_modified_unix_ms: 1,
                created_unix_ms: 1,
            })
            .unwrap();
    }

    snapshot_repo.remove_tree(&task.id, "a%b").unwrap();
    baseline_repo.remove_tree(&task.id, "a%b").unwrap();
    pending_repo.remove_tree(&task.id, "a%b").unwrap();

    assert!(snapshot_repo
        .get(&task.id, "a%b/child.txt")
        .unwrap()
        .is_none());
    assert!(baseline_repo
        .get(&task.id, "a%b/child.txt")
        .unwrap()
        .is_none());
    assert!(pending_repo
        .get(&task.id, "a%b/child.txt")
        .unwrap()
        .is_none());
    assert!(snapshot_repo
        .get(&task.id, "aXb/child.txt")
        .unwrap()
        .is_some());
    assert!(baseline_repo
        .get(&task.id, "aXb/child.txt")
        .unwrap()
        .is_some());
    assert!(pending_repo
        .get(&task.id, "aXb/child.txt")
        .unwrap()
        .is_some());

    snapshot_repo.remove_tree(&task.id, "a_b").unwrap();
    baseline_repo.remove_tree(&task.id, "a_b").unwrap();
    pending_repo.remove_tree(&task.id, "a_b").unwrap();
    assert!(snapshot_repo
        .get(&task.id, "a_b/child.txt")
        .unwrap()
        .is_none());
    assert!(snapshot_repo
        .get(&task.id, "aXb/child.txt")
        .unwrap()
        .is_some());
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
        last_transfer_activity_unix_ms: 0,
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
fn test_deferred_transfer_operations() {
    let conn = setup_db();
    let task_repo = SyncTaskRepository::new(&conn);
    let deferred_repo = DeferredTransferRepository::new(&conn);

    let task = SyncTask {
        id: Uuid::new_v4(),
        name: "Deferred Test".to_string(),
        primary_device_id: "a".to_string(),
        secondary_device_id: "b".to_string(),
        local_path: "/tmp/a".to_string(),
        remote_path: "/tmp/b".to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
        last_transfer_activity_unix_ms: 0,
    };
    task_repo.insert(&task).unwrap();

    deferred_repo
        .upsert(&DeferredTransferRecord {
            task_id: task.id,
            relative_path: "large.zip".to_string(),
            direction: "upload".to_string(),
            reason: "cancelled by user".to_string(),
            created_unix_ms: 10,
        })
        .unwrap();
    deferred_repo
        .upsert(&DeferredTransferRecord {
            task_id: task.id,
            relative_path: "large.zip".to_string(),
            direction: "download".to_string(),
            reason: "cancelled by user".to_string(),
            created_unix_ms: 20,
        })
        .unwrap();

    assert!(deferred_repo
        .exists(&task.id, "large.zip", Some("upload"))
        .unwrap());
    assert!(deferred_repo.exists(&task.id, "large.zip", None).unwrap());
    assert_eq!(deferred_repo.list_all().unwrap().len(), 2);

    deferred_repo
        .remove(&task.id, "large.zip", Some("upload"))
        .unwrap();
    assert!(!deferred_repo
        .exists(&task.id, "large.zip", Some("upload"))
        .unwrap());
    assert!(deferred_repo
        .exists(&task.id, "large.zip", Some("download"))
        .unwrap());

    deferred_repo.remove(&task.id, "large.zip", None).unwrap();
    assert!(deferred_repo.list_all().unwrap().is_empty());
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
