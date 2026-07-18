/// System tray / menu bar support.
///
/// Enable by adding "system-tray" to tauri features in Cargo.toml
/// and configuring tray in tauri.conf.json.

/// Menu item definitions (used by both tray and native menu).
pub struct MenuItems;

impl MenuItems {
    pub const OPEN: &'static str = "open";
    pub const PAUSE_ALL: &'static str = "pause_all";
    pub const SYNC_NOW: &'static str = "sync_now";
    pub const QUIT: &'static str = "quit";
}
