use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;

const MULTICAST_ADDR: &str = "239.10.10.10";
const MULTICAST_PORT: u16 = 53530;
const ANNOUNCE_INTERVAL_SECS: u64 = 5;
const PEER_TIMEOUT_SECS: u64 = 15;
const DISCOVERY_PROTOCOL_VERSION: u16 = 2;
const MIN_SUPPORTED_DISCOVERY_PROTOCOL_VERSION: u16 = 2;

fn current_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn legacy_app_version() -> String {
    "旧版本".to_string()
}

fn legacy_protocol_version() -> u16 {
    1
}

fn current_protocol_version() -> u16 {
    DISCOVERY_PROTOCOL_VERSION
}

fn current_min_protocol_version() -> u16 {
    MIN_SUPPORTED_DISCOVERY_PROTOCOL_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Announce {
    pub device_id: String,
    pub display_name: String,
    #[serde(default)]
    pub public_key: Vec<u8>,
    pub port: u16,
    #[serde(default = "legacy_app_version")]
    pub app_version: String,
    #[serde(default = "legacy_protocol_version")]
    pub protocol_version: u16,
    #[serde(default = "legacy_protocol_version")]
    pub min_protocol_version: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct OnlineDeviceAddress {
    pub ip: String,
    pub port: u16,
    pub interface_name: Option<String>,
    pub last_seen_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OnlineDevice {
    pub device_id: String,
    pub display_name: String,
    pub ip: String,
    pub port: u16,
    pub public_key: Vec<u8>,
    pub app_version: Option<String>,
    pub protocol_version: Option<u16>,
    pub compatible: bool,
    pub compatibility_reason: Option<String>,
    pub addresses: Vec<OnlineDeviceAddress>,
    pub last_seen_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryStatus {
    pub running: bool,
    pub error: Option<String>,
    pub interfaces: Vec<String>,
    pub multicast_addr: String,
    pub multicast_port: u16,
}

#[derive(Debug, Clone)]
struct PeerRecord {
    announce: Announce,
    addresses: HashMap<String, OnlineDeviceAddress>,
}

pub struct DiscoveryState {
    peers: Mutex<HashMap<String, PeerRecord>>,
    devices: Mutex<Vec<OnlineDevice>>,
    status: Mutex<DiscoveryStatus>,
}

impl DiscoveryState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::with_status(false, None, Vec::new()))
    }

    pub fn failed(error: String) -> Arc<Self> {
        Arc::new(Self::with_status(false, Some(error), Vec::new()))
    }

    fn with_status(running: bool, error: Option<String>, interfaces: Vec<String>) -> Self {
        Self {
            peers: Mutex::new(HashMap::new()),
            devices: Mutex::new(Vec::new()),
            status: Mutex::new(DiscoveryStatus {
                running,
                error,
                interfaces,
                multicast_addr: MULTICAST_ADDR.to_string(),
                multicast_port: MULTICAST_PORT,
            }),
        }
    }

    pub fn list_devices(&self) -> Vec<OnlineDevice> {
        prune_peers(self);
        self.devices.lock().unwrap().clone()
    }

    pub fn status(&self) -> DiscoveryStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn mark_running(&self, interfaces: Vec<String>) {
        let mut status = self.status.lock().unwrap();
        status.running = true;
        status.error = None;
        status.interfaces = interfaces;
    }

    pub fn mark_failed(&self, error: String) {
        let mut status = self.status.lock().unwrap();
        status.running = false;
        status.error = Some(error);
    }

    pub fn record_peer(&self, announce: Announce, ip: String, interface_name: Option<String>) {
        if announce.port == 0 {
            tracing::warn!(
                "ignoring discovery announce from {} without a valid TCP port",
                announce.device_id
            );
            return;
        }

        let last_seen = now_ms();
        let mut peers = self.peers.lock().unwrap();
        let record = peers
            .entry(announce.device_id.clone())
            .or_insert_with(|| PeerRecord {
                announce: announce.clone(),
                addresses: HashMap::new(),
            });

        record.announce = announce.clone();
        record.addresses.insert(
            ip.clone(),
            OnlineDeviceAddress {
                ip,
                port: announce.port,
                interface_name,
                last_seen_unix_ms: last_seen,
            },
        );

        refresh_devices(&mut peers, &self.devices);
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn start_in_background(
    device_id: String,
    display_name: String,
    public_key: Vec<u8>,
    port: u16,
) -> Result<Arc<DiscoveryState>> {
    if port == 0 {
        return Err(anyhow!("discovery requires a valid TCP port"));
    }

    let state = DiscoveryState::new();
    let sockets = create_multicast_sockets()?;
    let interface_names = sockets
        .iter()
        .filter_map(|(_, name)| name.clone())
        .collect::<Vec<_>>();
    state.mark_running(interface_names);

    let return_state = state.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to create discovery tokio runtime");

        rt.block_on(async move {
            let announce = Announce {
                device_id: device_id.clone(),
                display_name,
                public_key,
                port,
                app_version: current_app_version(),
                protocol_version: current_protocol_version(),
                min_protocol_version: current_min_protocol_version(),
            };

            for (socket, interface_name) in sockets {
                let socket = match UdpSocket::from_std(socket) {
                    Ok(socket) => Arc::new(socket),
                    Err(e) => {
                        state.mark_failed(format!("failed to create tokio UDP socket: {}", e));
                        continue;
                    }
                };

                let announce_socket = socket.clone();
                let announce_msg = announce.clone();
                tokio::spawn(async move {
                    announce_loop(announce_socket, announce_msg).await;
                });

                let listen_state = state.clone();
                let listen_device_id = device_id.clone();
                tokio::spawn(async move {
                    listen_loop(socket, listen_device_id, listen_state, interface_name).await;
                });
            }

            std::future::pending::<()>().await;
        });
    });

    tracing::info!(
        "discovery service started on {}:{}",
        MULTICAST_ADDR,
        MULTICAST_PORT
    );
    Ok(return_state)
}

async fn announce_loop(socket: Arc<UdpSocket>, announce: Announce) {
    let data = serde_json::to_vec(&announce).expect("failed to serialize announce");
    let multicast_addr: SocketAddr = format!("{}:{}", MULTICAST_ADDR, MULTICAST_PORT)
        .parse()
        .unwrap();
    let broadcast_addr: SocketAddr = format!("255.255.255.255:{}", MULTICAST_PORT)
        .parse()
        .unwrap();

    loop {
        if let Err(e) = socket.send_to(&data, multicast_addr).await {
            tracing::warn!("failed to send multicast announce: {}", e);
        }
        if let Err(e) = socket.send_to(&data, broadcast_addr).await {
            tracing::debug!("failed to send broadcast announce: {}", e);
        }
        tokio::time::sleep(Duration::from_secs(ANNOUNCE_INTERVAL_SECS)).await;
    }
}

async fn listen_loop(
    socket: Arc<UdpSocket>,
    local_device_id: String,
    state: Arc<DiscoveryState>,
    interface_name: Option<String>,
) {
    let mut buf = [0u8; 2048];

    loop {
        match tokio::time::timeout(
            Duration::from_secs(ANNOUNCE_INTERVAL_SECS),
            socket.recv_from(&mut buf),
        )
        .await
        {
            Ok(Ok((len, addr))) => {
                let ip = match addr {
                    SocketAddr::V4(v4) => v4.ip().to_string(),
                    SocketAddr::V6(v6) => v6.ip().to_string(),
                };
                if let Ok(peer) = serde_json::from_slice::<Announce>(&buf[..len]) {
                    if peer.device_id != local_device_id {
                        state.record_peer(peer, ip, interface_name.clone());
                    }
                }
            }
            Ok(Err(e)) => tracing::warn!("discovery recv error: {}", e),
            Err(_) => prune_peers(&state),
        }
    }
}

fn prune_peers(state: &DiscoveryState) {
    let mut peers = state.peers.lock().unwrap();
    refresh_devices(&mut peers, &state.devices);
}

fn refresh_devices(peers: &mut HashMap<String, PeerRecord>, devices: &Mutex<Vec<OnlineDevice>>) {
    let cutoff = now_ms() - (PEER_TIMEOUT_SECS as i64) * 1000;
    peers.retain(|_, record| {
        record
            .addresses
            .retain(|_, addr| addr.last_seen_unix_ms > cutoff);
        !record.addresses.is_empty()
    });

    let mut next = peers
        .values()
        .filter_map(|record| {
            let mut addresses = record.addresses.values().cloned().collect::<Vec<_>>();
            addresses.sort_by(|a, b| address_score(b).cmp(&address_score(a)));
            let preferred = addresses.first()?.clone();
            let last_seen = addresses
                .iter()
                .map(|addr| addr.last_seen_unix_ms)
                .max()
                .unwrap_or(preferred.last_seen_unix_ms);

            Some(OnlineDevice {
                device_id: record.announce.device_id.clone(),
                display_name: record.announce.display_name.clone(),
                ip: preferred.ip,
                port: preferred.port,
                public_key: record.announce.public_key.clone(),
                app_version: Some(record.announce.app_version.clone()),
                protocol_version: Some(record.announce.protocol_version),
                compatible: announce_is_compatible(&record.announce),
                compatibility_reason: announce_compatibility_reason(&record.announce),
                addresses,
                last_seen_unix_ms: last_seen,
            })
        })
        .collect::<Vec<_>>();

    next.sort_by(|a, b| b.last_seen_unix_ms.cmp(&a.last_seen_unix_ms));
    *devices.lock().unwrap() = next;
}

fn address_score(addr: &OnlineDeviceAddress) -> i32 {
    let ip_score = addr
        .ip
        .parse::<Ipv4Addr>()
        .map(|ip| {
            if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() {
                -100
            } else if ip.is_private() {
                100
            } else {
                10
            }
        })
        .unwrap_or(0);

    let iface_penalty = addr
        .interface_name
        .as_deref()
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            if lower.contains("vpn")
                || lower.contains("tun")
                || lower.contains("tap")
                || lower.contains("utun")
                || lower.contains("virtual")
                || lower.contains("hyper-v")
                || lower.contains("vmware")
            {
                -40
            } else {
                0
            }
        })
        .unwrap_or(0);

    ip_score + iface_penalty
}

