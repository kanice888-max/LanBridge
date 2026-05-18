use lanbridge::app_state::AppState;
use lanbridge::core::model::{
    ChangeKind, DeviceRole, EntryKind, FileSnapshot, HashStatus, LogLevel, PairedDevice,
    SyncBaseline, SyncDecision, SyncTask,
};
use lanbridge::core::{planner, scanner};
use lanbridge::pairing::{derive_pairing_code, generate_nonce, DeviceIdentity, PublicIdentity};
use lanbridge::platform::windows::WinPlatform;
use lanbridge::state::{
    db,
    repository::{
        FileSnapshotRepository, LogRepository, PairedDeviceRepository, PendingReturnRepository,
        SyncBaselineRepository, SyncTaskRepository,
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
                last_address: Some(peer.address()),
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

struct CommandTestNode {
    _root: TempDir,
    sync_dir: PathBuf,
    state: AppState,
}

impl CommandTestNode {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let sync_dir = root.path().join("sync");
        let app_dir = root.path().join("app-data");
        std::fs::create_dir_all(&sync_dir).unwrap();
        std::fs::create_dir_all(&app_dir).unwrap();

        let identity = DeviceIdentity::generate();
        let public = identity.public();
        let server = SyncServer::start_in_background(0).unwrap();
        server.set_local_identity(public);
        let state = AppState::new(
            identity,
            Box::new(WinPlatform::with_data_dir(app_dir)),
            DiscoveryState::new(),
            Some(server),
        )
        .unwrap();

        Self {
            _root: root,
            sync_dir,
            state,
        }
    }

    fn public(&self) -> PublicIdentity {
        self.state.identity.public()
    }

    fn address(&self) -> String {
        format!("127.0.0.1:{}", self.state._server.as_ref().unwrap().port())
    }

    fn trust_and_connect(&self, peer: &CommandTestNode) {
        let peer_public = peer.public();
        self.state.connections.pin_peer(peer_public.clone());
        self.state
            ._server
            .as_ref()
            .unwrap()
            .register_trusted_peer(peer_public.clone());
        self.state.connections.register_connection(PeerConnection {
            device_id: peer_public.device_id,
            address: peer.address(),
            connected: true,
            last_seen_unix_ms: 1,
        });
    }

    fn insert_task(&self, task: SyncTask) {
        let db = self.state.db.lock().unwrap();
        SyncTaskRepository::new(&db).insert(&task).unwrap();
        if task.enabled {
            self.state
                ._server
                .as_ref()
                .unwrap()
                .register_task_root(task.id.to_string(), &task.local_path)
                .unwrap();
        }
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
    )
    .unwrap();
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
    let remote_file = remote_scan
        .iter()
        .find(|file| file.relative_path == "docs/report.txt")
        .expect("remote scan should include transferred file");
    assert_eq!(remote_file.blake3_hash, local_snapshots[0].blake3_hash);

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
    let history_trash = secondary.sync_dir.join(".lanbridge-history").join("trash");
    assert!(walkdir::WalkDir::new(&history_trash)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .any(|entry| entry.path().ends_with("docs/report.txt")));
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

#[tokio::test]
async fn test_secondary_sync_now_returns_new_file_to_primary() {
    let primary = CommandTestNode::new();
    let secondary = CommandTestNode::new();
    primary.trust_and_connect(&secondary);
    secondary.trust_and_connect(&primary);

    let task_id = Uuid::new_v4();
    primary.insert_task(SyncTask {
        id: task_id,
        name: "Secondary Return".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: primary.sync_dir.to_string_lossy().to_string(),
        remote_path: secondary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });
    secondary.insert_task(SyncTask {
        id: task_id,
        name: "Secondary Return".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: secondary.sync_dir.to_string_lossy().to_string(),
        remote_path: primary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Secondary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });

    std::fs::create_dir_all(secondary.sync_dir.join("from-secondary")).unwrap();
    std::fs::write(
        secondary.sync_dir.join("from-secondary").join("note.txt"),
        "created on secondary",
    )
    .unwrap();

    let results = lanbridge::commands::run_sync_now(&secondary.state, task_id.to_string())
        .await
        .unwrap();

    assert!(results
        .iter()
        .any(|result| { result.relative_path == "from-secondary/note.txt" && result.success }));
    assert!(!primary
        .sync_dir
        .join("from-secondary")
        .join("note.txt")
        .exists());
    assert_eq!(
        lanbridge::state::repository::PendingReturnRepository::new(
            &secondary.state.db.lock().unwrap()
        )
        .count_by_task(&task_id)
        .unwrap(),
        2
    );

    let return_results = lanbridge::commands::run_execute_return_sync(
        &secondary.state,
        task_id.to_string(),
        vec![
            "from-secondary".to_string(),
            "from-secondary/note.txt".to_string(),
        ],
    )
    .await
    .unwrap();

    assert!(return_results
        .iter()
        .any(|result| result.relative_path == "from-secondary/note.txt" && result.success));
    assert_eq!(
        std::fs::read_to_string(primary.sync_dir.join("from-secondary").join("note.txt")).unwrap(),
        "created on secondary"
    );
    assert_eq!(
        lanbridge::state::repository::PendingReturnRepository::new(
            &secondary.state.db.lock().unwrap()
        )
        .count_by_task(&task_id)
        .unwrap(),
        0
    );
}

#[test]
fn test_refresh_pending_returns_discovers_secondary_new_file_without_network() {
    let primary = CommandTestNode::new();
    let secondary = CommandTestNode::new();
    let task_id = Uuid::new_v4();
    secondary.insert_task(SyncTask {
        id: task_id,
        name: "Refresh Pending".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: secondary.sync_dir.to_string_lossy().to_string(),
        remote_path: primary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Secondary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });

    std::fs::write(
        secondary.sync_dir.join("secondary-note.txt"),
        "created on secondary",
    )
    .unwrap();

    let results =
        lanbridge::commands::run_refresh_pending_returns(&secondary.state, task_id.to_string())
            .unwrap();

    assert!(results.iter().any(|result| {
        result.relative_path == "secondary-note.txt" && result.success && result.error.is_none()
    }));
    let pending = PendingReturnRepository::new(&secondary.state.db.lock().unwrap())
        .list_by_task(&task_id)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].relative_path, "secondary-note.txt");
    assert!(!primary.sync_dir.join("secondary-note.txt").exists());
}

