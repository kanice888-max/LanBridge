use anyhow::Result;
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::core::model::*;

/// Repository for sync task CRUD operations.
pub struct SyncTaskRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SyncTaskRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn insert(&self, task: &SyncTask) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_tasks (id, name, primary_device_id, secondary_device_id, local_path, remote_path, local_role, enabled, created_unix_ms, updated_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task.id.to_string(),
                task.name,
                task.primary_device_id,
                task.secondary_device_id,
                task.local_path,
                task.remote_path,
                format!("{:?}", task.local_role),
                task.enabled as i32,
                task.created_unix_ms,
                task.updated_unix_ms,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &Uuid) -> Result<Option<SyncTask>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, primary_device_id, secondary_device_id, local_path, remote_path, local_role, enabled, created_unix_ms, updated_unix_ms
             FROM sync_tasks WHERE id = ?1",
        )?;
        let result = stmt.query_row(params![id.to_string()], |row| {
            let role_str: String = row.get(6)?;
            Ok(SyncTask {
                id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                name: row.get(1)?,
                primary_device_id: row.get(2)?,
                secondary_device_id: row.get(3)?,
                local_path: row.get(4)?,
                remote_path: row.get(5)?,
                local_role: match role_str.as_str() {
                    "Primary" => DeviceRole::Primary,
                    _ => DeviceRole::Secondary,
                },
                enabled: row.get::<_, i32>(7)? != 0,
                created_unix_ms: row.get(8)?,
                updated_unix_ms: row.get(9)?,
            })
        });
        match result {
            Ok(task) => Ok(Some(task)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_all(&self) -> Result<Vec<SyncTask>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, primary_device_id, secondary_device_id, local_path, remote_path, local_role, enabled, created_unix_ms, updated_unix_ms
             FROM sync_tasks ORDER BY created_unix_ms",
        )?;
        let tasks = stmt.query_map([], |row| {
            let role_str: String = row.get(6)?;
            Ok(SyncTask {
                id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                name: row.get(1)?,
                primary_device_id: row.get(2)?,
                secondary_device_id: row.get(3)?,
                local_path: row.get(4)?,
                remote_path: row.get(5)?,
                local_role: match role_str.as_str() {
                    "Primary" => DeviceRole::Primary,
                    _ => DeviceRole::Secondary,
                },
                enabled: row.get::<_, i32>(7)? != 0,
                created_unix_ms: row.get(8)?,
                updated_unix_ms: row.get(9)?,
            })
        })?;
        let mut result = Vec::new();
        for task in tasks {
            result.push(task?);
        }
        Ok(result)
    }

    pub fn update_enabled(&self, id: &Uuid, enabled: bool, now_unix_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE sync_tasks SET enabled = ?1, updated_unix_ms = ?2 WHERE id = ?3",
            params![enabled as i32, now_unix_ms, id.to_string()],
        )?;
        Ok(())
    }
}

/// Repository for file snapshot operations.
pub struct FileSnapshotRepository<'a> {
    conn: &'a Connection,
}

