use anyhow::Result;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::core::model::{HistoryEntry, HistoryReason};

/// Default history retention: 30 days in milliseconds.
pub const DEFAULT_RETENTION_DAYS_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Default history size limit: 1 GB.
pub const DEFAULT_SIZE_LIMIT_BYTES: i64 = 1024 * 1024 * 1024;

/// History store manages the `.lanbridge-history/` directory within a sync root.
pub struct HistoryStore {
    history_dir: PathBuf,
}

impl HistoryStore {
    pub fn new(sync_root: &Path) -> Self {
        Self {
            history_dir: sync_root.join(".lanbridge-history"),
        }
    }

    /// Return the path to the trash subdirectory.
    pub fn trash_dir(&self) -> PathBuf {
        self.history_dir.join("trash")
    }

    /// Return the path to the overwritten subdirectory.
    pub fn overwritten_dir(&self) -> PathBuf {
        self.history_dir.join("overwritten")
    }

    /// Move a file from its current location into the history trash.
    ///
    /// Stores at: `.lanbridge-history/trash/<unix-ms>/<relative_path>`
    pub fn move_to_trash(
        &self,
        source: &Path,
        relative_path: &str,
        now_unix_ms: i64,
    ) -> Result<HistoryEntry> {
        let dest = self
            .trash_dir()
            .join(now_unix_ms.to_string())
            .join(relative_path);
        self.move_to_history(
            source,
            &dest,
            relative_path,
            HistoryReason::Trash,
            now_unix_ms,
        )
    }

    /// Move a file to history as an overwritten backup.
    ///
    /// Stores at: `.lanbridge-history/overwritten/<unix-ms>/<relative_path>`
    pub fn move_to_overwritten(
        &self,
        source: &Path,
        relative_path: &str,
        now_unix_ms: i64,
    ) -> Result<HistoryEntry> {
        let dest = self
            .overwritten_dir()
            .join(now_unix_ms.to_string())
            .join(relative_path);
        self.move_to_history(
            source,
            &dest,
            relative_path,
            HistoryReason::Overwritten,
            now_unix_ms,
        )
    }

    /// Restore a history entry to its original relative path.
    ///
    /// If the original path is occupied, restore to a timestamped conflict-safe name.
    pub fn restore(
        &self,
        entry: &HistoryEntry,
        sync_root: &Path,
        now_unix_ms: i64,
    ) -> Result<PathBuf> {
        let source = Path::new(&entry.stored_path);
        let original = sync_root.join(&entry.original_relative_path);

        let dest = if original.exists() {
            // Original path occupied — use timestamped name
            let stem = Path::new(&entry.original_relative_path)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            let ext = Path::new(&entry.original_relative_path)
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let parent = Path::new(&entry.original_relative_path)
                .parent()
                .unwrap_or(Path::new(""));

            let dt = chrono::DateTime::from_timestamp_millis(now_unix_ms)
                .unwrap_or_default()
                .naive_utc();
            let ts = dt.format("%Y-%m-%d %H%M%S").to_string();

            let restored_name = format!("{} (restored {}){}", stem, ts, ext);
            sync_root.join(parent).join(restored_name)
        } else {
            original
        };

        // Ensure parent directory exists
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::rename(source, &dest)?;
        Ok(dest)
    }

