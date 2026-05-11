use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::net::TcpStream;

use crate::pairing::PublicIdentity;

/// Connection state for a paired peer.
#[derive(Debug, Clone)]
pub struct PeerConnection {
    pub device_id: String,
    pub address: String,
    pub connected: bool,
    pub last_seen_unix_ms: i64,
}

/// Connection manager for paired peers.
///
/// P0: Manual IP connection only.
/// P1: UDP discovery with manual IP fallback.
pub struct ConnectionManager {
    peers: Arc<Mutex<HashMap<String, PeerConnection>>>,
    pinned_identities: Arc<Mutex<HashMap<String, PublicIdentity>>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            peers: Arc::new(Mutex::new(HashMap::new())),
            pinned_identities: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Pin a peer identity after successful pairing.
    pub fn pin_peer(&self, identity: PublicIdentity) {
        let mut pins = self.pinned_identities.lock().unwrap();
        pins.insert(identity.device_id.clone(), identity);
    }

    /// Check if a device ID is pinned.
    pub fn is_pinned(&self, device_id: &str) -> bool {
        let pins = self.pinned_identities.lock().unwrap();
        pins.contains_key(device_id)
    }

    /// Get a pinned peer identity.
    pub fn get_pinned(&self, device_id: &str) -> Option<PublicIdentity> {
        let pins = self.pinned_identities.lock().unwrap();
        pins.get(device_id).cloned()
    }

    /// Register a connected peer.
    pub fn register_connection(&self, conn: PeerConnection) {
        let mut peers = self.peers.lock().unwrap();
        peers.insert(conn.device_id.clone(), conn);
    }

    /// Mark a peer as disconnected.
    pub fn disconnect(&self, device_id: &str) {
        let mut peers = self.peers.lock().unwrap();
        if let Some(peer) = peers.get_mut(device_id) {
            peer.connected = false;
        }
    }

    /// Check if a peer is currently connected.
    pub fn is_connected(&self, device_id: &str) -> bool {
        let peers = self.peers.lock().unwrap();
        peers.get(device_id).map_or(false, |p| p.connected)
    }

    /// List all known peer connections.
    pub fn list_peers(&self) -> Vec<PeerConnection> {
        let peers = self.peers.lock().unwrap();
        peers.values().cloned().collect()
    }
}

/// Attempt to connect to a peer at the given address.
///
/// P0: Manual TCP connection. Returns the stream on success.
pub async fn connect_to_peer(address: &str, port: u16) -> Result<TcpStream> {
    let addr = format!("{}:{}", address, port);
    let stream = TcpStream::connect(&addr).await?;
    Ok(stream)
}
