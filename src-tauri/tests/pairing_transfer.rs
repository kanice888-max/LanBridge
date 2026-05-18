use lanbridge::core::model::{
    DeviceRole, EntryKind, HashStatus, SyncBaseline, SyncDecision, SyncTask,
};
use lanbridge::core::{planner, scanner};
use lanbridge::pairing::PublicIdentity;
use lanbridge::pairing::{derive_pairing_code, generate_nonce, DeviceIdentity};
use lanbridge::platform::macos::MacPlatform;
use lanbridge::state::{
    db,
    repository::{FileSnapshotRepository, SyncBaselineRepository, SyncTaskRepository},
};
use lanbridge::transport::connection::pin_connected_peer;
use lanbridge::transport::discovery::{Announce, DiscoveryState};
use lanbridge::transport::server::SyncServer;
use lanbridge::transport::{
    decode_message, encode_message, ConnectionManager, PeerConnection, SyncMessage,
};
use rusqlite::Connection;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

// ===== Identity Tests =====

#[test]
fn test_identity_generate() {
    let identity = DeviceIdentity::generate();
    let pub_id = identity.public();
    assert!(!pub_id.device_id.is_empty());
    assert_eq!(pub_id.public_key.len(), 32);
}

#[test]
fn test_identity_load_or_create() {
    let dir = TempDir::new().unwrap();
    let key_path = dir.path().join("identity.key");

    // First call creates
    let id1 = DeviceIdentity::load_or_create(&key_path).unwrap();
    let pub1 = id1.public();

    // Second call loads the same key
    let id2 = DeviceIdentity::load_or_create(&key_path).unwrap();
    let pub2 = id2.public();

    assert_eq!(pub1.device_id, pub2.device_id);
    assert_eq!(pub1.public_key, pub2.public_key);
}

#[test]
fn test_identity_sign_verify() {
    let identity = DeviceIdentity::generate();
    let message = b"test message";

    let signature = identity.sign(message);
    let pub_key = identity.public().public_key;

    // Verify succeeds
    assert!(DeviceIdentity::verify(&pub_key, message, &signature).is_ok());

    // Verify fails with wrong message
    assert!(DeviceIdentity::verify(&pub_key, b"wrong message", &signature).is_err());
}

// ===== Pairing Code Tests =====

#[test]
fn test_pairing_code_same_both_ways() {
    let id_a = DeviceIdentity::generate();
    let id_b = DeviceIdentity::generate();
    let nonce = generate_nonce();

    let code_a = derive_pairing_code(&id_a.public().public_key, &id_b.public().public_key, &nonce);
    let code_b = derive_pairing_code(&id_b.public().public_key, &id_a.public().public_key, &nonce);

    assert_eq!(code_a, code_b, "both devices should see the same code");
    assert_eq!(code_a.len(), 6);
}

#[test]
fn test_pairing_code_different_for_different_pairs() {
    let id_a = DeviceIdentity::generate();
    let id_b = DeviceIdentity::generate();
    let id_c = DeviceIdentity::generate();
    let nonce = generate_nonce();

    let code_ab = derive_pairing_code(&id_a.public().public_key, &id_b.public().public_key, &nonce);
    let code_ac = derive_pairing_code(&id_a.public().public_key, &id_c.public().public_key, &nonce);

    assert_ne!(
        code_ab, code_ac,
        "different pairs should have different codes"
    );
}

// ===== Protocol Tests =====

#[test]
fn test_protocol_encode_decode() {
    let msg = SyncMessage::PairRequest {
        device_id: "dev-001".to_string(),
        public_key: vec![1, 2, 3],
        nonce: vec![4, 5, 6],
    };

    let encoded = encode_message(&msg).unwrap();
    assert!(encoded.len() > 4, "should have length prefix");

    // Decode the JSON part (skip 4-byte length prefix)
    let decoded = decode_message(&encoded[4..]).unwrap();
    match decoded {
        SyncMessage::PairRequest { device_id, .. } => {
            assert_eq!(device_id, "dev-001");
        }
        _ => panic!("wrong message type"),
    }
}

#[test]
fn test_protocol_file_transfer() {
    let msg = SyncMessage::FileTransfer {
        task_id: "task-123".to_string(),
        relative_path: "docs/readme.txt".to_string(),
        file_hash: "abc123".to_string(),
        total_bytes: 1024,
        data: vec![0; 1024],
    };

    let encoded = encode_message(&msg).unwrap();
    let decoded = decode_message(&encoded[4..]).unwrap();

    match decoded {
        SyncMessage::FileTransfer {
            relative_path,
            total_bytes,
            ..
        } => {
            assert_eq!(relative_path, "docs/readme.txt");
            assert_eq!(total_bytes, 1024);
        }
        _ => panic!("wrong message type"),
    }
}

// ===== Connection Manager Tests =====

#[test]
fn test_connection_manager_pin() {
    let manager = ConnectionManager::new();
    let identity = DeviceIdentity::generate();
    let pub_id = identity.public();

    assert!(!manager.is_pinned(&pub_id.device_id));

    manager.pin_peer(pub_id.clone());
    assert!(manager.is_pinned(&pub_id.device_id));

    let retrieved = manager.get_pinned(&pub_id.device_id).unwrap();
    assert_eq!(retrieved.device_id, pub_id.device_id);
}

#[test]
fn test_connection_manager_connect_disconnect() {
    let manager = ConnectionManager::new();

    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "dev-001".to_string(),
        address: "192.168.1.100".to_string(),
        connected: true,
        last_seen_unix_ms: 0,
    });

    assert!(manager.is_connected("dev-001"));
    assert!(!manager.is_connected("dev-002"));

    manager.disconnect("dev-001");
    assert!(!manager.is_connected("dev-001"));
}

// ===== Discovery Tests =====

#[test]
fn test_discovery_state_keeps_multiple_addresses_for_same_peer() {
    let state = DiscoveryState::new();
    let announce = Announce {
        device_id: "dev-001".to_string(),
        display_name: "Peer".to_string(),
        public_key: vec![7; 32],
        port: 9527,
    };

    state.record_peer(
        announce.clone(),
        "192.168.1.20".to_string(),
        Some("wifi".to_string()),
    );
    state.record_peer(announce, "10.8.0.20".to_string(), Some("vpn".to_string()));

    let devices = state.list_devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device_id, "dev-001");
    assert_eq!(devices[0].ip, "192.168.1.20");
    assert_eq!(devices[0].addresses.len(), 2);
    assert!(devices[0]
        .addresses
        .iter()
        .any(|addr| addr.ip == "10.8.0.20"));
    assert_eq!(devices[0].public_key, vec![7; 32]);
}

