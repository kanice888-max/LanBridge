use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::State;
use uuid::Uuid;

use crate::app_state::{AppState, PendingOutgoingTaskInvite};
use crate::core::conflict;
use crate::core::executor;
use crate::core::model::*;
use crate::core::planner;
use crate::core::scanner;
use crate::history::store::HistoryStore;
use crate::pairing;
use crate::state::repository;
use crate::transport::protocol::RemoteFileState;
use crate::transport::{connection, SyncMessage};
use crate::transport::{DiscoveryStatus, OnlineDevice};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

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

    // Store the peer as pinned
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

    let device = PairedDevice {
        device_id: peer_device_id,
        display_name,
        public_key: pinned.public_key,
        last_seen_unix_ms: now_ms(),
        trusted: true,
    };
    if let Some(server) = &state._server {
        server.register_trusted_peer(pairing::PublicIdentity {
            device_id: device.device_id.clone(),
            public_key: device.public_key.clone(),
        });
    }

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::PairedDeviceRepository::new(&db);
    repo.upsert(&device).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_paired_devices(state: State<'_, AppState>) -> Result<Vec<PairedDevice>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::PairedDeviceRepository::new(&db);
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

    let peer = Some(
        connection::request_peer_identity(&address, port)
            .await
            .map_err(|e| e.to_string())?,
    );
    Ok(crate::transport::connection::pin_connected_peer(
        &state.connections,
        &address,
        port,
        peer,
    ))
}

