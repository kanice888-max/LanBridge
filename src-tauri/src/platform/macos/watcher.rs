use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc;

use crate::platform::traits::PlatformWatcherEvent;

/// Start watching a directory for filesystem changes.
///
/// On macOS, `notify` uses FSEvents. Platform events are forwarded
/// immediately; task-level debounce is handled by `TaskDirtyTracker`.
pub fn start_watcher(
    sync_root: &Path,
) -> Result<(RecommendedWatcher, mpsc::Receiver<PlatformWatcherEvent>)> {
    let (tx, rx) = mpsc::channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                if let Some(event) = platform_watcher_event(event) {
                    let _ = tx.send(event);
                }
            }
        },
        notify::Config::default(),
    )?;

    watcher.watch(sync_root, RecursiveMode::Recursive)?;

    Ok((watcher, rx))
}

fn platform_watcher_event(event: Event) -> Option<PlatformWatcherEvent> {
    match event.kind {
        EventKind::Any | EventKind::Other | EventKind::Access(_) => None,
        _ if event.paths.is_empty() => None,
        _ => Some(PlatformWatcherEvent { paths: event.paths }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::{event::ModifyKind, EventKind};
    use std::path::PathBuf;

    #[test]
    fn forwards_valid_event_without_waiting_for_next_event() {
        let path = PathBuf::from("/tmp/lanbridge-watch-file");
        let event = Event::new(EventKind::Modify(ModifyKind::Data(
            notify::event::DataChange::Any,
        )))
        .add_path(path.clone());

        let forwarded = platform_watcher_event(event).unwrap();
        assert_eq!(forwarded.paths, vec![path]);
    }
}