#[test]
fn test_discovery_status_reports_startup_error() {
    let state = DiscoveryState::failed("multicast bind failed".to_string());
    let status = state.status();

    assert!(!status.running);
    assert_eq!(status.error.as_deref(), Some("multicast bind failed"));
}

#[test]
fn test_discovery_state_ignores_announces_without_tcp_port() {
    let state = DiscoveryState::new();

    state.record_peer(
        Announce {
            device_id: "dev-no-tcp".to_string(),
            display_name: "No TCP".to_string(),
            public_key: vec![1; 32],
            port: 0,
        },
        "192.168.1.50".to_string(),
        Some("wifi".to_string()),
    );

    assert!(
        state.list_devices().is_empty(),
        "devices without a TCP listening port should not be shown as connectable"
    );
}

#[test]
fn test_discovery_start_rejects_missing_tcp_port() {
    let result = lanbridge::transport::discovery::start_in_background(
        "dev-no-port".to_string(),
        "No Port".to_string(),
        vec![2; 32],
        0,
    );

    let error = match result {
        Ok(_) => panic!("discovery should not advertise port 0"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("TCP port"),
        "error should explain that a TCP port is required"
    );
}

#[test]
fn test_connecting_discovered_peer_pins_real_device_id() {
    let manager = ConnectionManager::new();
    let peer = PublicIdentity {
        device_id: "real-device-id".to_string(),
        public_key: vec![9; 32],
    };

    let device_id = pin_connected_peer(&manager, "192.168.1.20", 9527, Some(peer)).unwrap();

    assert_eq!(device_id, "real-device-id");
    assert!(manager.is_pinned("real-device-id"));
}

#[test]
fn test_pin_connected_peer_rejects_missing_identity() {
    let manager = ConnectionManager::new();

    let result = pin_connected_peer(&manager, "192.168.1.20", 9527, None);

    assert!(result.is_err());
    assert!(manager.list_peers().is_empty());
}

#[tokio::test]
async fn test_sync_server_background_accepts_tcp_connection() {
    let server = SyncServer::start_in_background(0).unwrap();
    let port = server.port();

    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_ping_peer_address_roundtrip() {
    let server = SyncServer::start_in_background(0).unwrap();

    lanbridge::transport::connection::ping_peer_address("127.0.0.1", server.port())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_manual_ip_connection_fetches_real_peer_identity() {
    let server = SyncServer::start_in_background(0).unwrap();
    let peer_identity = DeviceIdentity::generate();
    let peer_public = peer_identity.public();
    server.set_local_identity(peer_public.clone());

    let fetched =
        lanbridge::transport::connection::request_peer_identity("127.0.0.1", server.port())
            .await
            .unwrap();

    assert_eq!(fetched.device_id, peer_public.device_id);
    assert_eq!(fetched.public_key, peer_public.public_key);
}

#[tokio::test]
async fn test_sync_server_rejects_unauthenticated_file_transfer() {
    let server = SyncServer::start_in_background(0).unwrap();
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-network-001";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .unwrap();
    let data = b"hello over tcp".to_vec();
    let msg = SyncMessage::FileTransfer {
        task_id: task_id.to_string(),
        relative_path: "docs/readme.txt".to_string(),
        file_hash: blake3::hash(&data).to_hex().to_string(),
        total_bytes: data.len() as u64,
        data,
    };

    stream
        .write_all(&encode_message(&msg).unwrap())
        .await
        .unwrap();

    let response = read_one_message(&mut stream).await;
    match response {
        SyncMessage::FileAck {
            task_id: ack_task,
            relative_path,
            success,
            error,
        } => {
            assert_eq!(ack_task, task_id);
            assert_eq!(relative_path, "docs/readme.txt");
            assert!(!success, "unauthenticated transfer should be rejected");
            assert!(error.unwrap().contains("authenticated"));
        }
        other => panic!("expected FileAck, got {:?}", other),
    }

    assert!(!remote_dir.path().join("docs").join("readme.txt").exists());
}

#[tokio::test]
async fn test_sync_server_receives_authenticated_file_transfer_and_acks() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-network-auth-001";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "peer-auth-target".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let data = b"hello authenticated tcp".to_vec();
    let response = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &manager,
        &local_identity,
        "peer-auth-target",
        SyncMessage::FileTransfer {
            task_id: task_id.to_string(),
            relative_path: "docs/readme.txt".to_string(),
            file_hash: blake3::hash(&data).to_hex().to_string(),
            total_bytes: data.len() as u64,
            data,
        },
    )
    .await
    .unwrap();

    match response {
        SyncMessage::FileAck { success, error, .. } => {
            assert!(success, "unexpected ack error: {:?}", error);
        }
        other => panic!("expected FileAck, got {:?}", other),
    }

    assert_eq!(
        std::fs::read_to_string(remote_dir.path().join("docs").join("readme.txt")).unwrap(),
        "hello authenticated tcp"
    );
}

#[tokio::test]
async fn test_sync_server_keeps_legacy_v1_chunk_ack_contract() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-legacy-v1-ack";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .unwrap();
    authenticate_test_stream(&mut stream, &local_identity).await;

    let data = b"legacy v1 chunk".to_vec();
    let file_hash = blake3::hash(&data).to_hex().to_string();
    stream
        .write_all(
            &encode_message(&SyncMessage::FileChunkStart {
                task_id: task_id.to_string(),
                relative_path: "legacy.txt".to_string(),
                file_hash,
                total_bytes: data.len() as u64,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_file_ack_success(&mut stream).await;

    stream
        .write_all(
            &encode_message(&SyncMessage::FileChunk {
                task_id: task_id.to_string(),
                relative_path: "legacy.txt".to_string(),
                offset: 0,
                data,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_file_ack_success(&mut stream).await;

    stream
        .write_all(
            &encode_message(&SyncMessage::FileChunkEnd {
                task_id: task_id.to_string(),
                relative_path: "legacy.txt".to_string(),
                file_hash: None,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_file_ack_success(&mut stream).await;

    assert_eq!(
        std::fs::read_to_string(remote_dir.path().join("legacy.txt")).unwrap(),
        "legacy v1 chunk"
    );
}

#[tokio::test]
async fn test_sync_server_v1_streaming_uses_checkpoint_acks() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-v1-checkpoint-ack";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .unwrap();
    authenticate_test_stream(&mut stream, &local_identity).await;

    let chunk = vec![9u8; 1024 * 1024];
    let total_bytes = 17 * chunk.len() as u64;
    let mut hasher = blake3::Hasher::new();
    stream
        .write_all(
            &encode_message(&SyncMessage::FileChunkStart {
                task_id: task_id.to_string(),
                relative_path: "checkpoint.bin".to_string(),
                file_hash: String::new(),
                total_bytes,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_file_ack_success(&mut stream).await;

    for index in 0..17u64 {
        let offset = index * chunk.len() as u64;
        hasher.update(&chunk);
        stream
            .write_all(
                &encode_message(&SyncMessage::FileChunk {
                    task_id: task_id.to_string(),
                    relative_path: "checkpoint.bin".to_string(),
                    offset,
                    data: chunk.clone(),
                })
                .unwrap(),
            )
            .await
            .unwrap();

        if index == 0 {
            let no_ack = tokio::time::timeout(
                std::time::Duration::from_millis(100),
                read_one_message(&mut stream),
            )
            .await;
            assert!(no_ack.is_err(), "first 1MB chunk should not be ACKed");
        } else if index == 15 {
            assert_file_chunk_ack_success(&mut stream, 16 * 1024 * 1024).await;
        } else if index == 16 {
            assert_file_chunk_ack_success(&mut stream, total_bytes).await;
        }
    }

    stream
        .write_all(
            &encode_message(&SyncMessage::FileChunkEnd {
                task_id: task_id.to_string(),
                relative_path: "checkpoint.bin".to_string(),
                file_hash: Some(hasher.finalize().to_hex().to_string()),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_file_ack_success(&mut stream).await;

    let received = std::fs::read(remote_dir.path().join("checkpoint.bin")).unwrap();
    assert_eq!(received.len(), total_bytes as usize);
    assert!(received.iter().all(|byte| *byte == 9));
}

#[tokio::test]
async fn test_sync_server_rejects_windows_reserved_relative_path() {
    let server = SyncServer::start_in_background(0).unwrap();
    let server_identity = DeviceIdentity::generate();
    let server_public = server_identity.public();
    server.set_local_identity(server_public.clone());
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-reserved-path-001";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();

    let manager = ConnectionManager::new();
    manager.pin_peer(server_public.clone());
    manager.register_connection(PeerConnection {
        device_id: server_public.device_id.clone(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 1,
    });

    let source = remote_dir.path().join("source.txt");
    std::fs::write(&source, "reserved").unwrap();
    let err = lanbridge::transport::connection::send_authenticated_file_to_peer(
        &manager,
        &local_identity,
        &server_public.device_id,
        task_id.to_string(),
        "CON",
        &source,
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        err.contains("reserved device name"),
        "unexpected error: {err}"
    );
    assert!(!remote_dir.path().join("CON").exists());
}

#[tokio::test]
async fn test_connection_manager_sends_message_to_registered_peer() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-network-002";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "peer-001".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let data = b"sent by connection manager".to_vec();
    let response = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &manager,
        &local_identity,
        "peer-001",
        SyncMessage::FileTransfer {
            task_id: task_id.to_string(),
            relative_path: "outbox/file.txt".to_string(),
            file_hash: blake3::hash(&data).to_hex().to_string(),
            total_bytes: data.len() as u64,
            data,
        },
    )
    .await
    .unwrap();

    match response {
        SyncMessage::FileAck { success, error, .. } => {
            assert!(success, "unexpected ack error: {:?}", error)
        }
        other => panic!("expected FileAck, got {:?}", other),
    }
    assert_eq!(
        std::fs::read_to_string(remote_dir.path().join("outbox").join("file.txt")).unwrap(),
        "sent by connection manager"
    );
}

#[tokio::test]
async fn test_task_register_message_enables_later_file_transfer() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-network-003";
    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "peer-002".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let register_response = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &manager,
        &local_identity,
        "peer-002",
        SyncMessage::TaskRegister {
            task_id: task_id.to_string(),
            root_path: remote_dir.path().to_string_lossy().to_string(),
        },
    )
    .await
    .unwrap();
    match register_response {
        SyncMessage::TaskAck { success, error, .. } => {
            assert!(success, "unexpected task ack error: {:?}", error);
        }
        other => panic!("expected TaskAck, got {:?}", other),
    }

    let data = b"after task register".to_vec();
    let file_response = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &manager,
        &local_identity,
        "peer-002",
        SyncMessage::FileTransfer {
            task_id: task_id.to_string(),
            relative_path: "later.txt".to_string(),
            file_hash: blake3::hash(&data).to_hex().to_string(),
            total_bytes: data.len() as u64,
            data,
        },
    )
    .await
    .unwrap();

    match file_response {
        SyncMessage::FileAck { success, error, .. } => {
            assert!(success, "unexpected file ack error: {:?}", error);
        }
        other => panic!("expected FileAck, got {:?}", other),
    }
    assert_eq!(
        std::fs::read_to_string(remote_dir.path().join("later.txt")).unwrap(),
        "after task register"
    );
}

#[tokio::test]
async fn test_unregister_task_root_rejects_later_file_transfer() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-network-unregistered";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();
    server.unregister_task_root(task_id).unwrap();

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "peer-unregistered".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let data = b"should not be written".to_vec();
    let response = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &manager,
        &local_identity,
        "peer-unregistered",
        SyncMessage::FileTransfer {
            task_id: task_id.to_string(),
            relative_path: "later.txt".to_string(),
            file_hash: blake3::hash(&data).to_hex().to_string(),
            total_bytes: data.len() as u64,
            data,
        },
    )
    .await
    .unwrap();

    match response {
        SyncMessage::FileAck { success, error, .. } => {
            assert!(!success);
            assert!(error.unwrap().contains("task root not registered"));
        }
        other => panic!("expected FileAck, got {:?}", other),
    }
    assert!(!remote_dir.path().join("later.txt").exists());
}

#[tokio::test]
async fn test_task_invite_allocates_peer_inbox_without_sender_remote_path() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let inbox_dir = TempDir::new().unwrap();
    server.set_task_invite_inbox_root(inbox_dir.path()).unwrap();
    let source_dir = TempDir::new().unwrap();
    let source_path = source_dir.path().join("invite-file.txt");
    std::fs::write(&source_path, "created through invite").unwrap();
    let task_id = "task-invite-001";

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "invite-peer".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let invite_response = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &manager,
        &local_identity,
        "invite-peer",
        SyncMessage::TaskInvite {
            invite_id: "invite-auto-001".to_string(),
            task_id: task_id.to_string(),
            task_name: "Invite Flow".to_string(),
            requester_port: 0,
            requester_path: Some(source_dir.path().to_string_lossy().to_string()),
            proposed_role: "Secondary".to_string(),
        },
    )
    .await
    .unwrap();

    let remote_path = match invite_response {
        SyncMessage::TaskInviteAck {
            success: true,
            remote_path: Some(path),
            error: None,
            ..
        } => path,
        other => panic!("expected successful TaskInviteAck, got {:?}", other),
    };
    assert!(
        remote_path.starts_with(&inbox_dir.path().to_string_lossy().to_string()),
        "peer should choose a root inside its own inbox"
    );

    lanbridge::transport::connection::send_authenticated_file_to_peer(
        &manager,
        &local_identity,
        "invite-peer",
        task_id,
        "invite-file.txt",
        &source_path,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&remote_path).join("invite-file.txt"))
            .unwrap(),
        "created through invite"
    );
}

#[tokio::test]
async fn test_task_invite_can_wait_for_peer_acceptance() {
    let server = SyncServer::start_in_background(0).unwrap();
    server.set_auto_accept_task_invites(false);
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let source_dir = TempDir::new().unwrap();
    let source_path = source_dir.path().join("accepted-file.txt");
    std::fs::write(&source_path, "accepted through invite").unwrap();

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "pending-invite-peer".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let invite_response = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &manager,
        &local_identity,
        "pending-invite-peer",
        SyncMessage::TaskInvite {
            invite_id: "invite-pending-001".to_string(),
            task_id: "task-pending-001".to_string(),
            task_name: "Pending Invite".to_string(),
            requester_port: 0,
            requester_path: Some(source_dir.path().to_string_lossy().to_string()),
            proposed_role: "Secondary".to_string(),
        },
    )
    .await
    .unwrap();

    match invite_response {
        SyncMessage::TaskInvitePending { invite_id, task_id } => {
            assert_eq!(invite_id, "invite-pending-001");
            assert_eq!(task_id, "task-pending-001");
        }
        other => panic!("expected pending invite, got {:?}", other),
    }
    assert_eq!(server.list_task_invites().len(), 1);

    server
        .accept_task_invite("invite-pending-001", remote_dir.path())
        .unwrap();

    let status_response = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &manager,
        &local_identity,
        "pending-invite-peer",
        SyncMessage::TaskInviteStatusRequest {
            invite_id: "invite-pending-001".to_string(),
        },
    )
    .await
    .unwrap();
    match status_response {
        SyncMessage::TaskInviteStatus {
            status,
            remote_path: Some(path),
            error: None,
            ..
        } => {
            assert_eq!(status, "Accepted");
            assert_eq!(path, remote_dir.path().to_string_lossy());
        }
        other => panic!("expected accepted invite status, got {:?}", other),
    }

    lanbridge::transport::connection::send_authenticated_file_to_peer(
        &manager,
        &local_identity,
        "pending-invite-peer",
        "task-pending-001",
        "accepted-file.txt",
        &source_path,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(remote_dir.path().join("accepted-file.txt")).unwrap(),
        "accepted through invite"
    );
}

#[tokio::test]
async fn test_untrusted_task_invite_becomes_trusted_only_after_peer_accepts() {
    let server = SyncServer::start_in_background(0).unwrap();
    server.set_auto_accept_task_invites(false);
    let local_identity = DeviceIdentity::generate();
    let local_public = local_identity.public();
    let remote_dir = TempDir::new().unwrap();
    let source_dir = TempDir::new().unwrap();
    let source_path = source_dir.path().join("first-contact.txt");
    std::fs::write(&source_path, "first contact transfer").unwrap();

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "first-contact-peer".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let invite_response = lanbridge::transport::connection::send_message_to_peer(
        &manager,
        "first-contact-peer",
        SyncMessage::TaskInviteProposal {
            invite_id: "invite-first-contact-001".to_string(),
            task_id: "task-first-contact-001".to_string(),
            task_name: "First Contact".to_string(),
            requester_device_id: local_public.device_id.clone(),
            requester_public_key: local_public.public_key.clone(),
            requester_port: 0,
            requester_path: Some(source_dir.path().to_string_lossy().to_string()),
            proposed_role: "Secondary".to_string(),
        },
    )
    .await
    .unwrap();

    match invite_response {
        SyncMessage::TaskInvitePending { invite_id, task_id } => {
            assert_eq!(invite_id, "invite-first-contact-001");
            assert_eq!(task_id, "task-first-contact-001");
        }
        other => panic!("expected pending invite proposal, got {:?}", other),
    }

    let pending_status = lanbridge::transport::connection::send_message_to_peer(
        &manager,
        "first-contact-peer",
        SyncMessage::TaskInviteStatusRequest {
            invite_id: "invite-first-contact-001".to_string(),
        },
    )
    .await
    .unwrap();
    match pending_status {
        SyncMessage::TaskInviteStatus { status, .. } => assert_eq!(status, "Pending"),
        other => panic!("expected pending invite status, got {:?}", other),
    }

    server
        .accept_task_invite("invite-first-contact-001", remote_dir.path())
        .unwrap();

    lanbridge::transport::connection::send_authenticated_file_to_peer(
        &manager,
        &local_identity,
        "first-contact-peer",
        "task-first-contact-001",
        "first-contact.txt",
        &source_path,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(remote_dir.path().join("first-contact.txt")).unwrap(),
        "first contact transfer"
    );
}

#[tokio::test]
async fn test_pending_task_invite_persists_across_server_restart() {
    let state_dir = TempDir::new().unwrap();
    let invites_path = state_dir.path().join("pending_task_invites.json");
    let local_identity = DeviceIdentity::generate();

    {
        let server = SyncServer::start_in_background(0).unwrap();
        server
            .set_task_invites_persistence_path(&invites_path)
            .unwrap();
        server.set_auto_accept_task_invites(false);
        server.register_trusted_peer(local_identity.public());

        let manager = ConnectionManager::new();
        manager.register_connection(lanbridge::transport::PeerConnection {
            device_id: "pending-persist-peer".to_string(),
            address: format!("127.0.0.1:{}", server.port()),
            connected: true,
            last_seen_unix_ms: 0,
        });

        let response = lanbridge::transport::connection::send_authenticated_message_to_peer(
            &manager,
            &local_identity,
            "pending-persist-peer",
            SyncMessage::TaskInvite {
                invite_id: "invite-persist-001".to_string(),
                task_id: "task-persist-invite-001".to_string(),
                task_name: "Persisted Pending Invite".to_string(),
                requester_port: 0,
                requester_path: Some("C:\\Sender\\Docs".to_string()),
                proposed_role: "Secondary".to_string(),
            },
        )
        .await
        .unwrap();

        match response {
            SyncMessage::TaskInvitePending { invite_id, .. } => {
                assert_eq!(invite_id, "invite-persist-001");
            }
            other => panic!("expected pending invite, got {:?}", other),
        }
        assert_eq!(server.list_task_invites().len(), 1);
    }

    let server = SyncServer::start_in_background(0).unwrap();
    server
        .set_task_invites_persistence_path(&invites_path)
        .unwrap();
    let invites = server.list_task_invites();

    assert_eq!(invites.len(), 1);
    assert_eq!(invites[0].invite_id, "invite-persist-001");
    assert_eq!(invites[0].status, "Pending");
    assert_eq!(invites[0].task_id, "task-persist-invite-001");
}

#[test]
fn test_accept_task_invite_rejects_missing_or_non_empty_folder() {
    let server = SyncServer::start_in_background(0).unwrap();
    server.set_auto_accept_task_invites(false);
    let missing_dir = TempDir::new().unwrap().path().join("missing");
    let non_empty_dir = TempDir::new().unwrap();
    std::fs::write(
        non_empty_dir.path().join("existing.txt"),
        "do not overwrite",
    )
    .unwrap();

    server
        .record_pending_task_invite_for_test(
            "invite-invalid-path-001",
            "task-invalid-path-001",
            "Invalid Path Invite",
            "requester-device",
            Some("C:\\Sender".to_string()),
            "Secondary",
        )
        .unwrap();

    let missing_error = server
        .accept_task_invite("invite-invalid-path-001", &missing_dir)
        .unwrap_err()
        .to_string();
    assert!(missing_error.contains("must exist"));

    let non_empty_error = server
        .accept_task_invite("invite-invalid-path-001", non_empty_dir.path())
        .unwrap_err()
        .to_string();
    assert!(non_empty_error.contains("must be empty"));
}

#[tokio::test]
async fn test_rejected_task_invite_status_can_be_polled() {
    let server = SyncServer::start_in_background(0).unwrap();
    server.set_auto_accept_task_invites(false);
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "reject-poll-peer".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    lanbridge::transport::connection::send_authenticated_message_to_peer(
        &manager,
        &local_identity,
        "reject-poll-peer",
        SyncMessage::TaskInvite {
            invite_id: "invite-reject-001".to_string(),
            task_id: "task-reject-001".to_string(),
            task_name: "Rejected Invite".to_string(),
            requester_port: 0,
            requester_path: None,
            proposed_role: "Secondary".to_string(),
        },
    )
    .await
    .unwrap();

    server
        .reject_task_invite("invite-reject-001", "folder not allowed")
        .unwrap();

    let response = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &manager,
        &local_identity,
        "reject-poll-peer",
        SyncMessage::TaskInviteStatusRequest {
            invite_id: "invite-reject-001".to_string(),
        },
    )
    .await
    .unwrap();

    match response {
        SyncMessage::TaskInviteStatus {
            status,
            error: Some(error),
            ..
        } => {
            assert_eq!(status, "Rejected");
            assert_eq!(error, "folder not allowed");
        }
        other => panic!("expected rejected invite status, got {:?}", other),
    }
}

#[tokio::test]
async fn test_authenticated_chunked_transfer_supports_files_over_message_limit() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let source_dir = TempDir::new().unwrap();
    let task_id = "task-large-file-001";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();

    let source_path = source_dir.path().join("large.bin");
    let data = vec![42u8; 11 * 1024 * 1024];
    std::fs::write(&source_path, &data).unwrap();

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "large-peer".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });
    lanbridge::transport::connection::set_cached_protocol("large-peer", 1);

    lanbridge::transport::connection::send_authenticated_file_to_peer(
        &manager,
        &local_identity,
        "large-peer",
        task_id,
        "large.bin",
        &source_path,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(remote_dir.path().join("large.bin")).unwrap(),
        data
    );
}

