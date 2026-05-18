use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use crate::platform::traits::PlatformWatcherEvent;

/// Debounce interval for filesystem events (milliseconds).
const DEBOUNCE_MS: u64 = 500;

/// Start watching a directory for filesystem changes.
///
/// On Windows, `notify` uses ReadDirectoryChangesW automatically.
/// Events are debounced: paths are accumulated during the debounce window,
/// then flushed as a single batch after the quiet period.
/// The watcher triggers scan requests, not direct sync decisions.
pub fn start_watcher(
    sync_root: &Path,
) -> Result<(RecommendedWatcher, mpsc::Receiver<PlatformWatcherEvent>)> {
    let (tx, rx) = mpsc::channel();
    let debounce = Duration::from_millis(DEBOUNCE_MS);

    // Shared state for debounce: accumulated paths and last event time
    let pending_paths: std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let last_event_time: std::sync::Arc<std::sync::Mutex<std::time::Instant>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now() - debounce));

    let paths_for_callback = pending_paths.clone();
    let time_for_callback = last_event_time.clone();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Any | EventKind::Other | EventKind::Access(_) => return,
                    _ => {}
                }

                let now = std::time::Instant::now();
                let mut last = time_for_callback.lock().unwrap();
                let mut paths = paths_for_callback.lock().unwrap();

                if now.duration_since(*last) >= debounce {
                    let mut batch = std::mem::take(&mut *paths);
                    batch.extend(event.paths.clone());
                    *last = now;
                    let _ = tx.send(PlatformWatcherEvent { paths: batch });
                } else {
                    paths.extend(event.paths);
                }
            }
        },
        notify::Config::default(),
    )?;

    watcher.watch(sync_root, RecursiveMode::Recursive)?;

    Ok((watcher, rx))
}