impl<'a> FileSnapshotRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn upsert(&self, snap: &FileSnapshot) -> Result<()> {
        self.conn.execute(
            "INSERT INTO file_snapshots (task_id, relative_path, kind, size, modified_unix_ms, blake3_hash, hash_status, deleted, is_symlink)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(task_id, relative_path) DO UPDATE SET
                kind = excluded.kind,
                size = excluded.size,
                modified_unix_ms = excluded.modified_unix_ms,
                blake3_hash = excluded.blake3_hash,
                hash_status = excluded.hash_status,
                deleted = excluded.deleted,
                is_symlink = excluded.is_symlink",
            params![
                snap.task_id.to_string(),
                snap.relative_path,
                format!("{:?}", snap.kind),
                snap.size,
                snap.modified_unix_ms,
                snap.blake3_hash,
                format!("{:?}", snap.hash_status),
                snap.deleted as i32,
                snap.is_symlink as i32,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, task_id: &Uuid, relative_path: &str) -> Result<Option<FileSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, relative_path, kind, size, modified_unix_ms, blake3_hash, hash_status, deleted, is_symlink
             FROM file_snapshots WHERE task_id = ?1 AND relative_path = ?2",
        )?;
        let result = stmt.query_row(params![task_id.to_string(), relative_path], |row| {
            Ok(FileSnapshot {
                task_id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                relative_path: row.get(1)?,
                kind: match row.get::<_, String>(2)?.as_str() {
                    "Directory" => EntryKind::Directory,
                    _ => EntryKind::File,
                },
                size: row.get(3)?,
                modified_unix_ms: row.get(4)?,
                blake3_hash: row.get(5)?,
                hash_status: match row.get::<_, String>(6)?.as_str() {
                    "Verified" => HashStatus::Verified,
                    "UnverifiedLargeFile" => HashStatus::UnverifiedLargeFile,
                    _ => HashStatus::Unavailable,
                },
                deleted: row.get::<_, i32>(7)? != 0,
                is_symlink: row.get::<_, i32>(8)? != 0,
            })
        });
        match result {
            Ok(snap) => Ok(Some(snap)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_by_task(&self, task_id: &Uuid) -> Result<Vec<FileSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, relative_path, kind, size, modified_unix_ms, blake3_hash, hash_status, deleted, is_symlink
             FROM file_snapshots WHERE task_id = ?1",
        )?;
        let rows = stmt.query_map(params![task_id.to_string()], |row| {
            Ok(FileSnapshot {
                task_id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                relative_path: row.get(1)?,
                kind: match row.get::<_, String>(2)?.as_str() {
                    "Directory" => EntryKind::Directory,
                    _ => EntryKind::File,
                },
                size: row.get(3)?,
                modified_unix_ms: row.get(4)?,
                blake3_hash: row.get(5)?,
                hash_status: match row.get::<_, String>(6)?.as_str() {
                    "Verified" => HashStatus::Verified,
                    "UnverifiedLargeFile" => HashStatus::UnverifiedLargeFile,
                    _ => HashStatus::Unavailable,
                },
                deleted: row.get::<_, i32>(7)? != 0,
                is_symlink: row.get::<_, i32>(8)? != 0,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn replace_for_task(&self, task_id: &Uuid, snapshots: &[FileSnapshot]) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_snapshots WHERE task_id = ?1",
            params![task_id.to_string()],
        )?;
        for snap in snapshots {
            self.upsert(snap)?;
        }
        Ok(())
    }

    pub fn mark_deleted(&self, task_id: &Uuid, relative_path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE file_snapshots SET deleted = 1 WHERE task_id = ?1 AND relative_path = ?2",
            params![task_id.to_string(), relative_path],
        )?;
        Ok(())
    }
}

/// Repository for sync baseline operations.
pub struct SyncBaselineRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SyncBaselineRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn upsert(&self, baseline: &SyncBaseline) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_baselines (task_id, relative_path, primary_hash, primary_hash_status, primary_size, primary_modified_unix_ms, secondary_hash, secondary_hash_status, secondary_modified_unix_ms, last_synced_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(task_id, relative_path) DO UPDATE SET
                primary_hash = excluded.primary_hash,
                primary_hash_status = excluded.primary_hash_status,
                primary_size = excluded.primary_size,
                primary_modified_unix_ms = excluded.primary_modified_unix_ms,
                secondary_hash = excluded.secondary_hash,
                secondary_hash_status = excluded.secondary_hash_status,
                secondary_modified_unix_ms = excluded.secondary_modified_unix_ms,
                last_synced_unix_ms = excluded.last_synced_unix_ms",
            params![
                baseline.task_id.to_string(),
                baseline.relative_path,
                baseline.primary_hash,
                format!("{:?}", baseline.primary_hash_status),
                baseline.primary_size,
                baseline.primary_modified_unix_ms,
                baseline.secondary_hash,
                format!("{:?}", baseline.secondary_hash_status),
                baseline.secondary_modified_unix_ms,
                baseline.last_synced_unix_ms,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, task_id: &Uuid, relative_path: &str) -> Result<Option<SyncBaseline>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, relative_path, primary_hash, primary_hash_status, primary_size, primary_modified_unix_ms, secondary_hash, secondary_hash_status, secondary_modified_unix_ms, last_synced_unix_ms
             FROM sync_baselines WHERE task_id = ?1 AND relative_path = ?2",
        )?;
        let result = stmt.query_row(params![task_id.to_string(), relative_path], |row| {
            let parse_hs = |s: String| match s.as_str() {
                "Verified" => HashStatus::Verified,
                "UnverifiedLargeFile" => HashStatus::UnverifiedLargeFile,
                _ => HashStatus::Unavailable,
            };
            Ok(SyncBaseline {
                task_id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                relative_path: row.get(1)?,
                primary_hash: row.get(2)?,
                primary_hash_status: parse_hs(row.get(3)?),
                primary_size: row.get(4)?,
                primary_modified_unix_ms: row.get(5)?,
                secondary_hash: row.get(6)?,
                secondary_hash_status: parse_hs(row.get(7)?),
                secondary_modified_unix_ms: row.get(8)?,
                last_synced_unix_ms: row.get(9)?,
            })
        });
        match result {
            Ok(b) => Ok(Some(b)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_by_task(&self, task_id: &Uuid) -> Result<Vec<SyncBaseline>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, relative_path, primary_hash, primary_hash_status, primary_size, primary_modified_unix_ms, secondary_hash, secondary_hash_status, secondary_modified_unix_ms, last_synced_unix_ms
             FROM sync_baselines WHERE task_id = ?1",
        )?;
        let rows = stmt.query_map(params![task_id.to_string()], |row| {
            let parse_hs = |s: String| match s.as_str() {
                "Verified" => HashStatus::Verified,
                "UnverifiedLargeFile" => HashStatus::UnverifiedLargeFile,
                _ => HashStatus::Unavailable,
            };
            Ok(SyncBaseline {
                task_id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                relative_path: row.get(1)?,
                primary_hash: row.get(2)?,
                primary_hash_status: parse_hs(row.get(3)?),
                primary_size: row.get(4)?,
                primary_modified_unix_ms: row.get(5)?,
                secondary_hash: row.get(6)?,
                secondary_hash_status: parse_hs(row.get(7)?),
                secondary_modified_unix_ms: row.get(8)?,
                last_synced_unix_ms: row.get(9)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }
}

/// Repository for pending return changes.
pub struct PendingReturnRepository<'a> {
    conn: &'a Connection,
}

impl<'a> PendingReturnRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn upsert(&self, change: &PendingReturnChange) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pending_return_changes (task_id, relative_path, change_kind, secondary_hash, secondary_hash_status, secondary_modified_unix_ms, created_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(task_id, relative_path) DO UPDATE SET
                change_kind = excluded.change_kind,
                secondary_hash = excluded.secondary_hash,
                secondary_hash_status = excluded.secondary_hash_status,
                secondary_modified_unix_ms = excluded.secondary_modified_unix_ms",
            params![
                change.task_id.to_string(),
                change.relative_path,
                format!("{:?}", change.change_kind),
                change.secondary_hash,
                format!("{:?}", change.secondary_hash_status),
                change.secondary_modified_unix_ms,
                change.created_unix_ms,
            ],
        )?;
        Ok(())
    }

    pub fn list_by_task(&self, task_id: &Uuid) -> Result<Vec<PendingReturnChange>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, relative_path, change_kind, secondary_hash, secondary_hash_status, secondary_modified_unix_ms, created_unix_ms
             FROM pending_return_changes WHERE task_id = ?1 ORDER BY created_unix_ms",
        )?;
        let rows = stmt.query_map(params![task_id.to_string()], |row| {
            let parse_hs = |s: String| match s.as_str() {
                "Verified" => HashStatus::Verified,
                "UnverifiedLargeFile" => HashStatus::UnverifiedLargeFile,
                _ => HashStatus::Unavailable,
            };
            Ok(PendingReturnChange {
                task_id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                relative_path: row.get(1)?,
                change_kind: match row.get::<_, String>(2)?.as_str() {
                    "Created" => ChangeKind::Created,
                    "Modified" => ChangeKind::Modified,
                    _ => ChangeKind::Deleted,
                },
                secondary_hash: row.get(3)?,
                secondary_hash_status: parse_hs(row.get(4)?),
                secondary_modified_unix_ms: row.get(5)?,
                created_unix_ms: row.get(6)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn remove(&self, task_id: &Uuid, relative_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM pending_return_changes WHERE task_id = ?1 AND relative_path = ?2",
            params![task_id.to_string(), relative_path],
        )?;
        Ok(())
    }

    pub fn count_by_task(&self, task_id: &Uuid) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM pending_return_changes WHERE task_id = ?1",
            params![task_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

/// Repository for history entries.
pub struct HistoryRepository<'a> {
    conn: &'a Connection,
}

impl<'a> HistoryRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn insert(&self, entry: &HistoryEntry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO history_entries (id, task_id, original_relative_path, stored_path, reason, created_unix_ms, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.id.to_string(),
                entry.task_id.to_string(),
                entry.original_relative_path,
                entry.stored_path,
                format!("{:?}", entry.reason),
                entry.created_unix_ms,
                entry.size,
            ],
        )?;
        Ok(())
    }

    pub fn list_by_task(&self, task_id: &Uuid) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, original_relative_path, stored_path, reason, created_unix_ms, size
             FROM history_entries WHERE task_id = ?1 ORDER BY created_unix_ms DESC",
        )?;
        let rows = stmt.query_map(params![task_id.to_string()], |row| {
            Ok(HistoryEntry {
                id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                task_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap(),
                original_relative_path: row.get(2)?,
                stored_path: row.get(3)?,
                reason: match row.get::<_, String>(4)?.as_str() {
                    "Overwritten" => HistoryReason::Overwritten,
                    _ => HistoryReason::Trash,
                },
                created_unix_ms: row.get(5)?,
                size: row.get(6)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn total_size_by_task(&self, task_id: &Uuid) -> Result<i64> {
        let size: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(size), 0) FROM history_entries WHERE task_id = ?1",
            params![task_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(size)
    }

    pub fn delete_older_than(&self, task_id: &Uuid, cutoff_unix_ms: i64) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM history_entries WHERE task_id = ?1 AND created_unix_ms < ?2",
            params![task_id.to_string(), cutoff_unix_ms],
        )?;
        Ok(count)
    }
}

/// Repository for paired devices.
pub struct PairedDeviceRepository<'a> {
    conn: &'a Connection,
}