#[tokio::test]
async fn test_authenticated_v2_upload_negotiates_and_transfers_file() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let source_dir = TempDir::new().unwrap();
    let task_id = "task-v2-upload-001";
    let peer_id = "v2-upload-peer";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();

    let source_path = source_dir.path().join("v2-upload.bin");
    let data = vec![13u8; 5 * 1024 * 1024];
    std::fs::write(&source_path, &data).unwrap();

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: peer_id.to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });
    lanbridge::transport::connection::clear_cached_protocol(peer_id);

    lanbridge::transport::connection::send_authenticated_file_to_peer(
        &manager,
        &local_identity,
        peer_id,
        task_id,
        "v2-upload.bin",
        &source_path,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(remote_dir.path().join("v2-upload.bin")).unwrap(),
        data
    );
    assert_eq!(
        lanbridge::transport::connection::get_cached_protocol(peer_id),
        Some(2)
    );
}

#[tokio::test]
async fn test_v2_negotiation_failure_falls_back_to_v1() {
    let local_identity = DeviceIdentity::generate();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer_id = "legacy-fallback-peer";
    let task_id = "task-v2-fallback";
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let trusted = local_identity.public();

    tokio::spawn(async move {
        let (mut first_stream, _) = listener.accept().await.unwrap();
        authenticate_legacy_server_stream(&mut first_stream, &trusted).await;
        match read_one_message(&mut first_stream).await {
            SyncMessage::TransferHello { .. } => {}
            other => panic!("expected TransferHello, got {:?}", other),
        }
        drop(first_stream);

        let (mut second_stream, _) = listener.accept().await.unwrap();
        authenticate_legacy_server_stream(&mut second_stream, &trusted).await;
        let mut received = Vec::new();

        let relative_path = match read_one_message(&mut second_stream).await {
            SyncMessage::FileChunkStart {
                task_id: start_task,
                relative_path: start_path,
                file_hash,
                total_bytes,
            } => {
                assert_eq!(start_task, task_id);
                assert_eq!(start_path, "fallback.txt");
                assert_eq!(
                    file_hash,
                    blake3::hash(b"fallback through v1").to_hex().to_string()
                );
                assert_eq!(total_bytes, "fallback through v1".len() as u64);
                write_message(
                    &mut second_stream,
                    SyncMessage::FileAck {
                        task_id: start_task,
                        relative_path: start_path.clone(),
                        success: true,
                        error: None,
                    },
                )
                .await;
                start_path
            }
            other => panic!("expected FileChunkStart, got {:?}", other),
        };

        loop {
            match read_one_message(&mut second_stream).await {
                SyncMessage::FileChunk {
                    task_id: chunk_task,
                    relative_path: chunk_path,
                    offset,
                    data,
                } => {
                    assert_eq!(chunk_task, task_id);
                    assert_eq!(chunk_path, relative_path);
                    assert_eq!(offset, received.len() as u64);
                    received.extend_from_slice(&data);
                    write_message(
                        &mut second_stream,
                        SyncMessage::FileAck {
                            task_id: chunk_task,
                            relative_path: chunk_path,
                            success: true,
                            error: None,
                        },
                    )
                    .await;
                }
                SyncMessage::FileChunkEnd {
                    task_id: end_task,
                    relative_path: end_path,
                    file_hash,
                } => {
                    assert_eq!(end_task, task_id);
                    assert_eq!(end_path, relative_path);
                    assert_eq!(file_hash, None);
                    write_message(
                        &mut second_stream,
                        SyncMessage::FileAck {
                            task_id: end_task,
                            relative_path: end_path,
                            success: true,
                            error: None,
                        },
                    )
                    .await;
                    break;
                }
                other => panic!("unexpected fallback message: {:?}", other),
            }
        }
        received_tx.send(received).unwrap();
    });

    let source_dir = TempDir::new().unwrap();
    let source_path = source_dir.path().join("fallback.txt");
    std::fs::write(&source_path, "fallback through v1").unwrap();
    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: peer_id.to_string(),
        address: format!("127.0.0.1:{port}"),
        connected: true,
        last_seen_unix_ms: 0,
    });
    lanbridge::transport::connection::clear_cached_protocol(peer_id);

    lanbridge::transport::connection::send_authenticated_file_to_peer(
        &manager,
        &local_identity,
        peer_id,
        task_id,
        "fallback.txt",
        &source_path,
    )
    .await
    .unwrap();

    assert_eq!(received_rx.await.unwrap(), b"fallback through v1");
    assert_eq!(
        lanbridge::transport::connection::get_cached_protocol(peer_id),
        None
    );
}

