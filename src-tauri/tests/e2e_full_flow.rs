use lanbridge::core::model::{
    DeviceRole, EntryKind, FileSnapshot, HashStatus, LogLevel, PairedDevice, SyncBaseline,
    SyncDecision, SyncTask,
};
use lanbridge::core::{planner, scanner};
use lanbridge::pairing::{
    derive_pairing_code, generate_nonce, DeviceIdentity, PublicIdentity,
};
use lanbridge::platform::macos::MacPlatform;
use lanbridge::state::{
    db,
    repository::{
        FileSnapshotRepository, LogRepository, PairedDeviceRepository, SyncBaselineRepository,
        SyncTaskRepository,
    },
};
use lanbridge::transport::connection::{pin_connected_peer, PeerConnection};
use lanbridge::transport::discovery::{Announce, DiscoveryState};
use lanbridge::transport::server::SyncServer;
use lanbridge::transport::{ConnectionManager, SyncMessage};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use uuid::Uuid;

struct TestNode {
    _root: TempDir,
    sync_dir: PathBuf,
    conn: Connection,
    identity: DeviceIdentity,
    public: PublicIdentity,
    server: SyncServer,
    manager: ConnectionManager,
}

impl TestNode {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let sync_dir = root.path().join("sync");
        let app_dir = root.path().join("app-data");
        let db_path = app_dir.join("state.sqlite");
        std::fs::create_dir_all(&sync_dir).unwrap();
        std::fs::create_dir_all(&app_dir).unwrap();

        let conn = db::open_db(&db_path).unwrap();
        db::migrate(&conn).unwrap();
        let identity = DeviceIdentity::generate();
        let public = identity.public();
        let server = SyncServer::start_in_background(0).unwrap();
        server.set_local_identity(public.clone());
        server.set_state_db_path(&db_path).unwrap();
        server
            .set_task_roots_persistence_path(app_dir.join("remote_task_roots.json"))
            .unwrap();
        server
            .set_task_invites_persistence_path(app_dir.join("pending_task_invites.json"))
            .unwrap();
        server
            .set_task_invite_inbox_root(app_dir.join("incoming_tasks"))
            .unwrap();
        server.set_auto_accept_task_invites(false);

        Self {
            _root: root,
            sync_dir,
            conn,
            identity,
            public,
            server,
            manager: ConnectionManager::new(),
        }
    }

    fn address(&self) -> String {
        format!("127.0.0.1:{}", self.server.port())
    }

    fn trust(&self, peer: &TestNode) {
        self.server.register_trusted_peer(peer.public.clone());
        self.manager.pin_peer(peer.public.clone());
        PairedDeviceRepository::new(&self.conn)
            .upsert(&PairedDevice {
                device_id: peer.public.device_id.clone(),
                display_name: peer.public.device_id.clone(),
                public_key: peer.public.public_key.clone(),
                last_seen_unix_ms: 1,
                trusted: true,
            })
            .unwrap();
    }

    fn connect_to(&self, peer: &TestNode) {
        self.manager.register_connection(PeerConnection {
            device_id: peer.public.device_id.clone(),
            address: peer.address(),
            connected: true,
            last_seen_unix_ms: 1,
        });
    }

    fn insert_task(&self, task: &SyncTask) {
        SyncTaskRepository::new(&self.conn).insert(task).unwrap();
        self.server
            .register_task_root(task.id.to_string(), &task.local_path)
            .unwrap();
    }
}