impl<'a> PairedDeviceRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn upsert(&self, device: &PairedDevice) -> Result<()> {
        self.conn.execute(
            "INSERT INTO paired_devices (device_id, display_name, public_key, last_seen_unix_ms, trusted)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(device_id) DO UPDATE SET
                display_name = excluded.display_name,
                public_key = excluded.public_key,
                last_seen_unix_ms = excluded.last_seen_unix_ms,
                trusted = excluded.trusted",
            params![
                device.device_id,
                device.display_name,
                device.public_key,
                device.last_seen_unix_ms,
                device.trusted as i32,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, device_id: &str) -> Result<Option<PairedDevice>> {
        let mut stmt = self.conn.prepare(
            "SELECT device_id, display_name, public_key, last_seen_unix_ms, trusted
             FROM paired_devices WHERE device_id = ?1",
        )?;
        let result = stmt.query_row(params![device_id], |row| {
            Ok(PairedDevice {
                device_id: row.get(0)?,
                display_name: row.get(1)?,
                public_key: row.get(2)?,
                last_seen_unix_ms: row.get(3)?,
                trusted: row.get::<_, i32>(4)? != 0,
            })
        });
        match result {
            Ok(d) => Ok(Some(d)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_all(&self) -> Result<Vec<PairedDevice>> {
        let mut stmt = self.conn.prepare(
            "SELECT device_id, display_name, public_key, last_seen_unix_ms, trusted
             FROM paired_devices ORDER BY last_seen_unix_ms DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PairedDevice {
                device_id: row.get(0)?,
                display_name: row.get(1)?,
                public_key: row.get(2)?,
                last_seen_unix_ms: row.get(3)?,
                trusted: row.get::<_, i32>(4)? != 0,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }
}

/// Repository for event logs.
pub struct LogRepository<'a> {
    conn: &'a Connection,
}

impl<'a> LogRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn insert(&self, entry: &LogEntry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO event_logs (level, task_id, relative_path, message, created_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                format!("{:?}", entry.level),
                entry.task_id.map(|id| id.to_string()),
                entry.relative_path,
                entry.message,
                entry.created_unix_ms,
            ],
        )?;
        Ok(())
    }

    pub fn list_recent(&self, limit: usize) -> Result<Vec<LogEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, level, task_id, relative_path, message, created_unix_ms
             FROM event_logs ORDER BY created_unix_ms DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(LogEntry {
                id: Some(row.get(0)?),
                level: match row.get::<_, String>(1)?.as_str() {
                    "Warn" => LogLevel::Warn,
                    "Error" => LogLevel::Error,
                    _ => LogLevel::Info,
                },
                task_id: row
                    .get::<_, Option<String>>(2)?
                    .and_then(|s| Uuid::parse_str(&s).ok()),
                relative_path: row.get(3)?,
                message: row.get(4)?,
                created_unix_ms: row.get(5)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Enforce log retention: keep latest `max_entries` or entries within `max_age_ms`.
    pub fn enforce_retention(&self, max_entries: usize, cutoff_unix_ms: i64) -> Result<usize> {
        // Delete entries older than cutoff
        let deleted_old = self.conn.execute(
            "DELETE FROM event_logs WHERE created_unix_ms < ?1",
            params![cutoff_unix_ms],
        )?;
        // Delete entries beyond max count
        let deleted_overflow = self.conn.execute(
            "DELETE FROM event_logs WHERE id NOT IN (
                SELECT id FROM event_logs ORDER BY created_unix_ms DESC LIMIT ?1
            )",
            params![max_entries as i64],
        )?;
        Ok(deleted_old + deleted_overflow)
    }
}