#[tokio::test]
async fn test_authenticated_file_download_request_streams_file_to_client() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-download-001";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();
    std::fs::create_dir_all(remote_dir.path().join("docs")).unwrap();
    std::fs::write(
        remote_dir.path().join("docs").join("pull.txt"),
        "download me",
    )
    .unwrap();

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "download-peer".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let local_dir = TempDir::new().unwrap();
    lanbridge::transport::connection::request_authenticated_file_from_peer(
        &manager,
        &local_identity,
        "download-peer",
        task_id,
        "docs/pull.txt",
        &local_dir.path().join("docs").join("pull.txt"),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(local_dir.path().join("docs").join("pull.txt")).unwrap(),
        "download me"
    );
}

#[tokio::test]
async fn test_authenticated_v2_file_download_uses_checkpoint_acks() {
    let server = SyncServer::start_in_background(0).unwrap();
    let local_identity = DeviceIdentity::generate();
    server.register_trusted_peer(local_identity.public());
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-download-v2-checkpoint";
    server
        .register_task_root(task_id, remote_dir.path())
        .unwrap();

    let source = remote_dir.path().join("large-download.bin");
    let data = vec![7u8; 17 * 1024 * 1024];
    std::fs::write(&source, &data).unwrap();

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "download-v2-checkpoint-peer".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let local_dir = TempDir::new().unwrap();
    let target = local_dir.path().join("large-download.bin");
    lanbridge::transport::connection::request_authenticated_file_from_peer(
        &manager,
        &local_identity,
        "download-v2-checkpoint-peer",
        task_id,
        "large-download.bin",
        &target,
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(target).unwrap(), data);
    assert_eq!(
        lanbridge::transport::connection::get_cached_protocol("download-v2-checkpoint-peer"),
        Some(2)
    );
}

