use std::backtrace::Backtrace;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const APP_LOG_FILE_NAME: &str = "lanbridge.log";
pub const STARTUP_CRASH_FILE_NAME: &str = "startup-crash.log";
pub const CRASH_DIAGNOSTICS_FILE_NAME: &str = "crash-diagnostics.log";

pub const INTERNAL_DIAGNOSTIC_FILE_NAMES: &[&str] = &[
    APP_LOG_FILE_NAME,
    STARTUP_CRASH_FILE_NAME,
    CRASH_DIAGNOSTICS_FILE_NAME,
];

pub fn record_operation(event: &str, details: impl AsRef<str>) {
    append_line(CRASH_DIAGNOSTICS_FILE_NAME, event, details.as_ref());
}

pub fn record_panic(message: &str) {
    append_line(STARTUP_CRASH_FILE_NAME, "panic", message);
    append_line(CRASH_DIAGNOSTICS_FILE_NAME, "panic", message);
    append_line(
        CRASH_DIAGNOSTICS_FILE_NAME,
        "panic_backtrace",
        format!("{:?}", Backtrace::force_capture()),
    );
}

pub fn record_startup_error(message: &str) {
    append_line(STARTUP_CRASH_FILE_NAME, "startup_error", message);
    append_line(CRASH_DIAGNOSTICS_FILE_NAME, "startup_error", message);
}

fn append_line(file_name: &str, event: &str, details: impl AsRef<str>) {
    let _guard = diagnostics_write_lock().lock().ok();
    let path = app_data_dir().join(file_name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(
        file,
        "[{}] {} {}",
        now_ms(),
        event,
        single_line(details.as_ref())
    );
}

fn diagnostics_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn single_line(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .collect()
}

fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .map(PathBuf::from)
                    .map(|home| home.join("AppData").join("Roaming"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("LanBridge")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Library")
            .join("Application Support")
            .join("LanBridge")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::temp_dir().join("LanBridge")
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