pub fn announce_is_compatible(announce: &Announce) -> bool {
    announce.protocol_version >= MIN_SUPPORTED_DISCOVERY_PROTOCOL_VERSION
        && announce.min_protocol_version <= DISCOVERY_PROTOCOL_VERSION
}

pub fn announce_compatibility_reason(announce: &Announce) -> Option<String> {
    if announce_is_compatible(announce) {
        None
    } else {
        Some("版本不兼容，请升级".to_string())
    }
}

fn create_multicast_sockets() -> Result<Vec<(std::net::UdpSocket, Option<String>)>> {
    let multicast_addr = MULTICAST_ADDR.parse::<Ipv4Addr>()?;
    let interfaces = local_ipv4_interfaces();
    let targets = if interfaces.is_empty() {
        vec![(None, Ipv4Addr::UNSPECIFIED)]
    } else {
        interfaces
    };

    let mut sockets = Vec::new();
    let mut errors = Vec::new();
    for (name, interface_ip) in targets {
        match create_socket_for_interface(multicast_addr, interface_ip) {
            Ok(socket) => sockets.push((socket, name)),
            Err(e) => errors.push(format!("{}: {}", interface_ip, e)),
        }
    }

    if sockets.is_empty() {
        return Err(anyhow!(
            "failed to bind discovery sockets on all interfaces: {}",
            errors.join("; ")
        ));
    }

    Ok(sockets)
}

