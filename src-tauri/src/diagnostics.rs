use std::collections::HashSet;
use std::backtrace::Backtrace;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::model::{LogEntry, SyncTask};

pub const APP_LOG_FILE_NAME: &str = "lanbridge.log";
pub const STARTUP_CRASH_FILE_NAME: &str = "startup-crash.log";
pub const CRASH_DIAGNOSTICS_FILE_NAME: &str = "crash-diagnostics.log";

const CRASH_DIAGNOSTICS_MAX_BYTES: u64 = 8 * 1024 * 1024;
const STARTUP_CRASH_MAX_BYTES: u64 = 1024 * 1024;
const MAX_DIAGNOSTIC_LINE_CHARS: usize = 64 * 1024;
const REPORT_DIAGNOSTIC_LINES: usize = 200;
const REPORT_STARTUP_LINES: usize = 100;
const REPORT_LOG_ENTRIES: usize = 50;
const REPORT_LINE_MAX_CHARS: usize = 2_048;
const REPORT_MAX_BYTES: usize = 256 * 1024;

pub const INTERNAL_DIAGNOSTIC_FILE_NAMES: &[&str] = &[
    APP_LOG_FILE_NAME,
    STARTUP_CRASH_FILE_NAME,
    CRASH_DIAGNOSTICS_FILE_NAME,
];

/// Build a bounded, shareable support report without exposing private app state.
///
/// The report deliberately excludes the database, keys, pins, and file contents.
/// Task roots, user home directories, and UUID-like identifiers are redacted before
/// the text crosses the Tauri command boundary.
pub fn build_diagnostic_report(
    app_data_dir: &Path,
    tasks: &[SyncTask],
    logs: &[LogEntry],
) -> String {
    let redactor = DiagnosticRedactor::new(app_data_dir, tasks);
    let mut report = String::new();
    report.push_str("LanBridge 诊断摘要\n");
    report.push_str(&format!("应用版本: {}\n", env!("CARGO_PKG_VERSION")));
    report.push_str(&format!(
        "系统: {} ({})\n生成时间: {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        now_ms()
    ));
    report.push_str("已自动隐藏本机目录、任务根目录与完整设备标识。\n");

    append_file_section(
        &mut report,
        "运行诊断（最近 200 条）",
        &app_data_dir.join(CRASH_DIAGNOSTICS_FILE_NAME),
        REPORT_DIAGNOSTIC_LINES,
    );
    append_file_section(
        &mut report,
        "启动/崩溃记录（最近 100 条）",
        &app_data_dir.join(STARTUP_CRASH_FILE_NAME),
        REPORT_STARTUP_LINES,
    );

    report.push_str("\n[同步记录（最近 50 条）]\n");
    if logs.is_empty() {
        report.push_str("暂无记录\n");
    } else {
        for entry in logs.iter().take(REPORT_LOG_ENTRIES) {
            let path = entry.relative_path.as_deref().unwrap_or("-");
            append_report_line(
                &mut report,
                &format!(
                    "[{}] {:?} path={} message={}",
                    entry.created_unix_ms, entry.level, path, entry.message
                ),
            );
        }
    }

    truncate_report(&redactor.redact(&report))
}

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
    append_line_to_path(&path, event, details.as_ref(), max_log_bytes(file_name));
}

fn append_line_to_path(path: &std::path::Path, event: &str, details: &str, max_bytes: u64) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = rotate_log_if_needed(path, max_bytes);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "[{}] {} {}", now_ms(), event, single_line(details));
}

fn append_file_section(report: &mut String, title: &str, path: &Path, limit: usize) {
    report.push_str(&format!("\n[{}]\n", title));
    match read_recent_lines(path, limit) {
        Ok(lines) if lines.is_empty() => report.push_str("暂无记录\n"),
        Ok(lines) => {
            for line in lines {
                append_report_line(report, &line);
            }
        }
        Err(_) => report.push_str("暂无记录\n"),
    }
}

fn read_recent_lines(path: &Path, limit: usize) -> std::io::Result<Vec<String>> {
    let content = String::from_utf8_lossy(&std::fs::read(path)?).into_owned();
    let mut lines = content.lines().rev().take(limit).map(str::to_owned).collect::<Vec<_>>();
    lines.reverse();
    Ok(lines)
}

fn append_report_line(report: &mut String, line: &str) {
    let line = single_line(line);
    let clipped = line.chars().take(REPORT_LINE_MAX_CHARS).collect::<String>();
    report.push_str(&clipped);
    if line.chars().count() > REPORT_LINE_MAX_CHARS {
        report.push_str("…");
    }
    report.push('\n');
}

