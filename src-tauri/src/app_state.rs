use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::core::model::DeviceRole;
use crate::pairing::DeviceIdentity;
use crate::platform::traits::Platform;
use crate::state::db;
use crate::transport::server::SyncServer;
use crate::transport::{ConnectionManager, DiscoveryState};

/// Shared application state accessible from Tauri commands.
pub struct AppState {
    pub db: Mutex<Connection>,
    pub identity: DeviceIdentity,
    pub platform: Box<dyn Platform>,
    pub connections: ConnectionManager,
    pub discovery: Arc<DiscoveryState>,
    pub _server: Option<SyncServer>,
    pub pending_outgoing_invites: Mutex<HashMap<String, PendingOutgoingTaskInvite>>,
}

#[derive(Debug, Clone)]
pub struct PendingOutgoingTaskInvite {
    pub task_id: Uuid,
    pub name: String,
    pub local_path: String,
    pub peer_device_id: String,
    pub local_role: DeviceRole,
}

impl AppState {
    pub fn new(
        identity: DeviceIdentity,
        platform: Box<dyn Platform>,
        discovery: Arc<DiscoveryState>,
        server: Option<SyncServer>,
    ) -> Result<Self> {
        let db_path = platform.database_path()?;
        let conn = db::open_db(&db_path)?;
        db::migrate(&conn)?;
        let connections = ConnectionManager::new();
        if let Some(server) = &server {
            server.set_local_identity(identity.public());
            let app_data_dir = platform.app_data_dir()?;
            server.set_state_db_path(&db_path)?;
            server.set_task_roots_persistence_path(app_data_dir.join("remote_task_roots.json"))?;
            server.set_task_invites_persistence_path(
                app_data_dir.join("pending_task_invites.json"),
            )?;
            server.set_task_invite_inbox_root(app_data_dir.join("incoming_tasks"))?;
            server.set_auto_accept_task_invites(false);
        }
        let paired_repo = crate::state::repository::PairedDeviceRepository::new(&conn);
        for peer in paired_repo.list_all()? {
            if peer.trusted {
                let identity = crate::pairing::PublicIdentity {
                    device_id: peer.device_id,
                    public_key: peer.public_key,
                };
                connections.pin_peer(identity.clone());
                if let Some(server) = &server {
                    server.register_trusted_peer(identity);
                }
            }
        }
        if let Some(server) = &server {
            let repo = crate::state::repository::SyncTaskRepository::new(&conn);
            for task in repo.list_all()? {
                server.register_task_root(task.id.to_string(), &task.local_path)?;
            }
        }

        Ok(Self {
            db: Mutex::new(conn),
            identity,
            platform,
            connections,
            discovery,
            _server: server,
            pending_outgoing_invites: Mutex::new(HashMap::new()),
        })
    }
}
