use anyhow::Result;
use notify::RecommendedWatcher;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::core::model::{DeviceRole, SyncTask};
use crate::pairing::DeviceIdentity;
use crate::platform::traits::{Platform, PlatformWatcherEvent};
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
    pub dirty_tasks: TaskDirtyTracker,
    /// File watchers kept alive for the lifetime of the app.
    /// Each watcher monitors one task's sync root.
    /// Event receivers are consumed by background dirty-marker threads.
    pub _watchers: Mutex<Vec<(String, RecommendedWatcher)>>,
}

const WATCHER_DEBOUNCE: Duration = Duration::from_millis(2_500);

#[derive(Debug, Clone)]
pub struct TaskDirtyTracker {
    tasks: Arc<Mutex<HashMap<Uuid, TaskDirtyState>>>,
    debounce: Duration,
}

#[derive(Debug)]
struct TaskDirtyState {
    dirty_paths: HashSet<PathBuf>,
    last_event_at: Instant,
    sync_scheduled: bool,
}

impl Default for TaskDirtyTracker {
    fn default() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            debounce: WATCHER_DEBOUNCE,
        }
    }
}

impl TaskDirtyTracker {
    pub fn mark_task_dirty(&self, task_id: Uuid) {
        self.mark_dirty_paths_at(task_id, Vec::new(), Instant::now());
    }

    pub fn mark_dirty_paths(&self, task_id: Uuid, paths: Vec<PathBuf>) {
        self.mark_dirty_paths_at(task_id, paths, Instant::now());
    }

    fn mark_dirty_paths_at(&self, task_id: Uuid, paths: Vec<PathBuf>, now: Instant) {
        let mut tasks = self.tasks.lock().unwrap();
        let state = tasks.entry(task_id).or_insert_with(|| TaskDirtyState {
            dirty_paths: HashSet::new(),
            last_event_at: now,
            sync_scheduled: false,
        });
        state.dirty_paths.extend(paths);
        state.last_event_at = now;
        state.sync_scheduled = true;
    }

    pub fn ready_task_ids(&self) -> Vec<Uuid> {
        self.ready_task_ids_at(Instant::now())
    }

    fn ready_task_ids_at(&self, now: Instant) -> Vec<Uuid> {
        let tasks = self.tasks.lock().unwrap();
        tasks
            .iter()
            .filter_map(|(task_id, state)| {
                if state.sync_scheduled && now.duration_since(state.last_event_at) >= self.debounce
                {
                    Some(*task_id)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn clear(&self, task_id: Uuid) {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.remove(&task_id);
    }
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

    #[test]
    fn dirty_tracker_debounces_watcher_events() {
        let tracker = TaskDirtyTracker::default();
        let task_id = Uuid::new_v4();
        let now = Instant::now();

        tracker.mark_dirty_paths_at(task_id, vec![PathBuf::from("file.txt")], now);
        assert!(tracker
            .ready_task_ids_at(now + Duration::from_millis(500))
            .is_empty());
        assert_eq!(
            tracker.ready_task_ids_at(now + WATCHER_DEBOUNCE + Duration::from_millis(1)),
            vec![task_id]
        );
        tracker.clear(task_id);
        assert!(tracker
            .ready_task_ids_at(now + WATCHER_DEBOUNCE + Duration::from_millis(2))
            .is_empty());
    }

    #[test]
    fn watcher_filter_ignores_protocol_internal_paths() {
        let root = PathBuf::from("/tmp/lanbridge-task");
        let paths = vec![
            root.join(".lanbridge-history")
                .join("trash")
                .join("1")
                .join("a.txt"),
            root.join(".lanbridge-temp").join("upload.tmp"),
            root.join("folder").join("keep.txt"),
        ];

        assert_eq!(
            filter_external_watcher_paths(&root, paths),
            vec![root.join("folder").join("keep.txt")]
        );
        assert!(filter_external_watcher_paths(
            &root,
            vec![root.join(".lanbridge-history").join("trash").join("1")]
        )
        .is_empty());
    }
}

impl AppState {
    pub fn start_task_watcher(&self, task: &SyncTask) -> Result<()> {
        let task_id = task.id.to_string();
        {
            let watchers = self._watchers.lock().unwrap();
            if watchers.iter().any(|(id, _)| id == &task_id) {
                return Ok(());
            }
        }
        let (watcher, rx) = self.platform.start_watcher(Path::new(&task.local_path))?;
        spawn_dirty_watcher_thread(
            task.id,
            task.name.clone(),
            task.local_path.clone(),
            self.dirty_tasks.clone(),
            rx,
        );
        self._watchers.lock().unwrap().push((task_id, watcher));
        Ok(())
    }

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
        let deferred_repo = crate::state::repository::DeferredTransferRepository::new(&conn);
        for transfer in deferred_repo.list_all()? {
            crate::transport::connection::defer_transfer(
                &transfer.task_id.to_string(),
                &transfer.relative_path,
                &transfer.direction,
            );
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
        let dirty_tasks = TaskDirtyTracker::default();
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
                    spawn_dirty_watcher_thread(
                        task.id,
                        task.name.clone(),
                        task.local_path.clone(),
                        dirty_tasks.clone(),
                        rx,
                    );
                    watchers.push((task.id.to_string(), w));
                }
                Err(e) => {
                    tracing::warn!("failed to start watcher for task '{}': {}", task.name, e);
                }
            }
            if task.enabled && task.local_role == DeviceRole::Primary {
                dirty_tasks.mark_task_dirty(task.id);
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
            dirty_tasks,
            _watchers: Mutex::new(watchers),
        })
    }
}

fn spawn_dirty_watcher_thread(
    task_id: Uuid,
    task_name: String,
    local_path: String,
    dirty_tasks: TaskDirtyTracker,
    rx: std::sync::mpsc::Receiver<PlatformWatcherEvent>,
) {
    let local_path = PathBuf::from(local_path);
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            let paths = filter_external_watcher_paths(&local_path, event.paths);
            if paths.is_empty() {
                continue;
            }
            dirty_tasks.mark_dirty_paths(task_id, paths);
            tracing::debug!(
                task_id = %task_id,
                task_name = %task_name,
                local_path = %local_path.display(),
                "marked task dirty from watcher event"
            );
        }
    });
}

fn filter_external_watcher_paths(local_path: &Path, paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|path| {
            let relative = path.strip_prefix(local_path).unwrap_or(path);
            !crate::core::transient::path_has_protocol_ignored_component(relative)
        })
        .collect()
}