fn truncate_report(report: &str) -> String {
    if report.len() <= REPORT_MAX_BYTES {
        return report.to_string();
    }

    const SUFFIX: &str = "\n[报告内容已截断]\n";
    let mut end = REPORT_MAX_BYTES.saturating_sub(SUFFIX.len());
    while end > 0 && !report.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &report[..end], SUFFIX)
}

struct DiagnosticRedactor {
    replacements: Vec<(String, String)>,
}

impl DiagnosticRedactor {
    fn new(app_data_dir: &Path, tasks: &[SyncTask]) -> Self {
        let mut replacements = Vec::new();
        add_path_variants(&mut replacements, &app_data_dir.to_string_lossy(), "<APP_DATA>");

        let mut seen_roots = HashSet::new();
        let mut root_index = 0;
        for task in tasks {
            for root in [&task.local_path, &task.remote_path] {
                let root = root.trim();
                if root.is_empty() || !seen_roots.insert(root.to_string()) {
                    continue;
                }
                root_index += 1;
                add_path_variants(
                    &mut replacements,
                    root,
                    &format!("<TASK_ROOT_{root_index}>"),
                );
            }
        }

        replacements.sort_by(|left, right| right.0.len().cmp(&left.0.len()));
        Self { replacements }
    }

    fn redact(&self, value: &str) -> String {
        let mut redacted = value.to_string();
        for (path, replacement) in &self.replacements {
            redacted = redacted.replace(path, replacement);
        }
        redact_user_home_prefixes(redact_uuid_like_identifiers(redacted))
    }
}

fn add_path_variants(replacements: &mut Vec<(String, String)>, path: &str, replacement: &str) {
    if path.is_empty() {
        return;
    }
    for variant in [
        path.to_string(),
        path.replace('\\', "/"),
        path.replace('/', "\\"),
    ] {
        if !variant.is_empty() && !replacements.iter().any(|(known, _)| known == &variant) {
            replacements.push((variant, replacement.to_string()));
        }
    }
}

fn redact_uuid_like_identifiers(value: String) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < value.len() {
        if let Some(candidate) = value.get(cursor..cursor.saturating_add(36)) {
            if uuid::Uuid::parse_str(candidate).is_ok() {
                output.push_str("<ID>");
                cursor += 36;
                continue;
            }
        }
        let Some(character) = value[cursor..].chars().next() else {
            break;
        };
        output.push(character);
        cursor += character.len_utf8();
    }
    output
}

fn redact_user_home_prefixes(value: String) -> String {
    let mut redacted = redact_home_prefix(&value, "/Users/");
    redacted = redact_home_prefix(&redacted, "/home/");
    for drive in 'A'..='Z' {
        redacted = redact_home_prefix(&redacted, &format!("{drive}:\\Users\\"));
        redacted = redact_home_prefix(&redacted, &format!("{drive}:/Users/"));
    }
    redacted
}

fn redact_home_prefix(value: &str, prefix: &str) -> String {
    let mut result = value.to_string();
    let mut search_start = 0;
    while let Some(offset) = result[search_start..].find(prefix) {
        let start = search_start + offset;
        let user_start = start + prefix.len();
        let user_end = result[user_start..]
            .find(|character: char| {
                character == '/' || character == '\\' || character.is_whitespace() || character == '"'
            })
            .map(|offset| user_start + offset)
            .unwrap_or(result.len());
        if user_end == user_start {
            search_start = user_start;
            continue;
        }
        result.replace_range(start..user_end, "<HOME>");
        search_start = start + "<HOME>".len();
    }
    result
}

fn max_log_bytes(file_name: &str) -> u64 {
    match file_name {
        STARTUP_CRASH_FILE_NAME => STARTUP_CRASH_MAX_BYTES,
        _ => CRASH_DIAGNOSTICS_MAX_BYTES,
    }
}

fn rotate_log_if_needed(path: &std::path::Path, max_bytes: u64) -> std::io::Result<()> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() < max_bytes {
        return Ok(());
    }

    let rotated = PathBuf::from(format!("{}.1", path.display()));
    let _ = std::fs::remove_file(&rotated);

    // Legacy builds could generate multi-gigabyte diagnostics files. Keeping one of
    // those as a rotated archive would not reclaim disk space, so discard grossly
    // oversized files and start a bounded log instead.
    if metadata.len() > max_bytes.saturating_mul(2) {
        std::fs::remove_file(path)?;
    } else {
        std::fs::rename(path, rotated)?;
    }
    Ok(())
}

