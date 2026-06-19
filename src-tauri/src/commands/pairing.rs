use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;
use tauri::State;

use crate::app_state::AppState;
use crate::pairing;
use crate::transport::connection;
use crate::transport::{DiscoveryStatus, OnlineDevice};

use super::now_ms;

// ─── Identity ───

#[derive(Debug, Clone, Serialize)]
pub struct IdentityInfo {
    pub device_id: String,
    pub display_name: String,
}

#[tauri::command]
pub fn get_identity(state: State<'_, AppState>) -> Result<IdentityInfo, String> {
    let pub_id = state.identity.public();
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "Device".to_string());
    Ok(IdentityInfo {
        device_id: pub_id.device_id,
        display_name: hostname,
    })
}

// ─── Pairing ───

#[tauri::command]
pub fn start_pairing(_state: State<'_, AppState>) -> Result<String, String> {
    let nonce = pairing::generate_nonce();
    Ok(hex::encode(&nonce))
}

#[tauri::command]
pub fn confirm_pairing_code(
    state: State<'_, AppState>,
    peer_device_id: String,
    peer_public_key: Vec<u8>,
    nonce_hex: String,
) -> Result<String, String> {
    let nonce = hex::decode(&nonce_hex).map_err(|e| e.to_string())?;
    let local_pub = state.identity.public();
    let code = pairing::derive_pairing_code(&local_pub.public_key, &peer_public_key, &nonce);

    state.connections.pin_peer(pairing::PublicIdentity {
        device_id: peer_device_id.clone(),
        public_key: peer_public_key,
    });

    Ok(code)
}

#[tauri::command]
pub fn approve_pairing(
    state: State<'_, AppState>,
    peer_device_id: String,
    display_name: String,
) -> Result<(), String> {
    let pinned = state
        .connections
        .get_pinned(&peer_device_id)
        .ok_or("peer not found")?;

    let device = crate::core::model::PairedDevice {
        device_id: peer_device_id.clone(),
        display_name,
        public_key: pinned.public_key,
        last_seen_unix_ms: now_ms(),
        trusted: true,
        last_address: state
            .connections
            .get_peer(&peer_device_id)
            .map(|peer| peer.address),
    };
    if let Some(server) = &state._server {
        server.register_trusted_peer(pairing::PublicIdentity {
            device_id: device.device_id.clone(),
            public_key: device.public_key.clone(),
        });
    }

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = crate::state::repository::PairedDeviceRepository::new(&db);
    repo.upsert(&device).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_paired_devices(
    state: State<'_, AppState>,
) -> Result<Vec<crate::core::model::PairedDevice>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = crate::state::repository::PairedDeviceRepository::new(&db);
    repo.list_all().map_err(|e| e.to_string())
}

// ─── Manual Connection ───