fn local_ipv4_interfaces() -> Vec<(Option<String>, Ipv4Addr)> {
    local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, ip)| match ip {
            IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() => {
                Some((Some(name), ip))
            }
            _ => None,
        })
        .collect()
}

fn create_socket_for_interface(
    multicast_addr: Ipv4Addr,
    interface_ip: Ipv4Addr,
) -> Result<std::net::UdpSocket> {
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MULTICAST_PORT);
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.set_broadcast(true)?;
    socket.set_multicast_loop_v4(false)?;
    socket.set_multicast_ttl_v4(1)?;
    socket.bind(&addr.into())?;
    socket.join_multicast_v4(&multicast_addr, &interface_ip)?;
    if !interface_ip.is_unspecified() {
        socket.set_multicast_if_v4(&interface_ip)?;
    }

    Ok(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_addr(ip: &str, interface: Option<&str>) -> OnlineDeviceAddress {
        OnlineDeviceAddress {
            ip: ip.to_string(),
            port: 9527,
            interface_name: interface.map(|s| s.to_string()),
            last_seen_unix_ms: now_ms(),
        }
    }

    fn make_announce(device_id: &str, display_name: &str, port: u16) -> Announce {
        Announce {
            device_id: device_id.to_string(),
            display_name: display_name.to_string(),
            public_key: vec![1, 2, 3],
            port,
            app_version: current_app_version(),
            protocol_version: DISCOVERY_PROTOCOL_VERSION,
            min_protocol_version: MIN_SUPPORTED_DISCOVERY_PROTOCOL_VERSION,
        }
    }

    #[test]
    fn address_score_prefers_private_ip() {
        let private = make_addr("192.168.1.100", None);
        let loopback = make_addr("127.0.0.1", None);
        let link_local = make_addr("169.254.1.1", None);

        assert!(
            address_score(&private) > 0,
            "private IP should score positively"
        );
        assert!(
            address_score(&loopback) < 0,
            "loopback should score negatively"
        );
        assert!(
            address_score(&link_local) < 0,
            "link-local should score negatively"
        );
        assert!(address_score(&private) > address_score(&loopback));
    }

    #[test]
    fn address_score_penalizes_vpn_interfaces() {
        let normal = make_addr("192.168.1.100", Some("en0"));
        let vpn = make_addr("192.168.1.100", Some("utun0"));
        assert!(address_score(&normal) > address_score(&vpn));

        let hyperv = make_addr("10.0.0.1", Some("vEthernet (Hyper-V)"));
        assert!(address_score(&normal) > address_score(&hyperv));
    }

    #[test]
    fn record_peer_adds_to_device_list() {
        let state = DiscoveryState::new();
        let announce = make_announce("dev-1", "Mac", 9527);

        state.record_peer(
            announce.clone(),
            "192.168.1.5".to_string(),
            Some("en0".to_string()),
        );
        let devices = state.list_devices();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, "dev-1");
        assert_eq!(devices[0].display_name, "Mac");
        assert_eq!(devices[0].ip, "192.168.1.5");
        assert_eq!(devices[0].port, 9527);
        assert!(devices[0].compatible);
        assert_eq!(devices[0].protocol_version, Some(DISCOVERY_PROTOCOL_VERSION));
    }

    #[test]
    fn record_peer_ignores_zero_port() {
        let state = DiscoveryState::new();
        let announce = make_announce("dev-1", "Ghost", 0);
        state.record_peer(announce, "192.168.1.5".to_string(), None);
        assert_eq!(state.list_devices().len(), 0);
    }

    #[test]
    fn record_peer_updates_existing_device() {
        let state = DiscoveryState::new();
        let announce1 = make_announce("dev-1", "Old", 9527);
        let mut announce2 = make_announce("dev-1", "New", 9528);
        announce2.public_key = vec![2];

        state.record_peer(announce1, "192.168.1.5".to_string(), None);
        state.record_peer(announce2, "192.168.1.6".to_string(), None);
        let devices = state.list_devices();
        assert_eq!(
            devices.len(),
            1,
            "same device_id should update, not duplicate"
        );
        assert_eq!(devices[0].display_name, "New");
        assert!(devices[0].addresses.len() >= 2);
    }

    #[test]
    fn prune_peers_removes_stale_addresses() {
        let state = DiscoveryState::new();
        let announce = make_announce("dev-1", "Stale", 9527);

        // Record with a timestamp far in the past
        {
            let mut peers = state.peers.lock().unwrap();
            let mut addresses = std::collections::HashMap::new();
            addresses.insert(
                "192.168.1.5".to_string(),
                OnlineDeviceAddress {
                    ip: "192.168.1.5".to_string(),
                    port: 9527,
                    interface_name: None,
                    last_seen_unix_ms: now_ms() - 20_000, // 20 seconds ago
                },
            );
            peers.insert(
                "dev-1".to_string(),
                PeerRecord {
                    announce: announce.clone(),
                    addresses,
                },
            );
        }

        // Pruning should remove the stale peer
        prune_peers(&state);
        let devices = state.list_devices();
        assert!(devices.is_empty(), "stale peers should be pruned");
    }

    #[test]
    fn list_devices_prunes_stale_cached_devices() {
        let state = DiscoveryState::new();
        let announce = make_announce("dev-1", "Stale", 9527);

        state.record_peer(announce, "192.168.1.5".to_string(), None);
        assert_eq!(state.devices.lock().unwrap().len(), 1);

        {
            let mut peers = state.peers.lock().unwrap();
            let record = peers.get_mut("dev-1").unwrap();
            for address in record.addresses.values_mut() {
                address.last_seen_unix_ms = now_ms() - ((PEER_TIMEOUT_SECS as i64) * 1000) - 1_000;
            }
        }

        let devices = state.list_devices();
        assert!(
            devices.is_empty(),
            "list_devices should not return stale cached peers"
        );
    }

    #[test]
    fn legacy_announce_without_version_is_visible_but_incompatible() {
        let json = serde_json::json!({
            "device_id": "old-dev",
            "display_name": "Old Mac",
            "public_key": [1, 2, 3],
            "port": 9527
        });
        let announce: Announce = serde_json::from_value(json).unwrap();
        assert_eq!(announce.protocol_version, 1);
        assert!(!announce_is_compatible(&announce));

        let state = DiscoveryState::new();
        state.record_peer(announce, "192.168.1.8".to_string(), None);
        let devices = state.list_devices();
        assert_eq!(devices.len(), 1);
        assert!(!devices[0].compatible);
        assert_eq!(
            devices[0].compatibility_reason.as_deref(),
            Some("版本不兼容，请升级")
        );
    }

    #[test]
    fn future_announce_with_higher_min_protocol_is_incompatible() {
        let mut announce = make_announce("future-dev", "Future", 9527);
        announce.protocol_version = DISCOVERY_PROTOCOL_VERSION + 1;
        announce.min_protocol_version = DISCOVERY_PROTOCOL_VERSION + 1;

        assert!(!announce_is_compatible(&announce));
        assert_eq!(
            announce_compatibility_reason(&announce).as_deref(),
            Some("版本不兼容，请升级")
        );
    }

    #[test]
    fn discovery_state_failed() {
        let state = DiscoveryState::failed("no interfaces".to_string());
        let status = state.status();
        assert!(!status.running);
        assert_eq!(status.error, Some("no interfaces".to_string()));
    }

    #[test]
    fn discovery_state_mark_running() {
        let state = DiscoveryState::new();
        state.mark_running(vec!["en0".to_string(), "eth0".to_string()]);
        let status = state.status();
        assert!(status.running);
        assert!(status.error.is_none());
        assert_eq!(status.interfaces.len(), 2);
    }
}
