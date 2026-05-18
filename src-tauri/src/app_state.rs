use anyhow::Result;
use notify::RecommendedWatcher;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::core::model::DeviceRole;
use crate::pairing::DeviceIdentity;
use crate::platform::traits::Platform;
use crate::state::db;
use crate::transport::server::SyncServer;
use crate::transport::{ConnectionManager, DiscoveryState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRunAdmission {
    Started,
    Queued,
}

#[derive(Debug, Default)]
pub struct SyncRunCoordinator {
    runs: Mutex<HashMap<Uuid, SyncRunState>>,
}

#[derive(Debug, Default)]
struct SyncRunState {
    running: bool,
    rerun_requested: bool,
}

impl SyncRunCoordinator {
    pub fn begin(&self, task_id: Uuid) -> SyncRunAdmission {
        let mut runs = self.runs.lock().unwrap();
        let state = runs.entry(task_id).or_default();
        if state.running {
            state.rerun_requested = true;
            SyncRunAdmission::Queued
        } else {
            state.running = true;
            SyncRunAdmission::Started
        }
    }

    pub fn finish(&self, task_id: Uuid) -> bool {
        let mut runs = self.runs.lock().unwrap();
        let Some(state) = runs.get_mut(&task_id) else {
            return false;
        };
        if state.rerun_requested {
            state.rerun_requested = false;
            true
        } else {
            runs.remove(&task_id);
            false
        }
    }

    pub fn abort(&self, task_id: Uuid) {
        let mut runs = self.runs.lock().unwrap();
        runs.remove(&task_id);
    }
}

/// Shared application state accessible from Tauri commands.
pub struct AppState {
    pub db: Mutex<Connection>,
    pub identity: DeviceIdentity,
    pub platform: Box<dyn Platform>,
    pub connections: ConnectionManager,
    pub discovery: Arc<DiscoveryState>,
    pub _server: Option<SyncServer>,
    pub pending_outgoing_invites: Mutex<HashMap<String, PendingOutgoingTaskInvite>>,
    pub sync_runs: SyncRunCoordinator,
    /// File watchers kept alive for the lifetime of the app.
    /// Each watcher monitors one task's sync root.
    /// Event receivers are consumed by background drain threads.
    pub _watchers: Mutex<Vec<(String, RecommendedWatcher)>>,
}

#[derive(Debug, Clone)]
pub struct PendingOutgoingTaskInvite {
    pub task_id: Uuid,
    pub name: String,
    pub local_path: String,
    pub peer_device_id: String,
    pub local_role: DeviceRole,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_run_coordinator_queues_second_run_for_same_task() {
        let coordinator = SyncRunCoordinator::default();
        let task_id = Uuid::new_v4();

        assert_eq!(coordinator.begin(task_id), SyncRunAdmission::Started);
        assert_eq!(coordinator.begin(task_id), SyncRunAdmission::Queued);
        assert!(coordinator.finish(task_id));
        assert_eq!(coordinator.begin(task_id), SyncRunAdmission::Queued);
        assert!(coordinator.finish(task_id));
        assert!(!coordinator.finish(task_id));
        assert_eq!(coordinator.begin(task_id), SyncRunAdmission::Started);
    }

    #[test]
    fn sync_run_coordinator_abort_releases_queued_run() {
        let coordinator = SyncRunCoordinator::default();
        let task_id = Uuid::new_v4();

        assert_eq!(coordinator.begin(task_id), SyncRunAdmission::Started);
        assert_eq!(coordinator.begin(task_id), SyncRunAdmission::Queued);
        coordinator.abort(task_id);

        assert_eq!(coordinator.begin(task_id), SyncRunAdmission::Started);
    }
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
                    device_id: peer.device_id.clone(),
                    public_key: peer.public_key,
                };
                connections.pin_peer(identity.clone());
                if let Some(server) = &server {
                    server.register_trusted_peer(identity);
                }
                if let Some(address) = peer.last_address {
                    connections.register_connection(crate::transport::connection::PeerConnection {
                        device_id: peer.device_id,
                        address,
                        connected: true,
                        last_seen_unix_ms: peer.last_seen_unix_ms,
                    });
                }
            }
        }
        let repo = crate::state::repository::SyncTaskRepository::new(&conn);
        let tasks = repo.list_all()?;
        if let Some(server) = &server {
            let active_task_ids = tasks
                .iter()
                .filter(|task| task.enabled)
                .map(|task| task.id.to_string())
                .collect::<HashSet<_>>();
            server.retain_registered_task_roots(&active_task_ids)?;
        }
        let mut watchers: Vec<(String, RecommendedWatcher)> = Vec::new();
        for task in tasks {
            if let Err(error) = crate::core::transient::cleanup_lanbridge_transient_files(
                std::path::Path::new(&task.local_path),
            ) {
                tracing::warn!(
                    "failed to clean transient sync files for '{}': {}",
                    task.local_path,
                    error
                );
            }
            if task.enabled {
                if let Some(server) = &server {
                    server.register_task_root(task.id.to_string(), &task.local_path)?;
                }
            }
            match platform.start_watcher(Path::new(&task.local_path)) {
                Ok((w, rx)) => {
                    tracing::info!(
                        "started watcher for task '{}' at {}",
                        task.name,
                        task.local_path
                    );
                    // Drain events in a background thread; P1 will connect them to scans.
                    std::thread::spawn(move || while rx.recv().is_ok() {});
                    watchers.push((task.id.to_string(), w));
                }
                Err(e) => {
                    tracing::warn!("failed to start watcher for task '{}': {}", task.name, e);
                }
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
            sync_runs: SyncRunCoordinator::default(),
            _watchers: Mutex::new(watchers),
        })
    }
}