#[tokio::test]
async fn test_task_invite_proposal_records_requester_service_address() {
    let server = SyncServer::start_in_background(0).unwrap();
    let requester = DeviceIdentity::generate().public();

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .unwrap();
    stream
        .write_all(
            &encode_message(&SyncMessage::TaskInviteProposal {
                invite_id: "invite-address-001".to_string(),
                task_id: "task-address-001".to_string(),
                task_name: "Address Test".to_string(),
                requester_device_id: requester.device_id.clone(),
                requester_public_key: requester.public_key,
                requester_port: 45678,
                requester_path: Some("/remote/path".to_string()),
                proposed_role: "Secondary".to_string(),
            })
            .unwrap(),
        )
        .await
        .unwrap();

    match read_one_message(&mut stream).await {
        SyncMessage::TaskInvitePending { .. } => {}
        other => panic!("expected TaskInvitePending, got {:?}", other),
    }

    let invites = server.list_task_invites();
    assert_eq!(invites.len(), 1);
    assert_eq!(
        invites[0].requester_address.as_deref(),
        Some("127.0.0.1:45678")
    );
}

#[tokio::test]
async fn test_server_updates_db_after_authenticated_chunked_receive() {
    let state_dir = TempDir::new().unwrap();
    let db_path = state_dir.path().join("state.sqlite");
    let conn = db::open_db(&db_path).unwrap();
    db::migrate(&conn).unwrap();

    let server = SyncServer::start_in_background(0).unwrap();
    server.set_state_db_path(&db_path).unwrap();
    let local_identity = DeviceIdentity::generate();
    let local_public = local_identity.public();
    let remote_identity = DeviceIdentity::generate();
    let remote_public = remote_identity.public();
    server.set_local_identity(remote_public.clone());
    server.register_trusted_peer(local_public.clone());

    let task_id = Uuid::new_v4();
    let remote_dir = TempDir::new().unwrap();
    SyncTaskRepository::new(&conn)
        .insert(&SyncTask {
            id: task_id,
            name: "Receiver DB".to_string(),
            primary_device_id: local_public.device_id.clone(),
            secondary_device_id: remote_public.device_id.clone(),
            local_path: remote_dir.path().to_string_lossy().to_string(),
            remote_path: String::new(),
            local_role: DeviceRole::Secondary,
            enabled: true,
            created_unix_ms: 1,
            updated_unix_ms: 1,
        })
        .unwrap();
    server
        .register_task_root(task_id.to_string(), remote_dir.path())
        .unwrap();

    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "receiver-db-peer".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let source_dir = TempDir::new().unwrap();
    let source = source_dir.path().join("state.txt");
    std::fs::write(&source, "receiver database state").unwrap();
    lanbridge::transport::connection::send_authenticated_file_to_peer(
        &manager,
        &local_identity,
        "receiver-db-peer",
        task_id.to_string(),
        "state.txt",
        &source,
    )
    .await
    .unwrap();

    let snapshot = FileSnapshotRepository::new(&conn)
        .get(&task_id, "state.txt")
        .unwrap()
        .expect("receiver snapshot should be updated");
    let baseline = SyncBaselineRepository::new(&conn)
        .get(&task_id, "state.txt")
        .unwrap()
        .expect("receiver baseline should be updated");

    assert_eq!(
        snapshot.blake3_hash,
        Some(
            blake3::hash(b"receiver database state")
                .to_hex()
                .to_string()
        )
    );
    assert_eq!(baseline.primary_hash, snapshot.blake3_hash);
    assert_eq!(baseline.secondary_hash, snapshot.blake3_hash);
}