#[tauri::command]
pub async fn connect_discovered_peer(
    state: State<'_, AppState>,
    address: String,
    port: u16,
    peer_device_id: String,
    peer_public_key: Vec<u8>,
) -> Result<String, String> {
    let (reachable_address, reachable_port) =
        connect_first_reachable_address(&state, &peer_device_id, &address, port).await?;

    let peer = peer_identity_from_args(Some(peer_device_id), Some(peer_public_key));
    Ok(crate::transport::connection::pin_connected_peer(
        &state.connections,
        &reachable_address,
        reachable_port,
        peer,
    ))
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
        suggestions.push("重启应用；如果仍失败，请检查 9527 端口是否被其他程序占用。".to_string());
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
    if discovery.running && discovery.error.is_none() {
        checks.push(NetworkCheckItem {
            label: "自动发现".to_string(),
            status: "ok".to_string(),
            detail: format!(
                "发现服务运行中，监听 {}:{}。",
                discovery.multicast_addr, discovery.multicast_port
            ),
        });
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
        suggestions.push(
            "确认两端应用都已启动，并且 Windows/macOS 防火墙允许本应用在当前网络通信。".to_string(),
        );
    } else {
        let address_count: usize = devices.iter().map(|device| device.addresses.len()).sum();
        checks.push(NetworkCheckItem {
            label: "已发现设备".to_string(),
            status: "ok".to_string(),
            detail: format!(
                "发现 {} 台设备，合计 {} 个可尝试地址。",
                devices.len(),
                address_count
            ),
        });
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

// ─── Sync Tasks ───

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTaskRequest {
    pub name: String,
    pub local_path: String,
    pub remote_path: Option<String>,
    pub peer_device_id: String,
    pub local_role: String, // "Primary" or "Secondary"
}

#[derive(Debug, Clone, Deserialize)]
pub struct SendTaskInviteRequest {
    pub name: String,
    pub local_path: String,
    pub peer_device_id: String,
    pub local_role: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskInviteProgress {
    pub invite_id: String,
    pub task_id: String,
    pub status: String,
    pub task: Option<SyncTask>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IncomingTaskInviteInfo {
    pub invite_id: String,
    pub task_id: String,
    pub task_name: String,
    pub requester_device_id: String,
    pub requester_address: Option<String>,
    pub requester_path: Option<String>,
    pub proposed_role: String,
    pub status: String,
    pub local_path: Option<String>,
    pub error: Option<String>,
    pub created_unix_ms: i64,
}

#[tauri::command]
pub async fn send_task_invite(
    state: State<'_, AppState>,
    request: SendTaskInviteRequest,
) -> Result<TaskInviteProgress, String> {
    let local_role = parse_device_role(&request.local_role);
    state
        .connections
        .get_pinned(&request.peer_device_id)
        .ok_or("peer device not pinned")?;

    let invite_id = Uuid::new_v4().to_string();
    let task_id = Uuid::new_v4();
    let proposed_role = match local_role {
        DeviceRole::Primary => "Secondary".to_string(),
        DeviceRole::Secondary => "Primary".to_string(),
    };
    let response = send_task_invite_to_peer(
        &state,
        &request.peer_device_id,
        invite_id.clone(),
        task_id.to_string(),
        request.name.clone(),
        Some(request.local_path.clone()),
        proposed_role,
    )
    .await?;

    let pending = PendingOutgoingTaskInvite {
        task_id,
        name: request.name,
        local_path: request.local_path,
        peer_device_id: request.peer_device_id,
        local_role,
    };

    match response {
        SyncMessage::TaskInvitePending { .. } => {
            state
                .pending_outgoing_invites
                .lock()
                .map_err(|e| e.to_string())?
                .insert(invite_id.clone(), pending);
            Ok(TaskInviteProgress {
                invite_id,
                task_id: task_id.to_string(),
                status: "Pending".to_string(),
                task: None,
                error: None,
            })
        }
        SyncMessage::TaskInviteAck {
            success: true,
            remote_path: Some(remote_path),
            ..
        } => {
            let task = create_task_from_invite(&state, &pending, remote_path)?;
            Ok(TaskInviteProgress {
                invite_id,
                task_id: task_id.to_string(),
                status: "Accepted".to_string(),
                task: Some(task),
                error: None,
            })
        }
        SyncMessage::TaskInviteAck { error, .. } => Ok(TaskInviteProgress {
            invite_id,
            task_id: task_id.to_string(),
            status: "Rejected".to_string(),
            task: None,
            error,
        }),
        other => Err(format!("unexpected peer response: {:?}", other)),
    }
}

#[tauri::command]
pub async fn poll_task_invite(
    state: State<'_, AppState>,
    invite_id: String,
) -> Result<TaskInviteProgress, String> {
    let pending = state
        .pending_outgoing_invites
        .lock()
        .map_err(|e| e.to_string())?
        .get(&invite_id)
        .cloned()
        .ok_or("task invite not found")?;

    let status_request = SyncMessage::TaskInviteStatusRequest {
        invite_id: invite_id.clone(),
    };
    let response = match connection::send_authenticated_message_to_peer(
        &state.connections,
        &state.identity,
        &pending.peer_device_id,
        status_request.clone(),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => connection::send_message_to_peer(
            &state.connections,
            &pending.peer_device_id,
            status_request,
        )
        .await
        .map_err(|e| format!("task invitation status failed: {}", e))?,
    };

    match response {
        SyncMessage::TaskInviteStatus {
            status,
            remote_path,
            error,
            ..
        } if status == "Accepted" => {
            let Some(remote_path) = remote_path else {
                return Err("accepted task invite did not include remote path".to_string());
            };
            let task = create_task_from_invite(&state, &pending, remote_path)?;
            state
                .pending_outgoing_invites
                .lock()
                .map_err(|e| e.to_string())?
                .remove(&invite_id);
            Ok(TaskInviteProgress {
                invite_id,
                task_id: pending.task_id.to_string(),
                status,
                task: Some(task),
                error,
            })
        }
        SyncMessage::TaskInviteStatus { status, error, .. } => Ok(TaskInviteProgress {
            invite_id,
            task_id: pending.task_id.to_string(),
            status,
            task: None,
            error,
        }),
        other => Err(format!("unexpected peer response: {:?}", other)),
    }
}

#[tauri::command]
pub fn list_task_invites(
    state: State<'_, AppState>,
) -> Result<Vec<IncomingTaskInviteInfo>, String> {
    let server = state._server.as_ref().ok_or("sync server is not running")?;
    Ok(server
        .list_task_invites()
        .into_iter()
        .map(|invite| IncomingTaskInviteInfo {
            invite_id: invite.invite_id,
            task_id: invite.task_id,
            task_name: invite.task_name,
            requester_device_id: invite.requester_device_id,
            requester_address: invite.requester_address,
            requester_path: invite.requester_path,
            proposed_role: invite.proposed_role,
            status: invite.status,
            local_path: invite.local_path,
            error: invite.error,
            created_unix_ms: invite.created_unix_ms,
        })
        .collect())
}

#[tauri::command]
pub fn accept_task_invite(
    state: State<'_, AppState>,
    invite_id: String,
    local_path: String,
) -> Result<SyncTask, String> {
    let server = state._server.as_ref().ok_or("sync server is not running")?;
    let invite = server
        .accept_task_invite(&invite_id, &local_path)
        .map_err(|e| e.to_string())?;
    let local_role = parse_device_role(&invite.proposed_role);
    let local_identity = state.identity.public();
    let peer_device_id = invite.requester_device_id.clone();
    let requester_address = invite.requester_address.clone();
    let (primary_device_id, secondary_device_id) = match local_role {
        DeviceRole::Primary => (local_identity.device_id, peer_device_id.clone()),
        DeviceRole::Secondary => (peer_device_id.clone(), local_identity.device_id),
    };
    let task_id = Uuid::parse_str(&invite.task_id).map_err(|e| e.to_string())?;
    let task = SyncTask {
        id: task_id,
        name: invite.task_name,
        primary_device_id,
        secondary_device_id,
        local_path,
        remote_path: invite.requester_path.unwrap_or_default(),
        local_role,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };
    let db = state.db.lock().map_err(|e| e.to_string())?;
    if !invite.requester_public_key.is_empty() {
        state.connections.pin_peer(pairing::PublicIdentity {
            device_id: peer_device_id.clone(),
            public_key: invite.requester_public_key.clone(),
        });
        repository::PairedDeviceRepository::new(&db)
            .upsert(&PairedDevice {
                device_id: peer_device_id.clone(),
                display_name: invite.requester_device_id.clone(),
                public_key: invite.requester_public_key,
                last_seen_unix_ms: now_ms(),
                trusted: true,
            })
            .map_err(|e| e.to_string())?;
    }
    if let Some(address) = requester_address {
        state
            .connections
            .register_connection(connection::PeerConnection {
                device_id: peer_device_id,
                address,
                connected: true,
                last_seen_unix_ms: now_ms(),
            });
    }
    repository::SyncTaskRepository::new(&db)
        .insert(&task)
        .map_err(|e| e.to_string())?;
    Ok(task)
}

#[tauri::command]
pub fn reject_task_invite(
    state: State<'_, AppState>,
    invite_id: String,
    reason: Option<String>,
) -> Result<(), String> {
    let server = state._server.as_ref().ok_or("sync server is not running")?;
    server
        .reject_task_invite(&invite_id, reason.as_deref().unwrap_or("rejected by peer"))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn create_sync_task(
    state: State<'_, AppState>,
    request: CreateTaskRequest,
) -> Result<SyncTask, String> {
    let local_role = parse_device_role(&request.local_role);

    let identity = state.identity.public();
    // Verify peer is pinned before creating task
    state
        .connections
        .get_pinned(&request.peer_device_id)
        .ok_or("peer device not pinned")?;

    let peer_device_id = request.peer_device_id.clone();
    let (primary_id, secondary_id) = match local_role {
        DeviceRole::Primary => (identity.device_id, peer_device_id.clone()),
        DeviceRole::Secondary => (peer_device_id.clone(), identity.device_id),
    };

    let task = SyncTask {
        id: Uuid::new_v4(),
        name: request.name,
        primary_device_id: primary_id,
        secondary_device_id: secondary_id,
        local_path: request.local_path,
        remote_path: request.remote_path.unwrap_or_default().trim().to_string(),
        local_role,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };

    if let Some(server) = &state._server {
        server
            .register_task_root(task.id.to_string(), &task.local_path)
            .map_err(|e| e.to_string())?;
    }

    let mut task = task;
    if task.remote_path.is_empty() {
        let proposed_role = match task.local_role {
            DeviceRole::Primary => "Secondary".to_string(),
            DeviceRole::Secondary => "Primary".to_string(),
        };
        match send_task_invite_to_peer(
            &state,
            &peer_device_id,
            Uuid::new_v4().to_string(),
            task.id.to_string(),
            task.name.clone(),
            Some(task.local_path.clone()),
            proposed_role,
        )
        .await
        {
            Ok(SyncMessage::TaskInviteAck {
                success: true,
                remote_path: Some(remote_path),
                ..
            }) => task.remote_path = remote_path,
            Ok(SyncMessage::TaskInvitePending { .. }) => {
                return Err("task invitation is waiting for peer approval".to_string())
            }
            Ok(SyncMessage::TaskInviteAck { error, .. }) => {
                return Err(error.unwrap_or_else(|| "peer rejected task invitation".to_string()))
            }
            Ok(other) => return Err(format!("unexpected peer response: {:?}", other)),
            Err(e) => return Err(format!("task invitation failed: {}", e)),
        }
    } else {
        let register_msg = SyncMessage::TaskRegister {
            task_id: task.id.to_string(),
            root_path: task.remote_path.clone(),
        };
        match connection::send_authenticated_message_to_peer(
            &state.connections,
            &state.identity,
            &peer_device_id,
            register_msg,
        )
        .await
        {
            Ok(SyncMessage::TaskAck { success: true, .. }) => {}
            Ok(SyncMessage::TaskAck { error, .. }) => {
                return Err(error.unwrap_or_else(|| "peer rejected task registration".to_string()))
            }
            Ok(other) => return Err(format!("unexpected peer response: {:?}", other)),
            Err(e) => return Err(format!("task registration failed: {}", e)),
        }
    }

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::SyncTaskRepository::new(&db);
    repo.insert(&task).map_err(|e| e.to_string())?;
    Ok(task)
}

async fn send_task_invite_to_peer(
    state: &State<'_, AppState>,
    peer_device_id: &str,
    invite_id: String,
    task_id: String,
    task_name: String,
    requester_path: Option<String>,
    proposed_role: String,
) -> Result<SyncMessage, String> {
    let requester_port = state._server.as_ref().map_or(0, |server| server.port());
    let authenticated_msg = SyncMessage::TaskInvite {
        invite_id: invite_id.clone(),
        task_id: task_id.clone(),
        task_name: task_name.clone(),
        requester_port,
        requester_path: requester_path.clone(),
        proposed_role: proposed_role.clone(),
    };

    match connection::send_authenticated_message_to_peer(
        &state.connections,
        &state.identity,
        peer_device_id,
        authenticated_msg,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(auth_error) => {
            let local_public = state.identity.public();
            connection::send_message_to_peer(
                &state.connections,
                peer_device_id,
                SyncMessage::TaskInviteProposal {
                    invite_id,
                    task_id,
                    task_name,
                    requester_device_id: local_public.device_id,
                    requester_public_key: local_public.public_key,
                    requester_port,
                    requester_path,
                    proposed_role,
                },
            )
            .await
            .map_err(|proposal_error| {
                format!(
                    "task invitation failed: {}; proposal fallback failed: {}",
                    auth_error, proposal_error
                )
            })
        }
    }
}

fn parse_device_role(role: &str) -> DeviceRole {
    match role {
        "Primary" => DeviceRole::Primary,
        _ => DeviceRole::Secondary,
    }
}

fn create_task_from_invite(
    state: &State<'_, AppState>,
    pending: &PendingOutgoingTaskInvite,
    remote_path: String,
) -> Result<SyncTask, String> {
    let identity = state.identity.public();
    let (primary_device_id, secondary_device_id) = match pending.local_role {
        DeviceRole::Primary => (identity.device_id, pending.peer_device_id.clone()),
        DeviceRole::Secondary => (pending.peer_device_id.clone(), identity.device_id),
    };
    let task = SyncTask {
        id: pending.task_id,
        name: pending.name.clone(),
        primary_device_id,
        secondary_device_id,
        local_path: pending.local_path.clone(),
        remote_path,
        local_role: pending.local_role,
        enabled: true,
        created_unix_ms: now_ms(),
        updated_unix_ms: now_ms(),
    };

    if let Some(server) = &state._server {
        server
            .register_task_root(task.id.to_string(), &task.local_path)
            .map_err(|e| e.to_string())?;
    }

    let db = state.db.lock().map_err(|e| e.to_string())?;
    repository::SyncTaskRepository::new(&db)
        .insert(&task)
        .map_err(|e| e.to_string())?;
    Ok(task)
}

#[tauri::command]
pub fn list_sync_tasks(state: State<'_, AppState>) -> Result<Vec<SyncTask>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::SyncTaskRepository::new(&db);
    repo.list_all().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_sync_task(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Option<SyncTask>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::SyncTaskRepository::new(&db);
    repo.get(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_task_enabled(
    state: State<'_, AppState>,
    task_id: String,
    enabled: bool,
) -> Result<(), String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::SyncTaskRepository::new(&db);
    repo.update_enabled(&id, enabled, now_ms())
        .map_err(|e| e.to_string())
}

// ─── Scan ───

#[tauri::command]
pub fn scan_task(state: State<'_, AppState>, task_id: String) -> Result<Vec<FileSnapshot>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let results = scanner::scan_root(sync_root, &*state.platform).map_err(|e| e.to_string())?;

    let mut snapshots = Vec::new();
    let snap_repo = repository::FileSnapshotRepository::new(&db);
    for result in &results {
        let mut snap = result.snapshot.clone();
        snap.task_id = id;
        snapshots.push(snap);
    }
    snap_repo
        .replace_for_task(&id, &snapshots)
        .map_err(|e| e.to_string())?;

    Ok(snapshots)
}

// ─── Sync ───

#[derive(Debug, Clone, Serialize)]
pub struct SyncActionResult {
    pub relative_path: String,
    pub success: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn sync_now(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<SyncActionResult>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let (task, actions, snapshots, baselines) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;

        let task_repo = repository::SyncTaskRepository::new(&db);
        let task = task_repo
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?;

        let sync_root = Path::new(&task.local_path);

        // Scan the current filesystem state so deletes are visible even when the
        // UI did not call scan_task immediately before sync_now.
        let scan_results =
            scanner::scan_root(sync_root, &*state.platform).map_err(|e| e.to_string())?;
        let mut snapshots = Vec::new();
        let snap_repo = repository::FileSnapshotRepository::new(&db);
        for result in &scan_results {
            let mut snap = result.snapshot.clone();
            snap.task_id = id;
            snapshots.push(snap);
        }
        snap_repo
            .replace_for_task(&id, &snapshots)
            .map_err(|e| e.to_string())?;

        // Get all baselines so files missing from current snapshots become delete actions.
        let baseline_repo = repository::SyncBaselineRepository::new(&db);
        let baselines = baseline_repo.list_by_task(&id).map_err(|e| e.to_string())?;

        let actions = planner::plan_sync(&snapshots, &baselines, task.local_role);
        (task, actions, snapshots, baselines)
    };

    let results = if task.local_role == DeviceRole::Primary {
        let connections = state.connections.clone();
        let sync_root = Path::new(&task.local_path);
        let remote_scan = connection::request_authenticated_scan(
            &connections,
            &state.identity,
            &task.secondary_device_id,
            task.id.to_string(),
        )
        .await;
        let mut results = match remote_scan {
            Ok(remote_files) => {
                execute_primary_actions_over_network(
                    &actions,
                    &task,
                    sync_root,
                    &connections,
                    &state.identity,
                    &remote_files,
                )
                .await
            }
            Err(e) => actions
                .iter()
                .map(|action| {
                    network_error(
                        &action.relative_path,
                        &format!("remote scan failed: {}", e),
                        true,
                    )
                })
                .collect(),
        };
        let db = state.db.lock().map_err(|e| e.to_string())?;
        persist_network_successes(&actions, &task, &mut results, &db);
        results
    } else {
        let connections = state.connections.clone();
        let sync_root = Path::new(&task.local_path);
        match connection::request_authenticated_scan(
            &connections,
            &state.identity,
            &task.primary_device_id,
            task.id.to_string(),
        )
        .await
        {
            Ok(remote_files) => {
                let mut local_results = {
                    let db = state.db.lock().map_err(|e| e.to_string())?;
                    executor::execute_actions(&actions, &task, sync_root, &db)
                };
                let mut results = execute_secondary_pull_over_network(
                    &task,
                    sync_root,
                    &connections,
                    &state.identity,
                    &remote_files,
                    &snapshots,
                    &baselines,
                )
                .await;
                let db = state.db.lock().map_err(|e| e.to_string())?;
                persist_secondary_pull_successes(&task, &mut results, sync_root, &db);
                local_results.extend(results);
                let results = local_results;
                results
            }
            Err(_) => {
                let db = state.db.lock().map_err(|e| e.to_string())?;
                executor::execute_actions(&actions, &task, sync_root, &db)
            }
        }
    };

    Ok(results
        .into_iter()
        .map(|r| SyncActionResult {
            relative_path: r.relative_path,
            success: r.success,
            error: r.error,
        })
        .collect())
}

async fn execute_primary_actions_over_network(
    actions: &[planner::PlannedAction],
    task: &SyncTask,
    sync_root: &Path,
    connections: &connection::ConnectionManager,
    local_identity: &pairing::DeviceIdentity,
    remote_files: &[RemoteFileState],
) -> Vec<executor::ExecutionResult> {
    let remote_map = remote_files
        .iter()
        .map(|file| (file.relative_path.as_str(), file))
        .collect::<HashMap<_, _>>();
    let mut results = Vec::new();
    for action in actions {
        let result = match action.decision {
            SyncDecision::ApplyToSecondary => match remote_conflict(action, &remote_map) {
                Some(error) => executor::ExecutionResult {
                    relative_path: action.relative_path.clone(),
                    success: false,
                    error: Some(error),
                    retryable: false,
                },
                None => {
                    send_file_action(action, task, sync_root, connections, local_identity).await
                }
            },
            SyncDecision::MoveSecondaryToHistory => {
                send_delete_action(action, task, connections, local_identity).await
            }
            SyncDecision::RequireConflictDecision => executor::ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: false,
                error: Some("conflict requires user decision".to_string()),
                retryable: false,
            },
            SyncDecision::KeepBoth | SyncDecision::MarkPendingReturn => executor::ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: false,
                error: Some("unsupported network action for primary sync".to_string()),
                retryable: false,
            },
            SyncDecision::Noop => executor::ExecutionResult {
                relative_path: action.relative_path.clone(),
                success: true,
                error: None,
                retryable: false,
            },
        };
        results.push(result);
    }
    results
}

fn remote_conflict(
    action: &planner::PlannedAction,
    remote_map: &HashMap<&str, &RemoteFileState>,
) -> Option<String> {
    let remote = remote_map.get(action.relative_path.as_str())?;
    match &action.baseline {
        Some(baseline) if baseline.secondary_hash == remote.blake3_hash => None,
        Some(_) => Some("remote file changed since last sync".to_string()),
        None => Some("remote file already exists".to_string()),
    }
}

async fn send_file_action(
    action: &planner::PlannedAction,
    task: &SyncTask,
    sync_root: &Path,
    connections: &connection::ConnectionManager,
    local_identity: &pairing::DeviceIdentity,
) -> executor::ExecutionResult {
    if action.snapshot.is_none() {
        return network_error(&action.relative_path, "no snapshot for apply action", false);
    }

    let source = sync_root.join(&action.relative_path);
    match send_file_with_retry(
        connections,
        local_identity,
        &task.secondary_device_id,
        task.id.to_string(),
        action.relative_path.clone(),
        &source,
    )
    .await
    {
        Ok(()) => executor::ExecutionResult {
            relative_path: action.relative_path.clone(),
            success: true,
            error: None,
            retryable: false,
        },
        Err(e) => {
            return network_error(
                &action.relative_path,
                &format!("network file transfer failed: {}", e),
                true,
            )
        }
    }
}

async fn send_file_with_retry(
    connections: &connection::ConnectionManager,
    local_identity: &pairing::DeviceIdentity,
    peer_device_id: &str,
    task_id: String,
    relative_path: String,
    source: &Path,
) -> anyhow::Result<()> {
    let mut last_error = None;
    for attempt in 0..3 {
        match connection::send_authenticated_file_to_peer(
            connections,
            local_identity,
            peer_device_id,
            task_id.clone(),
            relative_path.clone(),
            source,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_error = Some(e);
                if attempt < 2 {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        150 * (attempt + 1) as u64,
                    ))
                    .await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("network file transfer failed")))
}

async fn send_delete_action(
    action: &planner::PlannedAction,
    task: &SyncTask,
    connections: &connection::ConnectionManager,
    local_identity: &pairing::DeviceIdentity,
) -> executor::ExecutionResult {
    let msg = SyncMessage::FileDelete {
        task_id: task.id.to_string(),
        relative_path: action.relative_path.clone(),
    };
    expect_file_ack(
        &task.secondary_device_id,
        &action.relative_path,
        connections,
        local_identity,
        msg,
    )
    .await
}

async fn expect_file_ack(
    peer_device_id: &str,
    relative_path: &str,
    connections: &connection::ConnectionManager,
    local_identity: &pairing::DeviceIdentity,
    msg: SyncMessage,
) -> executor::ExecutionResult {
    match connection::send_authenticated_message_to_peer(
        connections,
        local_identity,
        peer_device_id,
        msg,
    )
    .await
    {
        Ok(SyncMessage::FileAck { success, .. }) if success => executor::ExecutionResult {
            relative_path: relative_path.to_string(),
            success: true,
            error: None,
            retryable: false,
        },
        Ok(SyncMessage::FileAck { error, .. }) => executor::ExecutionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some(error.unwrap_or_else(|| "peer rejected file operation".to_string())),
            retryable: true,
        },
        Ok(other) => executor::ExecutionResult {
            relative_path: relative_path.to_string(),
            success: false,
            error: Some(format!("unexpected peer response: {:?}", other)),
            retryable: true,
        },
        Err(e) => network_error(relative_path, &format!("network send failed: {}", e), true),
    }
}

fn persist_network_successes(
    actions: &[planner::PlannedAction],
    task: &SyncTask,
    results: &mut [executor::ExecutionResult],
    db: &rusqlite::Connection,
) {
    let now = now_ms();
    let baseline_repo = repository::SyncBaselineRepository::new(db);
    let snap_repo = repository::FileSnapshotRepository::new(db);

    for (action, result) in actions.iter().zip(results.iter_mut()) {
        if !result.success {
            continue;
        }
        match action.decision {
            SyncDecision::ApplyToSecondary => {
                let Some(snap) = &action.snapshot else {
                    continue;
                };
                let baseline = SyncBaseline {
                    task_id: task.id,
                    relative_path: action.relative_path.clone(),
                    primary_hash: snap.blake3_hash.clone(),
                    primary_hash_status: snap.hash_status,
                    primary_size: snap.size,
                    primary_modified_unix_ms: snap.modified_unix_ms,
                    secondary_hash: snap.blake3_hash.clone(),
                    secondary_hash_status: snap.hash_status,
                    secondary_modified_unix_ms: snap.modified_unix_ms,
                    last_synced_unix_ms: now,
                };
                if let Err(e) = baseline_repo.upsert(&baseline) {
                    result.success = false;
                    result.error = Some(format!("baseline update failed: {}", e));
                    result.retryable = true;
                }
            }
            SyncDecision::MoveSecondaryToHistory => {
                let _ = snap_repo.mark_deleted(&task.id, &action.relative_path);
            }
            _ => {}
        }
    }
}

async fn execute_secondary_pull_over_network(
    task: &SyncTask,
    sync_root: &Path,
    connections: &connection::ConnectionManager,
    local_identity: &pairing::DeviceIdentity,
    remote_files: &[RemoteFileState],
    local_snapshots: &[FileSnapshot],
    baselines: &[SyncBaseline],
) -> Vec<executor::ExecutionResult> {
    let local_map = local_snapshots
        .iter()
        .map(|snapshot| (snapshot.relative_path.as_str(), snapshot))
        .collect::<HashMap<_, _>>();
    let baseline_map = baselines
        .iter()
        .map(|baseline| (baseline.relative_path.as_str(), baseline))
        .collect::<HashMap<_, _>>();
    let mut results = Vec::new();

    for remote in remote_files {
        if !secondary_should_download(remote, &local_map, &baseline_map) {
            continue;
        }
        if let Some(error) = secondary_pull_conflict(remote, &local_map, &baseline_map) {
            results.push(network_error(&remote.relative_path, &error, false));
            continue;
        }

        let target = sync_root.join(&remote.relative_path);
        match connection::request_authenticated_file_from_peer(
            connections,
            local_identity,
            &task.primary_device_id,
            task.id.to_string(),
            remote.relative_path.clone(),
            &target,
        )
        .await
        {
            Ok(()) => results.push(executor::ExecutionResult {
                relative_path: remote.relative_path.clone(),
                success: true,
                error: None,
                retryable: false,
            }),
            Err(e) => results.push(network_error(
                &remote.relative_path,
                &format!("network file download failed: {}", e),
                true,
            )),
        }
    }

    results
}

fn secondary_should_download(
    remote: &RemoteFileState,
    local_map: &HashMap<&str, &FileSnapshot>,
    baseline_map: &HashMap<&str, &SyncBaseline>,
) -> bool {
    match baseline_map.get(remote.relative_path.as_str()) {
        Some(baseline) if baseline.primary_hash == remote.blake3_hash => false,
        _ => match local_map.get(remote.relative_path.as_str()) {
            Some(local) => local.blake3_hash != remote.blake3_hash,
            None => true,
        },
    }
}

fn secondary_pull_conflict(
    remote: &RemoteFileState,
    local_map: &HashMap<&str, &FileSnapshot>,
    baseline_map: &HashMap<&str, &SyncBaseline>,
) -> Option<String> {
    let local = local_map.get(remote.relative_path.as_str())?;
    match baseline_map.get(remote.relative_path.as_str()) {
        Some(baseline) if local.blake3_hash == baseline.secondary_hash => None,
        Some(_) => Some("local file changed since last sync".to_string()),
        None => Some("local file already exists".to_string()),
    }
}

fn persist_secondary_pull_successes(
    task: &SyncTask,
    results: &mut [executor::ExecutionResult],
    sync_root: &Path,
    db: &rusqlite::Connection,
) {
    let now = now_ms();
    let snap_repo = repository::FileSnapshotRepository::new(db);
    let baseline_repo = repository::SyncBaselineRepository::new(db);

    for result in results {
        if !result.success {
            continue;
        }
        let path = sync_root.join(&result.relative_path);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) => {
                result.success = false;
                result.error = Some(format!("pulled file metadata failed: {}", e));
                result.retryable = true;
                continue;
            }
        };
        let file_hash = match crate::core::scanner::hash_file(&path) {
            Ok(hash) => hash,
            Err(e) => {
                result.success = false;
                result.error = Some(format!("pulled file hash failed: {}", e));
                result.retryable = true;
                continue;
            }
        };
        let modified_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(now);
        let snapshot = FileSnapshot {
            task_id: task.id,
            relative_path: result.relative_path.clone(),
            kind: EntryKind::File,
            size: metadata.len() as i64,
            modified_unix_ms,
            blake3_hash: Some(file_hash.clone()),
            hash_status: HashStatus::Verified,
            deleted: false,
            is_symlink: false,
        };
        if let Err(e) = snap_repo.upsert(&snapshot) {
            result.success = false;
            result.error = Some(format!("pulled snapshot update failed: {}", e));
            result.retryable = true;
            continue;
        }
        if let Err(e) = baseline_repo.upsert(&SyncBaseline {
            task_id: task.id,
            relative_path: result.relative_path.clone(),
            primary_hash: Some(file_hash.clone()),
            primary_hash_status: HashStatus::Verified,
            primary_size: metadata.len() as i64,
            primary_modified_unix_ms: modified_unix_ms,
            secondary_hash: Some(file_hash),
            secondary_hash_status: HashStatus::Verified,
            secondary_modified_unix_ms: modified_unix_ms,
            last_synced_unix_ms: now,
        }) {
            result.success = false;
            result.error = Some(format!("pulled baseline update failed: {}", e));
            result.retryable = true;
        }
    }
}

fn network_error(relative_path: &str, error: &str, retryable: bool) -> executor::ExecutionResult {
    executor::ExecutionResult {
        relative_path: relative_path.to_string(),
        success: false,
        error: Some(error.to_string()),
        retryable,
    }
}

// ─── Pending Return Changes ───

#[tauri::command]
pub fn list_pending_returns(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<PendingReturnChange>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::PendingReturnRepository::new(&db);
    repo.list_by_task(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_pending_count(state: State<'_, AppState>, task_id: String) -> Result<i64, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::PendingReturnRepository::new(&db);
    repo.count_by_task(&id).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct ReturnSyncResult {
    pub relative_path: String,
    pub success: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn execute_return_sync(
    state: State<'_, AppState>,
    task_id: String,
    selected_paths: Vec<String>,
) -> Result<Vec<ReturnSyncResult>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let task = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let task_repo = repository::SyncTaskRepository::new(&db);
        task_repo
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or("task not found")?
    };

    let results = if task.local_role == DeviceRole::Secondary {
        let pending = {
            let db = state.db.lock().map_err(|e| e.to_string())?;
            let pending_repo = repository::PendingReturnRepository::new(&db);
            pending_repo.list_by_task(&id).map_err(|e| e.to_string())?
        };
        let pending_map: HashMap<String, PendingReturnChange> = pending
            .into_iter()
            .map(|change| (change.relative_path.clone(), change))
            .collect();
        let connections = state.connections.clone();
        let sync_root = Path::new(&task.local_path);
        let mut results = execute_secondary_return_over_network(
            &task,
            &selected_paths,
            &pending_map,
            sync_root,
            &connections,
            &state.identity,
        )
        .await;
        let db = state.db.lock().map_err(|e| e.to_string())?;
        persist_return_successes(&task, &pending_map, &mut results, sync_root, &db);
        results
    } else {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let sync_root = std::path::Path::new(&task.local_path);

        // Get current primary snapshots for conflict detection
        let snap_repo = repository::FileSnapshotRepository::new(&db);
        let all_snaps = snap_repo.list_by_task(&id).map_err(|e| e.to_string())?;
        let primary_map: HashMap<String, FileSnapshot> = all_snaps
            .into_iter()
            .map(|s| (s.relative_path.clone(), s))
            .collect();

        // Get baselines
        let baseline_repo = repository::SyncBaselineRepository::new(&db);
        let mut baseline_map = HashMap::new();
        for path in &selected_paths {
            if let Some(b) = baseline_repo.get(&id, path).map_err(|e| e.to_string())? {
                baseline_map.insert(path.clone(), b);
            }
        }

        executor::execute_return_sync(
            &task,
            &selected_paths,
            &primary_map,
            &baseline_map,
            sync_root,
            &db,
        )
    };

    Ok(results
        .into_iter()
        .map(|r| ReturnSyncResult {
            relative_path: r.relative_path,
            success: r.success,
            error: r.error,
        })
        .collect())
}

async fn execute_secondary_return_over_network(
    task: &SyncTask,
    selected_paths: &[String],
    pending: &HashMap<String, PendingReturnChange>,
    sync_root: &Path,
    connections: &connection::ConnectionManager,
    local_identity: &pairing::DeviceIdentity,
) -> Vec<executor::ExecutionResult> {
    let mut results = Vec::new();
    for path in selected_paths {
        if !pending.contains_key(path) {
            results.push(network_error(
                path,
                "pending change not found in database",
                false,
            ));
            continue;
        }

        let source = sync_root.join(path);
        match send_file_with_retry(
            connections,
            local_identity,
            &task.primary_device_id,
            task.id.to_string(),
            path.clone(),
            &source,
        )
        .await
        {
            Ok(()) => results.push(executor::ExecutionResult {
                relative_path: path.clone(),
                success: true,
                error: None,
                retryable: false,
            }),
            Err(e) => results.push(network_error(
                path,
                &format!("network file transfer failed: {}", e),
                true,
            )),
        }
    }
    results
}

fn persist_return_successes(
    task: &SyncTask,
    pending: &HashMap<String, PendingReturnChange>,
    results: &mut [executor::ExecutionResult],
    sync_root: &Path,
    db: &rusqlite::Connection,
) {
    let now = now_ms();
    let baseline_repo = repository::SyncBaselineRepository::new(db);
    let pending_repo = repository::PendingReturnRepository::new(db);

    for result in results {
        if !result.success {
            continue;
        }
        let Some(change) = pending.get(&result.relative_path) else {
            continue;
        };
        let primary_size = std::fs::metadata(sync_root.join(&result.relative_path))
            .map(|metadata| metadata.len() as i64)
            .unwrap_or(0);
        let baseline = SyncBaseline {
            task_id: task.id,
            relative_path: result.relative_path.clone(),
            primary_hash: change.secondary_hash.clone(),
            primary_hash_status: change.secondary_hash_status,
            primary_size,
            primary_modified_unix_ms: change.secondary_modified_unix_ms,
            secondary_hash: change.secondary_hash.clone(),
            secondary_hash_status: change.secondary_hash_status,
            secondary_modified_unix_ms: change.secondary_modified_unix_ms,
            last_synced_unix_ms: now,
        };
        if let Err(e) = baseline_repo.upsert(&baseline) {
            result.success = false;
            result.error = Some(format!("return-sync baseline update failed: {}", e));
            result.retryable = true;
            continue;
        }
        if let Err(e) = pending_repo.remove(&task.id, &result.relative_path) {
            result.success = false;
            result.error = Some(format!("remove pending change failed: {}", e));
            result.retryable = true;
        }
    }
}

// ─── Conflict ───

#[derive(Debug, Clone, Serialize)]
pub struct ConflictInfo {
    pub relative_path: String,
    pub primary_hash: Option<String>,
    pub primary_modified_unix_ms: i64,
    pub secondary_hash: Option<String>,
    pub secondary_modified_unix_ms: i64,
    pub hash_unverified: bool,
}

#[tauri::command]
pub fn detect_conflicts(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<ConflictInfo>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    let pending_repo = repository::PendingReturnRepository::new(&db);
    let pending_list = pending_repo.list_by_task(&id).map_err(|e| e.to_string())?;

    let snap_repo = repository::FileSnapshotRepository::new(&db);
    let baseline_repo = repository::SyncBaselineRepository::new(&db);

    let mut conflicts = Vec::new();
    for pending in &pending_list {
        let current_primary = snap_repo
            .get(&id, &pending.relative_path)
            .map_err(|e| e.to_string())?;
        let baseline = baseline_repo
            .get(&id, &pending.relative_path)
            .map_err(|e| e.to_string())?;

        match conflict::detect_conflict(pending, current_primary.as_ref(), baseline.as_ref()) {
            conflict::ConflictResult::Conflict {
                relative_path,
                primary_hash,
                primary_hash_status: _,
                primary_modified_unix_ms,
                secondary_hash,
                secondary_hash_status: _,
                secondary_modified_unix_ms,
                hash_unverified,
            } => {
                conflicts.push(ConflictInfo {
                    relative_path,
                    primary_hash,
                    primary_modified_unix_ms,
                    secondary_hash,
                    secondary_modified_unix_ms,
                    hash_unverified,
                });
            }
            _ => {}
        }
    }

    Ok(conflicts)
}

#[tauri::command]
pub fn resolve_conflict_overwrite(
    state: State<'_, AppState>,
    task_id: String,
    relative_path: String,
) -> Result<SyncActionResult, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let result = executor::execute_confirmed_overwrite(&task, &relative_path, sync_root, &db);

    Ok(SyncActionResult {
        relative_path: result.relative_path,
        success: result.success,
        error: result.error,
    })
}

#[tauri::command]
pub fn resolve_conflict_keep_both(
    state: State<'_, AppState>,
    task_id: String,
    relative_path: String,
) -> Result<SyncActionResult, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let result = executor::execute_conflict_keep_both(&task, &relative_path, sync_root, &db);

    Ok(SyncActionResult {
        relative_path: result.relative_path,
        success: result.success,
        error: result.error,
    })
}

// ─── History ───

#[tauri::command]
pub fn list_history(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<HistoryEntry>, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::HistoryRepository::new(&db);
    repo.list_by_task(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn restore_history_entry(
    state: State<'_, AppState>,
    task_id: String,
    entry_id: String,
) -> Result<String, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let eid = Uuid::parse_str(&entry_id).map_err(|e| e.to_string())?;

    let db = state.db.lock().map_err(|e| e.to_string())?;
    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let history_repo = repository::HistoryRepository::new(&db);
    let entries = history_repo.list_by_task(&id).map_err(|e| e.to_string())?;
    let entry = entries
        .into_iter()
        .find(|e| e.id == eid)
        .ok_or("history entry not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let store = HistoryStore::new(sync_root);
    let restored = store
        .restore(&entry, sync_root, now_ms())
        .map_err(|e| e.to_string())?;

    Ok(restored.to_string_lossy().to_string())
}

#[tauri::command]
pub fn cleanup_history(state: State<'_, AppState>, task_id: String) -> Result<usize, String> {
    let id = Uuid::parse_str(&task_id).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    let task_repo = repository::SyncTaskRepository::new(&db);
    let task = task_repo
        .get(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task not found")?;

    let sync_root = std::path::Path::new(&task.local_path);
    let store = HistoryStore::new(sync_root);
    let cutoff = now_ms() - crate::history::store::DEFAULT_RETENTION_DAYS_MS;
    let deleted = store
        .cleanup_old_entries(cutoff)
        .map_err(|e| e.to_string())?;

    // Also clean up database entries
    let history_repo = repository::HistoryRepository::new(&db);
    history_repo
        .delete_older_than(&id, cutoff)
        .map_err(|e| e.to_string())?;

    Ok(deleted)
}

// ─── Logs ───

#[tauri::command]
pub fn list_logs(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<LogEntry>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::LogRepository::new(&db);
    repo.list_recent(limit.unwrap_or(100))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn write_log(
    state: State<'_, AppState>,
    level: String,
    message: String,
    task_id: Option<String>,
    relative_path: Option<String>,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let repo = repository::LogRepository::new(&db);

    let lvl = match level.as_str() {
        "Warn" => LogLevel::Warn,
        "Error" => LogLevel::Error,
        _ => LogLevel::Info,
    };

    let entry = LogEntry {
        id: None,
        level: lvl,
        task_id: task_id.and_then(|s| Uuid::parse_str(&s).ok()),
        relative_path,
        message,
        created_unix_ms: now_ms(),
    };

    repo.insert(&entry).map_err(|e| e.to_string())?;

    // Enforce retention
    repo.enforce_retention(10_000, now_ms() - 7 * 24 * 60 * 60 * 1000)
        .map_err(|e| e.to_string())?;

    Ok(())
}

// ─── Settings ───

#[derive(Debug, Clone, Serialize)]
pub struct AppSettings {
    pub history_retention_days: i64,
    pub history_size_limit_mb: i64,
}

#[tauri::command]
pub fn get_settings() -> Result<AppSettings, String> {
    Ok(AppSettings {
        history_retention_days: 30,
        history_size_limit_mb: 1024,
    })
}

// Helper for hex encoding/decoding
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
