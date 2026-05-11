use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher, Event, EventKind};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

/// Debounce interval for filesystem events (milliseconds).
const DEBOUNCE_MS: u64 = 500;

/// Event from the filesystem watcher after debouncing.
#[derive(Debug, Clone)]
pub struct WatcherEvent {
    pub paths: Vec<std::path::PathBuf>,
}

/// Start watching a directory for filesystem changes.
///
/// Returns a watcher handle and a receiver for debounced events.
/// The watcher triggers scan requests, not direct sync decisions.
pub fn start_watcher(
    sync_root: &Path,
) -> Result<(RecommendedWatcher, mpsc::Receiver<WatcherEvent>)> {
    let (tx, rx) = mpsc::channel();
    let mut last_event_time = std::time::Instant::now();
    let debounce = Duration::from_millis(DEBOUNCE_MS);

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                // Skip events we don't care about
                match event.kind {
                    EventKind::Any | EventKind::Other | EventKind::Access(_) => return,
                    _ => {}
                }

                // Debounce: only send if enough time has passed
                let now = std::time::Instant::now();
                if now.duration_since(last_event_time) >= debounce {
                    last_event_time = now;
                    let _ = tx.send(WatcherEvent {
                        paths: event.paths.clone(),
                    });
                }
            }
        },
        notify::Config::default(),
    )?;

    watcher.watch(sync_root, RecursiveMode::Recursive)?;

    Ok((watcher, rx))
}
