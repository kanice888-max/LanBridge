use anyhow::Result;
use rusqlite::Connection;
use std::sync::Mutex;

use crate::pairing::DeviceIdentity;
use crate::platform::macos::app_dirs::MacPlatform;
use crate::platform::traits::Platform;
use crate::state::db;
use crate::transport::ConnectionManager;

/// Shared application state accessible from Tauri commands.
pub struct AppState {
    pub db: Mutex<Connection>,
    pub identity: DeviceIdentity,
    pub platform: MacPlatform,
    pub connections: ConnectionManager,
}

impl AppState {
    pub fn new() -> Result<Self> {
        let platform = MacPlatform::new()?;
        let db_path = platform.database_path()?;
        let conn = db::open_db(&db_path)?;
        db::migrate(&conn)?;

        let key_path = platform.identity_key_path()?;
        let identity = DeviceIdentity::load_or_create(&key_path)?;

        Ok(Self {
            db: Mutex::new(conn),
            identity,
            platform,
            connections: ConnectionManager::new(),
        })
    }
}