#[tauri::command]
pub async fn connect_peer(
    state: State<'_, AppState>,
    address: String,
    port: u16,
) -> Result<String, String> {
    connection::ping_peer_address(&address, port)
        .await
        .map_err(|e| e.to_string())?;

    let peer_identity = connection::request_peer_identity(&address, port)
        .await
        .map_err(|e| e.to_string())?;
    ensure_not_local_device(&state.identity.public().device_id, &peer_identity.device_id)?;
    let peer = Some(peer_identity);
    crate::transport::connection::pin_connected_peer(&state.connections, &address, port, peer)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn connect_discovered_peer(
    state: State<'_, AppState>,
    address: String,
    port: u16,
    peer_device_id: String,
    peer_public_key: Vec<u8>,
) -> Result<String, String> {
    ensure_not_local_device(&state.identity.public().device_id, &peer_device_id)?;
    if let Some(device) = state
        .discovery
        .list_devices()
        .into_iter()
        .find(|device| device.device_id == peer_device_id)
    {
        if !device.compatible {
            return Err(device
                .compatibility_reason
                .unwrap_or_else(|| "对端版本不兼容，请升级两端应用".to_string()));
        }
    }

    let (reachable_address, reachable_port) =
        connect_first_reachable_address(&state, &peer_device_id, &address, port).await?;

    let peer = peer_identity_from_args(Some(peer_device_id), Some(peer_public_key));
    crate::transport::connection::pin_connected_peer(
        &state.connections,
        &reachable_address,
        reachable_port,
        peer,
    )
    .map_err(|e| e.to_string())
}

async fn connect_first_reachable_address(
    state: &State<'_, AppState>,
    peer_device_id: &str,
    requested_address: &str,
    requested_port: u16,
) -> Result<(String, u16), String> {
    let mut candidates = vec![(requested_address.to_string(), requested_port)];
    if let Some(device) = state
        .discovery
        .list_devices()
        .into_iter()
        .find(|device| device.device_id == peer_device_id)
    {
        for address in device.addresses {
            let candidate = (address.ip, address.port);
            if !candidates.iter().any(|item| item == &candidate) {
                candidates.push(candidate);
            }
        }
    }

    let mut last_error = None;
    for (address, port) in candidates {
        match connection::ping_peer_address(&address, port).await {
            Ok(()) => return Ok((address, port)),
            Err(e) => last_error = Some(e.to_string()),
        }
    }

    Err(last_error.unwrap_or_else(|| "peer is unreachable".to_string()))
}

fn peer_identity_from_args(
    device_id: Option<String>,
    public_key: Option<Vec<u8>>,
) -> Option<pairing::PublicIdentity> {
    match (device_id, public_key) {
        (Some(device_id), Some(public_key)) if !device_id.is_empty() && !public_key.is_empty() => {
            Some(pairing::PublicIdentity {
                device_id,
                public_key,
            })
        }
        _ => None,
    }
}

fn ensure_not_local_device(local_device_id: &str, peer_device_id: &str) -> Result<(), String> {
    if !peer_device_id.is_empty() && peer_device_id == local_device_id {
        return Err("不能连接本机".to_string());
    }
    Ok(())
}

// ─── Online Devices ───

#[tauri::command]
pub fn list_online_devices(state: State<'_, AppState>) -> Result<Vec<OnlineDevice>, String> {
    Ok(state.discovery.list_devices())
}

#[tauri::command]
pub fn get_discovery_status(state: State<'_, AppState>) -> Result<DiscoveryStatus, String> {
    Ok(state.discovery.status())
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkCheckItem {
    pub label: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkDiagnosticReport {
    pub ok: bool,
    pub tcp_port: u16,
    pub checks: Vec<NetworkCheckItem>,
    pub suggestions: Vec<String>,
}

#[tauri::command]
pub fn check_network_environment(
    state: State<'_, AppState>,
) -> Result<NetworkDiagnosticReport, String> {
    let mut checks = Vec::new();
    let mut suggestions = Vec::new();
    let tcp_port = state._server.as_ref().map_or(0, |server| server.port());

    if tcp_port == 0 {
        checks.push(NetworkCheckItem {
            label: "TCP 服务".to_string(),
            status: "error".to_string(),
            detail: "同步服务未监听端口，其他设备无法连接到本机。".to_string(),
        });
        suggestions.push("重启应用；如果仍失败，请检查本机端口占用或安全软件拦截。".to_string());
    } else {
        let localhost = SocketAddr::from((Ipv4Addr::LOCALHOST, tcp_port));
        match TcpStream::connect_timeout(&localhost, Duration::from_millis(500)) {
            Ok(_) => checks.push(NetworkCheckItem {
                label: "TCP 服务".to_string(),
                status: "ok".to_string(),
                detail: format!("本机同步服务正在监听 {} 端口。", tcp_port),
            }),
            Err(e) => {
                checks.push(NetworkCheckItem {
                    label: "TCP 服务".to_string(),
                    status: "error".to_string(),
                    detail: format!("本机无法连通 127.0.0.1:{}：{}", tcp_port, e),
                });
                suggestions.push("确认安全软件或系统防火墙没有拦截本应用的 TCP 服务。".to_string());
            }
        }
    }

    let discovery = state.discovery.status();
    if !discovery.enabled {
        checks.push(NetworkCheckItem {
            label: "自动发现".to_string(),
            status: "warn".to_string(),
            detail: "自动发现已关闭，仍可使用手动 IP 连接。".to_string(),
        });
    } else if discovery.running && discovery.error.is_none() {
        let announce_detail = if discovery.announce_interfaces.is_empty() {
            "暂未识别到广播网卡".to_string()
        } else {
            format!("正在通过 {} 广播", discovery.announce_interfaces.join("；"))
        };
        checks.push(NetworkCheckItem {
            label: "自动发现".to_string(),
            status: "ok".to_string(),
            detail: format!(
                "发现服务运行中，监听 {}:{}，{}。",
                discovery.multicast_addr, discovery.multicast_port, announce_detail
            ),
        });
        if !discovery.skipped_interfaces.is_empty() {
            checks.push(NetworkCheckItem {
                label: "自动发现网卡".to_string(),
                status: "warn".to_string(),
                detail: format!(
                    "检测到低优先级 VPN/虚拟网卡：{}",
                    discovery.skipped_interfaces.join("；")
                ),
            });
        }
        if !discovery.socket_errors.is_empty() {
            checks.push(NetworkCheckItem {
                label: "自动发现诊断".to_string(),
                status: "warn".to_string(),
                detail: discovery.socket_errors.join("；"),
            });
        }
    } else {
        checks.push(NetworkCheckItem {
            label: "自动发现".to_string(),
            status: "error".to_string(),
            detail: discovery
                .error
                .unwrap_or_else(|| "发现服务未运行。".to_string()),
        });
        suggestions.push(
            "自动发现失败时，可先使用手动 IP 连接；同时检查 UDP 组播/广播是否被网络或 VPN 阻断。"
                .to_string(),
        );
    }

    let interfaces = local_ipv4_interfaces();
    if interfaces.is_empty() {
        checks.push(NetworkCheckItem {
            label: "本机网络".to_string(),
            status: "error".to_string(),
            detail: "未检测到可用于局域网发现的 IPv4 地址。".to_string(),
        });
        suggestions.push(
            "确认电脑已连接到有线或无线局域网，并且没有只连接到不可达的 VPN 网络。".to_string(),
        );
    } else {
        let detail = interfaces
            .iter()
            .map(|(name, ip)| format!("{} {}", name, ip))
            .collect::<Vec<_>>()
            .join("；");
        checks.push(NetworkCheckItem {
            label: "本机网络".to_string(),
            status: "ok".to_string(),
            detail,
        });
    }

    let virtual_or_vpn = interfaces
        .iter()
        .filter(|(name, _)| looks_like_virtual_or_vpn(name))
        .map(|(name, ip)| format!("{} {}", name, ip))
        .collect::<Vec<_>>();
    if !virtual_or_vpn.is_empty() {
        checks.push(NetworkCheckItem {
            label: "VPN/虚拟网卡".to_string(),
            status: "warn".to_string(),
            detail: format!(
                "检测到可能影响默认路由的网卡：{}",
                virtual_or_vpn.join("；")
            ),
        });
        suggestions.push(
            "如果自动发现不到设备，请临时关闭 VPN，或确认 VPN 允许局域网访问和 split tunnel。"
                .to_string(),
        );
    }

    let devices = state.discovery.list_devices();
    if devices.is_empty() {
        checks.push(NetworkCheckItem {
            label: "已发现设备".to_string(),
            status: "warn".to_string(),
            detail: "当前没有发现其他设备。".to_string(),
        });
        suggestions.push(platform_network_permission_suggestion());
    } else {
        let address_count: usize = devices.iter().map(|device| device.addresses.len()).sum();
        let incompatible_count = devices.iter().filter(|device| !device.compatible).count();
        checks.push(NetworkCheckItem {
            label: "已发现设备".to_string(),
            status: if incompatible_count > 0 { "warn" } else { "ok" }.to_string(),
            detail: format!(
                "发现 {} 台设备，合计 {} 个可尝试地址{}。",
                devices.len(),
                address_count,
                if incompatible_count > 0 {
                    "，其中有旧版设备"
                } else {
                    ""
                }
            ),
        });
        if incompatible_count > 0 {
            suggestions.push("发现到旧版设备时，请升级两端 LanBridge。".to_string());
        }
    }

    let ok = !checks.iter().any(|check| check.status == "error");
    Ok(NetworkDiagnosticReport {
        ok,
        tcp_port,
        checks,
        suggestions,
    })
}

fn local_ipv4_interfaces() -> Vec<(String, Ipv4Addr)> {
    local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, ip)| match ip {
            IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() => {
                Some((name, ip))
            }
            _ => None,
        })
        .collect()
}

fn looks_like_virtual_or_vpn(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("vpn")
        || lower.contains("tun")
        || lower.contains("tap")
        || lower.contains("utun")
        || lower.contains("virtual")
        || lower.contains("hyper-v")
        || lower.contains("vmware")
        || lower.contains("wsl")
}

#[cfg(target_os = "macos")]
fn platform_network_permission_suggestion() -> String {
    "确认两端应用都已启动；macOS 安装版还需要在“系统设置 > 隐私与安全性 > 本地网络”允许 LanBridge，并允许防火墙接收入站连接。".to_string()
}

#[cfg(target_os = "windows")]
fn platform_network_permission_suggestion() -> String {
    "确认两端应用都已启动；Windows 防火墙需要允许 LanBridge.exe 入站 TCP 当前监听端口和 UDP 53530。"
        .to_string()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_network_permission_suggestion() -> String {
    "确认两端应用都已启动，并且系统防火墙允许本应用在当前网络通信。".to_string()
}

// Hex helper used by pairing
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        if s.len() % 2 != 0 {
            return Err("invalid hex length".to_string());
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::ensure_not_local_device;

    #[test]
    fn reject_local_device_connection() {
        assert!(ensure_not_local_device("local", "local").is_err());
    }

    #[test]
    fn allow_remote_device_connection() {
        assert!(ensure_not_local_device("local", "remote").is_ok());
    }
}