#[tokio::test]
async fn test_task_register_persists_across_server_restart() {
    let state_dir = TempDir::new().unwrap();
    let roots_path = state_dir.path().join("remote_task_roots.json");
    let local_identity = DeviceIdentity::generate();
    let remote_dir = TempDir::new().unwrap();
    let task_id = "task-persisted-root-001";

    {
        let server = SyncServer::start_in_background(0).unwrap();
        server.set_task_roots_persistence_path(&roots_path).unwrap();
        server.register_trusted_peer(local_identity.public());
        let manager = ConnectionManager::new();
        manager.register_connection(lanbridge::transport::PeerConnection {
            device_id: "persist-peer".to_string(),
            address: format!("127.0.0.1:{}", server.port()),
            connected: true,
            last_seen_unix_ms: 0,
        });

        lanbridge::transport::connection::send_authenticated_message_to_peer(
            &manager,
            &local_identity,
            "persist-peer",
            SyncMessage::TaskRegister {
                task_id: task_id.to_string(),
                root_path: remote_dir.path().to_string_lossy().to_string(),
            },
        )
        .await
        .unwrap();
    }

    let server = SyncServer::start_in_background(0).unwrap();
    server.set_task_roots_persistence_path(&roots_path).unwrap();
    server.register_trusted_peer(local_identity.public());
    let manager = ConnectionManager::new();
    manager.register_connection(lanbridge::transport::PeerConnection {
        device_id: "persist-peer-after-restart".to_string(),
        address: format!("127.0.0.1:{}", server.port()),
        connected: true,
        last_seen_unix_ms: 0,
    });

    let source_dir = TempDir::new().unwrap();
    let source_path = source_dir.path().join("after-restart.txt");
    std::fs::write(&source_path, "still registered").unwrap();
    lanbridge::transport::connection::send_authenticated_file_to_peer(
        &manager,
        &local_identity,
        "persist-peer-after-restart",
        task_id,
        "after-restart.txt",
        &source_path,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(remote_dir.path().join("after-restart.txt")).unwrap(),
        "still registered"
    );
}