    /// Discover history files that exist on disk even when no database row was
    /// written, such as files received from a peer-side delete before metadata
    /// persistence completed.
    pub fn discover_entries(&self, task_id: Uuid) -> Result<Vec<HistoryEntry>> {
        let mut entries = Vec::new();
        for (reason_dir, reason) in [
            (self.trash_dir(), HistoryReason::Trash),
            (self.overwritten_dir(), HistoryReason::Overwritten),
        ] {
            if !reason_dir.exists() {
                continue;
            }

            for entry in walkdir::WalkDir::new(&reason_dir)
                .min_depth(1)
                .into_iter()
                .filter_map(|entry| entry.ok())
            {
                if !entry.file_type().is_file() && !entry.file_type().is_dir() {
                    continue;
                }
                let stored_path = entry.path().to_path_buf();
                if entry.file_type().is_dir() && std::fs::read_dir(&stored_path)?.next().is_some() {
                    continue;
                }
                let relative = stored_path.strip_prefix(&reason_dir)?;
                let components = relative
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().to_string())
                    .collect::<Vec<_>>();
                if components.is_empty() {
                    continue;
                }

                let (created_unix_ms, original_parts) = match components[0].parse::<i64>() {
                    Ok(timestamp) if components.len() > 1 => (timestamp, &components[1..]),
                    Ok(_) => continue,
                    Err(_) => (
                        metadata_modified_unix_ms(&stored_path)?,
                        components.as_slice(),
                    ),
                };
                let original_relative_path = original_parts.join("/");
                let metadata = std::fs::metadata(&stored_path)?;
                let stored_path_string = stored_path.to_string_lossy().to_string();

                entries.push(HistoryEntry {
                    id: stable_history_id(task_id, &stored_path_string),
                    task_id,
                    original_relative_path,
                    stored_path: stored_path_string,
                    reason,
                    created_unix_ms,
                    size: if metadata.is_dir() {
                        0
                    } else {
                        metadata.len() as i64
                    },
                });
            }
        }
        entries.sort_by(|a, b| b.created_unix_ms.cmp(&a.created_unix_ms));
        Ok(entries)
    }

    /// Internal: move file to history directory and create HistoryEntry.
    fn move_to_history(
        &self,
        source: &Path,
        dest: &Path,
        relative_path: &str,
        reason: HistoryReason,
        now_unix_ms: i64,
    ) -> Result<HistoryEntry> {
        // Ensure destination directory exists
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Get file size before moving
        let size = std::fs::metadata(source)
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        // Move the file
        std::fs::rename(source, dest)?;

        Ok(HistoryEntry {
            id: uuid::Uuid::new_v4(),
            task_id: uuid::Uuid::nil(), // Will be set by caller
            original_relative_path: relative_path.to_string(),
            stored_path: dest.to_string_lossy().to_string(),
            reason,
            created_unix_ms: now_unix_ms,
            size,
        })
    }

    /// Check if history storage is within limits.
    ///
    /// Returns true if operations that require history storage should be blocked.
    pub fn is_storage_full(
        &self,
        total_size_bytes: i64,
        oldest_entry_unix_ms: i64,
        now_unix_ms: i64,
    ) -> bool {
        if total_size_bytes >= DEFAULT_SIZE_LIMIT_BYTES {
            return true;
        }

        if now_unix_ms - oldest_entry_unix_ms >= DEFAULT_RETENTION_DAYS_MS {
            return false; // Old entries should be cleaned, not blocking
        }

        false
    }

    /// Scan the on-disk history directory and check if storage limits are exceeded.
    ///
    /// Returns an error if destructive sync operations should be blocked.
    /// The error can be surfaced directly to the UI.
    pub fn check_storage_blocked(&self, now_unix_ms: i64) -> Result<()> {
        let mut total_size: i64 = 0;
        let mut oldest_ms: i64 = i64::MAX;

        for subdir in &["trash", "overwritten"] {
            let dir = self.history_dir.join(subdir);
            if !dir.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&dir)
                .min_depth(1)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    if let Ok(meta) = entry.metadata() {
                        total_size += meta.len() as i64;
                    }
                    if let Some(modified) = entry.metadata().ok().and_then(|m| m.modified().ok()) {
                        let ms = modified
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        if ms > 0 && ms < oldest_ms {
                            oldest_ms = ms;
                        }
                    }
                }
            }
        }

        let oldest = if oldest_ms == i64::MAX {
            now_unix_ms
        } else {
            oldest_ms
        };

        if self.is_storage_full(total_size, oldest, now_unix_ms) {
            anyhow::bail!(
                "history storage full ({:.1} MB used); clean up old entries before destructive sync operations",
                total_size as f64 / (1024.0 * 1024.0)
            );
        }

        Ok(())
    }

    /// Clean up old history entries.
    ///
    /// Removes entries older than `retention_days` days and enforces size limit.
    pub fn cleanup_old_entries(&self, cutoff_unix_ms: i64) -> Result<usize> {
        let mut deleted = 0;

        for subdir in &["trash", "overwritten"] {
            let dir = self.history_dir.join(subdir);
            if !dir.exists() {
                continue;
            }

            for entry in walkdir::WalkDir::new(&dir)
                .min_depth(2)
                .max_depth(2)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                // Check timestamp directory
                if let Some(ts_str) = entry.path().parent().and_then(|p| p.file_name()) {
                    if let Ok(ts) = ts_str.to_string_lossy().parse::<i64>() {
                        if ts < cutoff_unix_ms {
                            if entry.path().is_dir() {
                                std::fs::remove_dir_all(entry.path())?;
                            } else {
                                std::fs::remove_file(entry.path())?;
                            }
                            deleted += 1;
                        }
                    }
                }
            }
        }

        Ok(deleted)
    }
}

fn metadata_modified_unix_ms(path: &Path) -> Result<i64> {
    Ok(std::fs::metadata(path)?
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default())
}

fn stable_history_id(task_id: Uuid, stored_path: &str) -> Uuid {
    let mut hasher = blake3::Hasher::new();
    hasher.update(task_id.as_bytes());
    hasher.update(stored_path.as_bytes());
    let hash = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    Uuid::from_bytes(bytes)
}
