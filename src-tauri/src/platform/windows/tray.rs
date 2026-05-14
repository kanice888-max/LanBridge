/// System tray support for Windows.
///
/// Menu items used by the tray icon.

pub struct MenuItems;

impl MenuItems {
    pub const OPEN: &'static str = "open";
    pub const PAUSE_ALL: &'static str = "pause_all";
    pub const SYNC_NOW: &'static str = "sync_now";
    pub const QUIT: &'static str = "quit";
}