#[tokio::test]
async fn test_full_discovery_pair_task_plan_transfer_ack_and_status_flow() {
    let primary_identity = DeviceIdentity::generate();
    let secondary_identity = DeviceIdentity::generate();
    let primary_public = primary_identity.public();
    let secondary_public = secondary_identity.public();

    let primary_server = SyncServer::start_in_background(0).unwrap();
    let secondary_server = SyncServer::start_in_background(0).unwrap();
    primary_server.set_local_identity(primary_public.clone());
    secondary_server.set_local_identity(secondary_public.clone());
    primary_server.register_trusted_peer(secondary_public.clone());
    secondary_server.register_trusted_peer(primary_public.clone());

    let discovery = DiscoveryState::new();
    discovery.record_peer(
        Announce {
            device_id: secondary_public.device_id.clone(),
            display_name: "Secondary".to_string(),
            public_key: secondary_public.public_key.clone(),
            port: secondary_server.port(),
        },
        "127.0.0.1".to_string(),
        Some("loopback".to_string()),
    );
    let discovered = discovery
        .list_devices()
        .into_iter()
        .find(|device| device.device_id == secondary_public.device_id)
        .expect("secondary should be discoverable");

    let primary_manager = ConnectionManager::new();
    let connected_device_id = pin_connected_peer(
        &primary_manager,
        &discovered.ip,
        discovered.port,
        Some(PublicIdentity {
            device_id: discovered.device_id.clone(),
            public_key: discovered.public_key.clone(),
        }),
    )
    .unwrap();
    assert_eq!(connected_device_id, secondary_public.device_id);
    assert!(primary_manager.is_pinned(&secondary_public.device_id));

    let nonce = generate_nonce();
    let primary_code = derive_pairing_code(
        &primary_public.public_key,
        &secondary_public.public_key,
        &nonce,
    );
    let secondary_code = derive_pairing_code(
        &secondary_public.public_key,
        &primary_public.public_key,
        &nonce,
    );
    assert_eq!(primary_code, secondary_code);

    let task_id = Uuid::new_v4();
    let primary_dir = TempDir::new().unwrap();
    let secondary_dir = TempDir::new().unwrap();
    primary_server
        .register_task_root(task_id.to_string(), primary_dir.path())
        .unwrap();

    let register_response = lanbridge::transport::connection::send_authenticated_message_to_peer(
        &primary_manager,
        &primary_identity,
        &secondary_public.device_id,
        SyncMessage::TaskRegister {
            task_id: task_id.to_string(),
            root_path: secondary_dir.path().to_string_lossy().to_string(),
        },
    )
    .await
    .unwrap();
    match register_response {
        SyncMessage::TaskAck { success, error, .. } => {
            assert!(success, "unexpected task ack error: {:?}", error);
        }
        other => panic!("expected TaskAck, got {:?}", other),
    }

    std::fs::create_dir_all(primary_dir.path().join("docs")).unwrap();
    let source_path = primary_dir.path().join("docs").join("report.txt");
    std::fs::write(&source_path, "network flow is closed").unwrap();

    let empty_remote_scan = lanbridge::transport::connection::request_authenticated_scan(
        &primary_manager,
        &primary_identity,
        &secondary_public.device_id,
        task_id.to_string(),
    )
    .await
    .unwrap();
    assert!(empty_remote_scan.is_empty());

    let platform = MacPlatform::with_data_dir(primary_dir.path().join("app-data"));
    let mut snapshots = scanner::scan_root(primary_dir.path(), &platform)
        .unwrap()
        .into_iter()
        .map(|result| result.snapshot)
        .filter(|snapshot| snapshot.kind == EntryKind::File)
        .collect::<Vec<_>>();
    for snapshot in &mut snapshots {
        snapshot.task_id = task_id;
    }
    let actions = planner::plan_sync(&snapshots, &[], DeviceRole::Primary);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].decision, SyncDecision::ApplyToSecondary);

    for action in &actions {
        let snapshot = action.snapshot.as_ref().expect("apply action has snapshot");
        lanbridge::transport::connection::send_authenticated_file_to_peer(
            &primary_manager,
            &primary_identity,
            &secondary_public.device_id,
            task_id.to_string(),
            &snapshot.relative_path,
            &primary_dir.path().join(&snapshot.relative_path),
        )
        .await
        .unwrap();
    }

    let received_path = secondary_dir.path().join("docs").join("report.txt");
    assert_eq!(
        std::fs::read_to_string(&received_path).unwrap(),
        "network flow is closed"
    );

    let remote_scan = lanbridge::transport::connection::request_authenticated_scan(
        &primary_manager,
        &primary_identity,
        &secondary_public.device_id,
        task_id.to_string(),
    )
    .await
    .unwrap();
    let remote_report = remote_scan
        .iter()
        .find(|file| file.relative_path == "docs/report.txt")
        .expect("remote scan should include transferred file");
    assert_eq!(remote_report.blake3_hash, snapshots[0].blake3_hash);

    let conn = Connection::open_in_memory().unwrap();
    db::migrate(&conn).unwrap();
    SyncTaskRepository::new(&conn)
        .insert(&SyncTask {
            id: task_id,
            name: "Full network flow".to_string(),
            primary_device_id: primary_public.device_id.clone(),
            secondary_device_id: secondary_public.device_id.clone(),
            local_path: primary_dir.path().to_string_lossy().to_string(),
            remote_path: secondary_dir.path().to_string_lossy().to_string(),
            local_role: DeviceRole::Primary,
            enabled: true,
            created_unix_ms: 1,
            updated_unix_ms: 1,
        })
        .unwrap();
    let baseline_repo = SyncBaselineRepository::new(&conn);
    baseline_repo
        .upsert(&SyncBaseline {
            task_id,
            relative_path: snapshots[0].relative_path.clone(),
            primary_hash: snapshots[0].blake3_hash.clone(),
            primary_hash_status: snapshots[0].hash_status,
            primary_size: snapshots[0].size,
            primary_modified_unix_ms: snapshots[0].modified_unix_ms,
            secondary_hash: remote_report.blake3_hash.clone(),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: remote_report.modified_unix_ms,
            last_synced_unix_ms: 1,
        })
        .unwrap();
    let baselines = baseline_repo.list_by_task(&task_id).unwrap();
    assert_eq!(baselines.len(), 1);
    assert_eq!(baselines[0].primary_hash, baselines[0].secondary_hash);
}

