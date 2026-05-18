use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::time::Duration;

/// Open or create the SQLite database at the given path.
///
/// Enables WAL mode and foreign keys on every connection.
pub fn open_db(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

/// Run all pending migrations on the given connection.
pub fn migrate(conn: &Connection) -> Result<()> {
    super::migrations::run(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_db_sets_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(&dir.path().join("state.sqlite")).unwrap();
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();

        assert!(busy_timeout >= 5000);
    }
}