#[tokio::test]
async fn test_two_node_discovery_pair_invite_plan_transfer_retry_delete_and_db_status() {
    let primary = TestNode::new();
    let secondary = TestNode::new();
    primary.trust(&secondary);
    secondary.trust(&primary);

    let discovery = DiscoveryState::new();
    discovery.record_peer(
        Announce {
            device_id: secondary.public.device_id.clone(),
            display_name: "Secondary".to_string(),
            public_key: secondary.public.public_key.clone(),
            port: secondary.server.port(),
        },
        "127.0.0.1".to_string(),
        Some("loopback".to_string()),
    );
    let discovered = discovery
        .list_devices()
        .into_iter()
        .find(|device| device.device_id == secondary.public.device_id)
        .expect("secondary should be discovered");
    let connected_id = pin_connected_peer(
        &primary.manager,
        &discovered.ip,
        discovered.port,
        Some(PublicIdentity {
            device_id: discovered.device_id.clone(),
            public_key: discovered.public_key.clone(),
        }),
    );
    assert_eq!(connected_id, secondary.public.device_id);
    assert!(primary.manager.is_connected(&secondary.public.device_id));

    let nonce = generate_nonce();
    let primary_code = derive_pairing_code(
        &primary.public.public_key,
        &secondary.public.public_key,
        &nonce,
    );
    let secondary_code = derive_pairing_code(
        &secondary.public.public_key,
        &primary.public.public_key,
        &nonce,
    );
    assert_eq!(primary_code, secondary_code);
    assert!(primary.manager.is_pinned(&secondary.public.device_id));
    assert!(secondary.manager.is_pinned(&primary.public.device_id));

    let task_id = Uuid::new_v4();
    let invite_id = Uuid::new_v4().to_string();
    let proposal = SyncMessage::TaskInviteProposal {
        invite_id: invite_id.clone(),
        task_id: task_id.to_string(),
        task_name: "Two Node E2E".to_string(),
        requester_device_id: primary.public.device_id.clone(),
        requester_public_key: primary.public.public_key.clone(),
        requester_port: primary.server.port(),
        requester_path: Some(primary.sync_dir.to_string_lossy().to_string()),
        proposed_role: "Secondary".to_string(),
    };
    let invite_response = lanbridge::transport::connection::send_message_to_peer(
        &primary.manager,
        &secondary.public.device_id,
        proposal,
    )
    .await
    .unwrap();
    match invite_response {
        SyncMessage::TaskInvitePending { invite_id: id, .. } => assert_eq!(id, invite_id),
        other => panic!("expected pending invite, got {:?}", other),
    }

    let listed_invite = secondary
        .server
        .list_task_invites()
        .into_iter()
        .find(|invite| invite.invite_id == invite_id)
        .expect("secondary should list incoming invite");
    assert_eq!(
        listed_invite.requester_address.as_deref(),
        Some(primary.address().as_str())
    );

    let accepted = secondary
        .server
        .accept_task_invite(&invite_id, &secondary.sync_dir)
        .unwrap();
    secondary.connect_to(&primary);
    assert!(secondary.manager.is_connected(&primary.public.device_id));

    let primary_task = SyncTask {
        id: task_id,
        name: "Two Node E2E".to_string(),
        primary_device_id: primary.public.device_id.clone(),
        secondary_device_id: secondary.public.device_id.clone(),
        local_path: primary.sync_dir.to_string_lossy().to_string(),
        remote_path: secondary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    };
    let secondary_task = SyncTask {
        id: task_id,
        name: accepted.task_name,
        primary_device_id: primary.public.device_id.clone(),
        secondary_device_id: secondary.public.device_id.clone(),
        local_path: secondary.sync_dir.to_string_lossy().to_string(),
        remote_path: primary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Secondary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    };
    primary.insert_task(&primary_task);
    secondary.insert_task(&secondary_task);

    let status = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &primary.manager,
        &primary.identity,
        &secondary.public.device_id,
        SyncMessage::TaskInviteStatusRequest {
            invite_id: invite_id.clone(),
        },
    )
    .await
    .unwrap();
    match status {
        SyncMessage::TaskInviteStatus {
            status,
            remote_path: Some(path),
            ..
        } => {
            assert_eq!(status, "Accepted");
            assert_eq!(path, secondary.sync_dir.to_string_lossy());
        }
        other => panic!("expected accepted invite status, got {:?}", other),
    }

    std::fs::create_dir_all(primary.sync_dir.join("docs")).unwrap();
    let source_path = primary.sync_dir.join("docs").join("report.txt");
    std::fs::write(&source_path, "first real e2e payload").unwrap();

    let local_snapshots = scan_task_files(task_id, &primary.sync_dir);
    let local_baselines = SyncBaselineRepository::new(&primary.conn)
        .list_by_task(&task_id)
        .unwrap();
    let actions = planner::plan_sync(&local_snapshots, &local_baselines, DeviceRole::Primary);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].decision, SyncDecision::ApplyToSecondary);

    primary.manager.register_connection(PeerConnection {
        device_id: secondary.public.device_id.clone(),
        address: "127.0.0.1:1".to_string(),
        connected: true,
        last_seen_unix_ms: 1,
    });
    let failed_once = lanbridge::transport::connection::send_authenticated_file_to_peer(
        &primary.manager,
        &primary.identity,
        &secondary.public.device_id,
        task_id.to_string(),
        "docs/report.txt",
        &source_path,
    )
    .await;
    assert!(
        failed_once.is_err(),
        "stale connection should exercise the retry path"
    );

    primary.connect_to(&secondary);
    lanbridge::transport::connection::send_authenticated_file_to_peer(
        &primary.manager,
        &primary.identity,
        &secondary.public.device_id,
        task_id.to_string(),
        "docs/report.txt",
        &source_path,
    )
    .await
    .unwrap();

    let received_path = secondary.sync_dir.join("docs").join("report.txt");
    assert_eq!(
        std::fs::read_to_string(&received_path).unwrap(),
        "first real e2e payload"
    );

    let remote_scan = lanbridge::transport::connection::request_authenticated_scan(
        &primary.manager,
        &primary.identity,
        &secondary.public.device_id,
        task_id.to_string(),
    )
    .await
    .unwrap();
    assert_eq!(remote_scan.len(), 1);
    assert_eq!(remote_scan[0].relative_path, "docs/report.txt");
    assert_eq!(remote_scan[0].blake3_hash, local_snapshots[0].blake3_hash);

    let receiver_snapshot = FileSnapshotRepository::new(&secondary.conn)
        .get(&task_id, "docs/report.txt")
        .unwrap()
        .expect("receiver snapshot should be updated after ACK");
    let receiver_baseline = SyncBaselineRepository::new(&secondary.conn)
        .get(&task_id, "docs/report.txt")
        .unwrap()
        .expect("receiver baseline should be updated after ACK");
    assert_eq!(
        receiver_snapshot.blake3_hash,
        local_snapshots[0].blake3_hash
    );
    assert_eq!(
        receiver_baseline.primary_hash,
        local_snapshots[0].blake3_hash
    );
    assert_eq!(
        receiver_baseline.secondary_hash,
        local_snapshots[0].blake3_hash
    );
    assert!(LogRepository::new(&secondary.conn)
        .list_recent(20)
        .unwrap()
        .iter()
        .any(|entry| {
            entry.level == LogLevel::Info
                && entry.relative_path.as_deref() == Some("docs/report.txt")
                && entry.message.contains("received file")
        }));

    persist_primary_success(&primary.conn, &local_snapshots[0]);
    std::fs::remove_file(&source_path).unwrap();
    let deletion_actions = planner::plan_sync(
        &scan_task_files(task_id, &primary.sync_dir),
        &SyncBaselineRepository::new(&primary.conn)
            .list_by_task(&task_id)
            .unwrap(),
        DeviceRole::Primary,
    );
    assert_eq!(deletion_actions.len(), 1);
    assert_eq!(
        deletion_actions[0].decision,
        SyncDecision::MoveSecondaryToHistory
    );

    let delete_ack = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &primary.manager,
        &primary.identity,
        &secondary.public.device_id,
        SyncMessage::FileDelete {
            task_id: task_id.to_string(),
            relative_path: "docs/report.txt".to_string(),
        },
    )
    .await
    .unwrap();
    match delete_ack {
        SyncMessage::FileAck { success: true, .. } => {}
        other => panic!("expected delete ack, got {:?}", other),
    }
    assert!(!received_path.exists());
    assert!(secondary
        .sync_dir
        .join(".lanbridge-history")
        .join("trash")
        .join("docs")
        .join("report.txt")
        .exists());
    assert!(
        FileSnapshotRepository::new(&secondary.conn)
            .get(&task_id, "docs/report.txt")
            .unwrap()
            .expect("receiver snapshot should remain addressable")
            .deleted
    );
    assert!(LogRepository::new(&secondary.conn)
        .list_recent(20)
        .unwrap()
        .iter()
        .any(|entry| {
            entry.level == LogLevel::Info
                && entry.relative_path.as_deref() == Some("docs/report.txt")
                && entry.message.contains("received delete")
        }));
}

fn scan_task_files(task_id: Uuid, root: &Path) -> Vec<FileSnapshot> {
    let platform = MacPlatform::with_data_dir(root.join("app-data"));
    scanner::scan_root(root, &platform)
        .unwrap()
        .into_iter()
        .map(|result| result.snapshot)
        .filter(|snapshot| snapshot.kind == EntryKind::File)
        .map(|mut snapshot| {
            snapshot.task_id = task_id;
            snapshot
        })
        .collect()
}

fn persist_primary_success(conn: &Connection, snapshot: &FileSnapshot) {
    FileSnapshotRepository::new(conn).upsert(snapshot).unwrap();
    SyncBaselineRepository::new(conn)
        .upsert(&SyncBaseline {
            task_id: snapshot.task_id,
            relative_path: snapshot.relative_path.clone(),
            primary_hash: snapshot.blake3_hash.clone(),
            primary_hash_status: snapshot.hash_status,
            primary_size: snapshot.size,
            primary_modified_unix_ms: snapshot.modified_unix_ms,
            secondary_hash: snapshot.blake3_hash.clone(),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: snapshot.modified_unix_ms,
            last_synced_unix_ms: 1,
        })
        .unwrap();
}
