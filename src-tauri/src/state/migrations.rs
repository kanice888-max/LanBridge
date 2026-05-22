use anyhow::Result;
use rusqlite::Connection;

/// Current schema version. Increment when adding new migrations.
const CURRENT_VERSION: u32 = 5;

/// Run all pending migrations.
pub fn run(conn: &Connection) -> Result<()> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= CURRENT_VERSION {
        return Ok(());
    }

    if version < 1 {
        migrate_v1(conn)?;
    }

    if version < 2 {
        migrate_v2(conn)?;
    }

    if version < 3 {
        migrate_v3(conn)?;
    }

    if version < 4 {
        migrate_v4(conn)?;
    }

    if version < 5 {
        migrate_v5(conn)?;
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

/// Add primary_size to sync_baselines.
fn migrate_v2(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "sync_baselines", "primary_size")? {
        conn.execute_batch(
            r#"
            ALTER TABLE sync_baselines ADD COLUMN primary_size INTEGER NOT NULL DEFAULT 0;
            "#,
        )?;
    }
    Ok(())
}

/// Persist peer addresses and outgoing pending task invites.
fn migrate_v3(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "paired_devices", "last_address")? {
        conn.execute_batch(
            r#"
            ALTER TABLE paired_devices ADD COLUMN last_address TEXT;
            "#,
        )?;
    }

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS pending_outgoing_task_invites (
            invite_id       TEXT PRIMARY KEY,
            task_id         TEXT NOT NULL,
            name            TEXT NOT NULL,
            local_path      TEXT NOT NULL,
            peer_device_id  TEXT NOT NULL,
            local_role      TEXT NOT NULL CHECK (local_role IN ('Primary', 'Secondary')),
            created_unix_ms INTEGER NOT NULL
        );
        "#,
    )?;
    Ok(())
}

/// Add secondary_size to sync_baselines.
fn migrate_v4(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "sync_baselines", "secondary_size")? {
        conn.execute_batch(
            r#"
            ALTER TABLE sync_baselines ADD COLUMN secondary_size INTEGER NOT NULL DEFAULT 0;
            "#,
        )?;
    }
    Ok(())
}

/// Persist transfers that the user explicitly deferred.
fn migrate_v5(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS deferred_transfers (
            task_id         TEXT NOT NULL,
            relative_path   TEXT NOT NULL,
            direction       TEXT NOT NULL DEFAULT 'upload',
            reason          TEXT NOT NULL DEFAULT 'transfer deferred by user',
            created_unix_ms INTEGER NOT NULL,
            PRIMARY KEY (task_id, relative_path, direction),
            FOREIGN KEY (task_id) REFERENCES sync_tasks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_deferred_transfers_task
            ON deferred_transfers(task_id);
        "#,
    )?;
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_tolerate_columns_that_already_exist() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_v1(&conn).unwrap();
        conn.execute_batch(
            r#"
            ALTER TABLE sync_baselines ADD COLUMN primary_size INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE paired_devices ADD COLUMN last_address TEXT;
            PRAGMA user_version = 1;
            "#,
        )
        .unwrap();

        run(&conn).unwrap();

        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            CURRENT_VERSION
        );
        assert!(column_exists(&conn, "sync_baselines", "primary_size").unwrap());
        assert!(column_exists(&conn, "sync_baselines", "secondary_size").unwrap());
        assert!(column_exists(&conn, "paired_devices", "last_address").unwrap());
        assert!(table_exists(&conn, "pending_outgoing_task_invites").unwrap());
        assert!(table_exists(&conn, "deferred_transfers").unwrap());
    }

    fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}