// ===== Transfer Tests =====

#[test]
fn test_transfer_protocol_roundtrip() {
    // This test doesn't use async, just verifies the protocol design
    let msg = SyncMessage::FileDelete {
        task_id: "task-456".to_string(),
        relative_path: "old.txt".to_string(),
    };

    let encoded = encode_message(&msg).unwrap();
    let decoded = decode_message(&encoded[4..]).unwrap();

    match decoded {
        SyncMessage::FileDelete {
            task_id,
            relative_path,
        } => {
            assert_eq!(task_id, "task-456");
            assert_eq!(relative_path, "old.txt");
        }
        _ => panic!("wrong message type"),
    }
}

async fn read_one_message(stream: &mut tokio::net::TcpStream) -> SyncMessage {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await.unwrap();
    decode_message(&buf).unwrap()
}

async fn write_message(stream: &mut tokio::net::TcpStream, message: SyncMessage) {
    stream
        .write_all(&encode_message(&message).unwrap())
        .await
        .unwrap();
}

async fn authenticate_test_stream(stream: &mut tokio::net::TcpStream, identity: &DeviceIdentity) {
    let device_id = identity.public().device_id;
    stream
        .write_all(
            &encode_message(&SyncMessage::AuthHello {
                device_id: device_id.clone(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let nonce = match read_one_message(stream).await {
        SyncMessage::AuthChallenge { nonce } => nonce,
        other => panic!("expected AuthChallenge, got {:?}", other),
    };
    let signature = identity
        .sign(&lanbridge::transport::connection::auth_payload(
            &device_id, &nonce,
        ))
        .to_bytes()
        .to_vec();
    stream
        .write_all(
            &encode_message(&SyncMessage::AuthProof {
                device_id,
                signature,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    match read_one_message(stream).await {
        SyncMessage::AuthOk { .. } => {}
        other => panic!("expected AuthOk, got {:?}", other),
    }
}

async fn authenticate_legacy_server_stream(
    stream: &mut tokio::net::TcpStream,
    trusted: &PublicIdentity,
) {
    let device_id = match read_one_message(stream).await {
        SyncMessage::AuthHello { device_id } => device_id,
        other => panic!("expected AuthHello, got {:?}", other),
    };
    assert_eq!(device_id, trusted.device_id);
    let nonce = vec![4u8; 32];
    write_message(
        stream,
        SyncMessage::AuthChallenge {
            nonce: nonce.clone(),
        },
    )
    .await;

    match read_one_message(stream).await {
        SyncMessage::AuthProof {
            device_id,
            signature,
        } => {
            assert_eq!(device_id, trusted.device_id);
            let signature = ed25519_dalek::Signature::from_slice(&signature).unwrap();
            DeviceIdentity::verify(
                &trusted.public_key,
                &lanbridge::transport::connection::auth_payload(&device_id, &nonce),
                &signature,
            )
            .unwrap();
            write_message(stream, SyncMessage::AuthOk { device_id }).await;
        }
        other => panic!("expected AuthProof, got {:?}", other),
    }
}

async fn assert_file_ack_success(stream: &mut tokio::net::TcpStream) {
    match read_one_message(stream).await {
        SyncMessage::FileAck { success, error, .. } => {
            assert!(success, "unexpected FileAck error: {:?}", error);
        }
        other => panic!("expected FileAck, got {:?}", other),
    }
}

async fn assert_file_chunk_ack_success(
    stream: &mut tokio::net::TcpStream,
    expected_received_bytes: u64,
) {
    match read_one_message(stream).await {
        SyncMessage::FileChunkAck {
            received_bytes,
            success,
            error,
            ..
        } => {
            assert!(success, "unexpected FileChunkAck error: {:?}", error);
            assert_eq!(received_bytes, expected_received_bytes);
        }
        other => panic!("expected FileChunkAck, got {:?}", other),
    }
}
