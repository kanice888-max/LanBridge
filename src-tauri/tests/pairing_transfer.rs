use lan_folder_sync::pairing::{derive_pairing_code, generate_nonce, DeviceIdentity};
use lan_folder_sync::transport::{decode_message, encode_message, ConnectionManager, DiscoveryService, SyncMessage};
use tempfile::TempDir;

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

    assert_ne!(code_ab, code_ac, "different pairs should have different codes");
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
        SyncMessage::FileTransfer { relative_path, total_bytes, .. } => {
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

    manager.register_connection(lan_folder_sync::transport::PeerConnection {
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
fn test_discovery_disabled_in_p0() {
    let mut discovery = DiscoveryService::new();
    assert!(!discovery.is_enabled());
    discovery.start();
    assert!(!discovery.is_enabled(), "P0 should disable UDP discovery");
}

// ===== Transfer Tests =====

#[test]
fn test_transfer_protocol_roundtrip() {
    use std::io::Cursor;

    // This test doesn't use async, just verifies the protocol design
    let msg = SyncMessage::FileDelete {
        task_id: "task-456".to_string(),
        relative_path: "old.txt".to_string(),
    };

    let encoded = encode_message(&msg).unwrap();
    let decoded = decode_message(&encoded[4..]).unwrap();

    match decoded {
        SyncMessage::FileDelete { task_id, relative_path } => {
            assert_eq!(task_id, "task-456");
            assert_eq!(relative_path, "old.txt");
        }
        _ => panic!("wrong message type"),
    }
}