fn diagnostics_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn single_line(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .take(MAX_DIAGNOSTIC_LINE_CHARS)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{DeviceRole, LogLevel};

    fn sample_task(local_path: &str, remote_path: &str) -> SyncTask {
        SyncTask {
            id: uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            name: "Private task".to_string(),
            primary_device_id: "22222222-2222-2222-2222-222222222222".to_string(),
            secondary_device_id: "33333333-3333-3333-3333-333333333333".to_string(),
            local_path: local_path.to_string(),
            remote_path: remote_path.to_string(),
            local_role: DeviceRole::Primary,
            enabled: true,
            created_unix_ms: 0,
            updated_unix_ms: 0,
            last_transfer_activity_unix_ms: 0,
        }
    }

    fn sample_log() -> LogEntry {
        LogEntry {
            id: Some(1),
            level: LogLevel::Error,
            task_id: Some(uuid::Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap()),
            relative_path: Some("private/report.txt".to_string()),
            message: "transfer failed for 55555555-5555-5555-5555-555555555555".to_string(),
            created_unix_ms: 1,
        }
    }

    #[test]
    fn bounded_log_rotates_at_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("diagnostics.log");
        std::fs::write(&path, vec![b'x'; 128]).unwrap();

        append_line_to_path(&path, "event", "details", 128);

        assert!(path.exists());
        assert!(dir.path().join("diagnostics.log.1").exists());
        assert!(std::fs::metadata(&path).unwrap().len() < 128);
    }

    #[test]
    fn grossly_oversized_legacy_log_is_discarded_instead_of_archived() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("diagnostics.log");
        std::fs::write(&path, vec![b'x'; 257]).unwrap();

        append_line_to_path(&path, "event", "details", 128);

        assert!(path.exists());
        assert!(!dir.path().join("diagnostics.log.1").exists());
        assert!(std::fs::metadata(&path).unwrap().len() < 128);
    }

    #[test]
    fn diagnostic_lines_are_capped() {
        let line = single_line(&"x".repeat(MAX_DIAGNOSTIC_LINE_CHARS + 10));
        assert_eq!(line.len(), MAX_DIAGNOSTIC_LINE_CHARS);
    }

    #[test]
    fn report_redacts_task_roots_home_paths_and_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CRASH_DIAGNOSTICS_FILE_NAME),
            "scan root=/Users/alice/private-sync peer=aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa\nwindows=C:\\Users\\Alice\\Mirror\nother=/Users/alice/Desktop",
        )
        .unwrap();
        let report = build_diagnostic_report(
            dir.path(),
            &[sample_task("/Users/alice/private-sync", "C:\\Users\\Alice\\Mirror")],
            &[sample_log()],
        );

        assert!(!report.contains("/Users/alice/private-sync"));
        assert!(!report.contains("C:\\Users\\Alice\\Mirror"));
        assert!(!report.contains("/Users/alice/Desktop"));
        assert!(!report.contains("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"));
        assert!(!report.contains("55555555-5555-5555-5555-555555555555"));
        assert!(report.contains("<TASK_ROOT_1>"));
        assert!(report.contains("<TASK_ROOT_2>"));
        assert!(report.contains("<HOME>/Desktop"));
        assert!(report.contains("<ID>"));
        assert!(report.contains("private/report.txt"));
    }

    #[test]
    fn report_handles_missing_files_and_limits_recent_lines() {
        let dir = tempfile::tempdir().unwrap();
        let diagnostics = (0..220)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join(CRASH_DIAGNOSTICS_FILE_NAME), diagnostics).unwrap();

        let report = build_diagnostic_report(dir.path(), &[], &[]);
        assert!(report.contains("暂无记录"));
        assert!(report.contains("line-219"));
        assert!(!report.contains("line-19\n"));
    }

    #[test]
    fn report_is_bounded_even_when_each_section_is_large() {
        let dir = tempfile::tempdir().unwrap();
        let diagnostics = (0..REPORT_DIAGNOSTIC_LINES)
            .map(|_| "x".repeat(REPORT_LINE_MAX_CHARS + 100))
            .collect::<Vec<_>>()
            .join("\n");
        let startup = (0..REPORT_STARTUP_LINES)
            .map(|_| "y".repeat(REPORT_LINE_MAX_CHARS + 100))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join(CRASH_DIAGNOSTICS_FILE_NAME), diagnostics).unwrap();
        std::fs::write(dir.path().join(STARTUP_CRASH_FILE_NAME), startup).unwrap();

        let report = build_diagnostic_report(dir.path(), &[], &[]);
        assert!(report.len() <= REPORT_MAX_BYTES);
        assert!(report.contains("[报告内容已截断]"));
    }
}
