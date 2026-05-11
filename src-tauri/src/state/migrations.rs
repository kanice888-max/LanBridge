use anyhow::Result;
use rusqlite::Connection;

/// Current schema version. Increment when adding new migrations.
const CURRENT_VERSION: u32 = 1;

/// Run all pending migrations.
pub fn run(conn: &Connection) -> Result<()> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= CURRENT_VERSION {
        return Ok(());
    }

    if version < 1 {
        migrate_v1(conn)?;
    }

    conn.pragma_update(None, "user_version", CURRENT_VERSION)?;
    Ok(())
}

/// Initial schema.
fn migrate_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS sync_tasks (
            id              TEXT PRIMARY KEY,
            name            TEXT NOT NULL,
            primary_device_id   TEXT NOT NULL,
            secondary_device_id TEXT NOT NULL,
            local_path      TEXT NOT NULL,
            remote_path     TEXT NOT NULL,
            local_role      TEXT NOT NULL CHECK (local_role IN ('Primary', 'Secondary')),
            enabled         INTEGER NOT NULL DEFAULT 1,
            created_unix_ms INTEGER NOT NULL,
            updated_unix_ms INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS file_snapshots (
            task_id         TEXT NOT NULL,
            relative_path   TEXT NOT NULL,
            kind            TEXT NOT NULL CHECK (kind IN ('File', 'Directory')),
            size            INTEGER NOT NULL DEFAULT 0,
            modified_unix_ms INTEGER NOT NULL,
            blake3_hash     TEXT,
            hash_status     TEXT NOT NULL CHECK (hash_status IN ('Verified', 'UnverifiedLargeFile', 'Unavailable')),
            deleted         INTEGER NOT NULL DEFAULT 0,
            is_symlink      INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (task_id, relative_path),
            FOREIGN KEY (task_id) REFERENCES sync_tasks(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS sync_baselines (
            task_id             TEXT NOT NULL,
            relative_path       TEXT NOT NULL,
            primary_hash        TEXT,
            primary_hash_status TEXT NOT NULL CHECK (primary_hash_status IN ('Verified', 'UnverifiedLargeFile', 'Unavailable')),
            primary_modified_unix_ms INTEGER NOT NULL,
            secondary_hash      TEXT,
            secondary_hash_status TEXT NOT NULL CHECK (secondary_hash_status IN ('Verified', 'UnverifiedLargeFile', 'Unavailable')),
            secondary_modified_unix_ms INTEGER NOT NULL,
            last_synced_unix_ms INTEGER NOT NULL,
            PRIMARY KEY (task_id, relative_path),
            FOREIGN KEY (task_id) REFERENCES sync_tasks(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS pending_return_changes (
            task_id             TEXT NOT NULL,
            relative_path       TEXT NOT NULL,
            change_kind         TEXT NOT NULL CHECK (change_kind IN ('Created', 'Modified', 'Deleted')),
            secondary_hash      TEXT,
            secondary_hash_status TEXT NOT NULL CHECK (secondary_hash_status IN ('Verified', 'UnverifiedLargeFile', 'Unavailable')),
            secondary_modified_unix_ms INTEGER NOT NULL,
            created_unix_ms     INTEGER NOT NULL,
            PRIMARY KEY (task_id, relative_path),
            FOREIGN KEY (task_id) REFERENCES sync_tasks(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS history_entries (
            id                  TEXT PRIMARY KEY,
            task_id             TEXT NOT NULL,
            original_relative_path TEXT NOT NULL,
            stored_path         TEXT NOT NULL,
            reason              TEXT NOT NULL CHECK (reason IN ('Trash', 'Overwritten')),
            created_unix_ms     INTEGER NOT NULL,
            size                INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (task_id) REFERENCES sync_tasks(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS paired_devices (
            device_id       TEXT PRIMARY KEY,
            display_name    TEXT NOT NULL,
            public_key      BLOB NOT NULL,
            last_seen_unix_ms INTEGER NOT NULL,
            trusted         INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS event_logs (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            level           TEXT NOT NULL CHECK (level IN ('Info', 'Warn', 'Error')),
            task_id         TEXT,
            relative_path   TEXT,
            message         TEXT NOT NULL,
            created_unix_ms INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_snapshots_task ON file_snapshots(task_id);
        CREATE INDEX IF NOT EXISTS idx_baselines_task ON sync_baselines(task_id);
        CREATE INDEX IF NOT EXISTS idx_pending_task ON pending_return_changes(task_id);
        CREATE INDEX IF NOT EXISTS idx_history_task ON history_entries(task_id);
        CREATE INDEX IF NOT EXISTS idx_logs_time ON event_logs(created_unix_ms);
        "#,
    )?;
    Ok(())
}