#[tokio::test]
async fn test_secondary_delete_requires_explicit_return_delete() {
    let primary = CommandTestNode::new();
    let secondary = CommandTestNode::new();
    primary.trust_and_connect(&secondary);
    secondary.trust_and_connect(&primary);

    let task_id = Uuid::new_v4();
    primary.insert_task(SyncTask {
        id: task_id,
        name: "Secondary Delete Request".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: primary.sync_dir.to_string_lossy().to_string(),
        remote_path: secondary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });
    secondary.insert_task(SyncTask {
        id: task_id,
        name: "Secondary Delete Request".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: secondary.sync_dir.to_string_lossy().to_string(),
        remote_path: primary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Secondary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });

    std::fs::write(primary.sync_dir.join("shared.txt"), "same").unwrap();
    let hash = blake3::hash(b"same").to_hex().to_string();
    let baseline = SyncBaseline {
        task_id,
        relative_path: "shared.txt".to_string(),
        primary_hash: Some(hash.clone()),
        primary_hash_status: HashStatus::Verified,
        primary_size: 4,
        primary_modified_unix_ms: 1000,
        secondary_hash: Some(hash),
        secondary_hash_status: HashStatus::Verified,
        secondary_modified_unix_ms: 1000,
        last_synced_unix_ms: 1000,
    };
    SyncBaselineRepository::new(&primary.state.db.lock().unwrap())
        .upsert(&baseline)
        .unwrap();
    SyncBaselineRepository::new(&secondary.state.db.lock().unwrap())
        .upsert(&baseline)
        .unwrap();

    let results = lanbridge::commands::run_sync_now(&secondary.state, task_id.to_string())
        .await
        .unwrap();
    assert!(results
        .iter()
        .any(|result| result.relative_path == "shared.txt" && result.success));
    assert!(primary.sync_dir.join("shared.txt").exists());

    let pending = PendingReturnRepository::new(&secondary.state.db.lock().unwrap())
        .list_by_task(&task_id)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].change_kind, ChangeKind::Deleted);

    let delete_results = lanbridge::commands::run_execute_return_sync(
        &secondary.state,
        task_id.to_string(),
        vec!["shared.txt".to_string()],
    )
    .await
    .unwrap();
    assert_eq!(delete_results.len(), 1);
    assert!(delete_results[0].success);
    assert!(!primary.sync_dir.join("shared.txt").exists());
    assert!(primary.sync_dir.join(".lanbridge-history").exists());
    assert_eq!(
        PendingReturnRepository::new(&secondary.state.db.lock().unwrap())
            .count_by_task(&task_id)
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn test_secondary_sync_now_keeps_pending_when_primary_has_unbaselined_file() {
    let primary = CommandTestNode::new();
    let secondary = CommandTestNode::new();
    primary.trust_and_connect(&secondary);
    secondary.trust_and_connect(&primary);

    let task_id = Uuid::new_v4();
    primary.insert_task(SyncTask {
        id: task_id,
        name: "Secondary Return Conflict".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: primary.sync_dir.to_string_lossy().to_string(),
        remote_path: secondary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });
    secondary.insert_task(SyncTask {
        id: task_id,
        name: "Secondary Return Conflict".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: secondary.sync_dir.to_string_lossy().to_string(),
        remote_path: primary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Secondary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });

    std::fs::write(primary.sync_dir.join("same-name.txt"), "primary version").unwrap();
    std::fs::write(
        secondary.sync_dir.join("same-name.txt"),
        "secondary version",
    )
    .unwrap();

    let results = lanbridge::commands::run_sync_now(&secondary.state, task_id.to_string())
        .await
        .unwrap();

    assert!(results.iter().any(|result| {
        result.relative_path == "same-name.txt"
            && !result.success
            && result.error.as_deref() == Some("local file already exists")
    }));
    assert_eq!(
        std::fs::read_to_string(primary.sync_dir.join("same-name.txt")).unwrap(),
        "primary version"
    );
    assert_eq!(
        lanbridge::state::repository::PendingReturnRepository::new(
            &secondary.state.db.lock().unwrap()
        )
        .count_by_task(&task_id)
        .unwrap(),
        1
    );

    let return_results = lanbridge::commands::run_execute_return_sync(
        &secondary.state,
        task_id.to_string(),
        vec!["same-name.txt".to_string()],
    )
    .await
    .unwrap();
    let conflict = return_results
        .iter()
        .find(|result| {
            result.relative_path == "same-name.txt"
                && !result.success
                && result.error.as_deref() == Some("primary file already exists")
        })
        .expect("explicit return should be blocked by primary-side conflict");
    assert_eq!(
        conflict.error.as_deref(),
        Some("primary file already exists")
    );
    assert_eq!(
        lanbridge::state::repository::PendingReturnRepository::new(
            &secondary.state.db.lock().unwrap()
        )
        .count_by_task(&task_id)
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn test_secondary_sync_now_reports_remote_scan_failure_and_keeps_pending() {
    let primary = CommandTestNode::new();
    let secondary = CommandTestNode::new();
    secondary.state.connections.pin_peer(primary.public());
    secondary
        .state
        ._server
        .as_ref()
        .unwrap()
        .register_trusted_peer(primary.public());

    let task_id = Uuid::new_v4();
    secondary.insert_task(SyncTask {
        id: task_id,
        name: "Offline Primary Return".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: secondary.sync_dir.to_string_lossy().to_string(),
        remote_path: primary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Secondary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });

    std::fs::write(secondary.sync_dir.join("offline.txt"), "pending only").unwrap();

    let results = lanbridge::commands::run_sync_now(&secondary.state, task_id.to_string())
        .await
        .unwrap();

    let result = results
        .iter()
        .find(|result| result.relative_path == "offline.txt")
        .expect("changed secondary file should be reported");
    assert!(!result.success);
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("remote scan failed"));
    assert!(!primary.sync_dir.join("offline.txt").exists());
    assert_eq!(
        lanbridge::state::repository::PendingReturnRepository::new(
            &secondary.state.db.lock().unwrap()
        )
        .count_by_task(&task_id)
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn test_primary_recreates_same_content_after_delete_syncs_again() {
    let primary = CommandTestNode::new();
    let secondary = CommandTestNode::new();
    primary.trust_and_connect(&secondary);
    secondary.trust_and_connect(&primary);

    let task_id = Uuid::new_v4();
    primary.insert_task(SyncTask {
        id: task_id,
        name: "Recreate Same Name".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: primary.sync_dir.to_string_lossy().to_string(),
        remote_path: secondary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });
    secondary.insert_task(SyncTask {
        id: task_id,
        name: "Recreate Same Name".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: secondary.sync_dir.to_string_lossy().to_string(),
        remote_path: primary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Secondary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });

    let primary_file = primary.sync_dir.join("again.txt");
    let secondary_file = secondary.sync_dir.join("again.txt");
    std::fs::write(&primary_file, "same content").unwrap();
    assert!(
        lanbridge::commands::run_sync_now(&primary.state, task_id.to_string())
            .await
            .unwrap()
            .iter()
            .any(|result| result.relative_path == "again.txt" && result.success)
    );
    assert_eq!(
        std::fs::read_to_string(&secondary_file).unwrap(),
        "same content"
    );

    std::fs::remove_file(&primary_file).unwrap();
    assert!(
        lanbridge::commands::run_sync_now(&primary.state, task_id.to_string())
            .await
            .unwrap()
            .iter()
            .any(|result| result.relative_path == "again.txt" && result.success)
    );
    assert!(!secondary_file.exists());

    std::fs::write(&primary_file, "same content").unwrap();
    let results = lanbridge::commands::run_sync_now(&primary.state, task_id.to_string())
        .await
        .unwrap();

    assert!(results
        .iter()
        .any(|result| result.relative_path == "again.txt" && result.success));
    assert_eq!(
        std::fs::read_to_string(&secondary_file).unwrap(),
        "same content"
    );
}

#[tokio::test]
async fn test_primary_sync_now_creates_empty_directory_on_secondary() {
    let primary = CommandTestNode::new();
    let secondary = CommandTestNode::new();
    primary.trust_and_connect(&secondary);
    secondary.trust_and_connect(&primary);

    let task_id = Uuid::new_v4();
    primary.insert_task(SyncTask {
        id: task_id,
        name: "Empty Directory".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: primary.sync_dir.to_string_lossy().to_string(),
        remote_path: secondary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Primary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });
    secondary.insert_task(SyncTask {
        id: task_id,
        name: "Empty Directory".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: secondary.sync_dir.to_string_lossy().to_string(),
        remote_path: primary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Secondary,
        enabled: true,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });

    std::fs::create_dir_all(primary.sync_dir.join("empty-dir")).unwrap();

    let results = lanbridge::commands::run_sync_now(&primary.state, task_id.to_string())
        .await
        .unwrap();

    assert!(results
        .iter()
        .any(|result| result.relative_path == "empty-dir" && result.success));
    assert!(secondary.sync_dir.join("empty-dir").is_dir());
}

#[tokio::test]
async fn test_sync_now_rejects_paused_task() {
    let primary = CommandTestNode::new();
    let secondary = CommandTestNode::new();
    primary.trust_and_connect(&secondary);
    secondary.trust_and_connect(&primary);

    let task_id = Uuid::new_v4();
    primary.insert_task(SyncTask {
        id: task_id,
        name: "Paused Task".to_string(),
        primary_device_id: primary.public().device_id,
        secondary_device_id: secondary.public().device_id,
        local_path: primary.sync_dir.to_string_lossy().to_string(),
        remote_path: secondary.sync_dir.to_string_lossy().to_string(),
        local_role: DeviceRole::Primary,
        enabled: false,
        created_unix_ms: 1,
        updated_unix_ms: 1,
    });
    std::fs::write(primary.sync_dir.join("paused.txt"), "do not sync").unwrap();

    let error = lanbridge::commands::run_sync_now(&primary.state, task_id.to_string())
        .await
        .expect_err("paused task should not sync");

    assert!(error.contains("paused"));
    assert!(!secondary.sync_dir.join("paused.txt").exists());
}

fn scan_task_files(task_id: Uuid, root: &Path) -> Vec<FileSnapshot> {
    let platform = WinPlatform::with_data_dir(root.join("app-data"));
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
