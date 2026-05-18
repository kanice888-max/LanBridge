use anyhow::Result;
use std::path::{Component, Path};

pub const LANBRIDGE_TEMP_DIR: &str = ".lanbridge-temp";
pub const LANBRIDGE_HISTORY_DIR: &str = ".lanbridge-history";
pub const LANBRIDGE_PARTIAL_MARKER: &str = ".lanbridge-partial";

pub fn is_lanbridge_transient_dir_name(entry_name: &str) -> bool {
    entry_name == LANBRIDGE_TEMP_DIR
}

pub fn is_lanbridge_partial_file_name(entry_name: &str) -> bool {
    entry_name.ends_with(LANBRIDGE_PARTIAL_MARKER)
}

pub fn is_common_incomplete_download_name(entry_name: &str) -> bool {
    entry_name.ends_with(".part")
        || entry_name.ends_with(".crdownload")
        || entry_name.ends_with(".download")
}

pub fn is_protocol_ignored_component_name(entry_name: &str) -> bool {
    entry_name == LANBRIDGE_HISTORY_DIR
        || is_lanbridge_transient_dir_name(entry_name)
        || is_lanbridge_partial_file_name(entry_name)
        || is_common_incomplete_download_name(entry_name)
}

/// Platform-junk file names that should never block "empty directory" validation.
const COMMON_JUNK_FILE_NAMES: &[&str] = &[
    ".DS_Store",
    ".AppleDouble",
    "Thumbs.db",
    "desktop.ini",
    "Desktop.ini",
];

/// Platform-junk directory names that should never block "empty directory" validation.
const COMMON_JUNK_DIR_NAMES: &[&str] = &[
    "$RECYCLE.BIN",
    "System Volume Information",
    ".DocumentRevisions-V100",
    ".Spotlight-V100",
    ".TemporaryItems",
    ".Trashes",
];

/// Check whether an entry should be considered "user content" for
/// empty-directory validation during task setup.
///
/// Returns true if the entry is ignorable junk (not user content).
/// This combines protocol-transient names and common platform junk.
pub fn is_common_ignored_entry_name(name: &str, is_dir: bool) -> bool {
    if is_protocol_ignored_component_name(name) {
        return true;
    }
    if is_dir {
        COMMON_JUNK_DIR_NAMES.contains(&name)
    } else {
        COMMON_JUNK_FILE_NAMES.contains(&name)
            || name.starts_with("~$")
            || name.ends_with(".tmp")
            || name.ends_with(".lnk")
    }
}

#[cfg(test)]
mod tests_ignored_entry {
    use super::*;

    #[test]
    fn protocol_ignored_entries_are_ignored() {
        assert!(is_common_ignored_entry_name(".lanbridge-history", true));
        assert!(is_common_ignored_entry_name(".lanbridge-temp", true));
        assert!(is_common_ignored_entry_name(
            "file.txt.lanbridge-partial",
            false
        ));
    }

    #[test]
    fn platform_junk_files_are_ignored() {
        assert!(is_common_ignored_entry_name(".DS_Store", false));
        assert!(is_common_ignored_entry_name("Thumbs.db", false));
        assert!(is_common_ignored_entry_name("desktop.ini", false));
        assert!(is_common_ignored_entry_name("~$report.docx", false));
        assert!(is_common_ignored_entry_name("temp.tmp", false));
        assert!(is_common_ignored_entry_name("shortcut.lnk", false));
    }

    #[test]
    fn user_content_is_not_ignored() {
        assert!(!is_common_ignored_entry_name("readme.txt", false));
        assert!(!is_common_ignored_entry_name("src", true));
        assert!(!is_common_ignored_entry_name(".gitignore", false));
        assert!(!is_common_ignored_entry_name(".github", true));
        assert!(!is_common_ignored_entry_name("document.docx", false));
    }
}

pub fn path_has_protocol_ignored_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(name) if is_protocol_ignored_component_name(&name.to_string_lossy())
        )
    })
}

pub fn cleanup_lanbridge_transient_files(root: &Path) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }

    cleanup_lanbridge_transient_files_inner(root)
}

fn cleanup_lanbridge_transient_files_inner(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().to_string();

        if file_type.is_dir() {
            if is_lanbridge_transient_dir_name(&name) {
                std::fs::remove_dir_all(path)?;
            } else {
                cleanup_lanbridge_transient_files_inner(&path)?;
            }
        } else if file_type.is_file() && is_lanbridge_partial_file_name(&name) {
            std::fs::remove_file(path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn partial_marker_only_matches_suffixes() {
        assert!(is_lanbridge_partial_file_name("file.txt.lanbridge-partial"));
        assert!(is_lanbridge_partial_file_name(
            "file.txt.lanbridge-partial.lanbridge-partial"
        ));
        assert!(!is_lanbridge_partial_file_name(
            "my.lanbridge-partial.notes.txt"
        ));
    }

    #[test]
    fn cleanup_preserves_regular_file_containing_partial_marker() {
        let dir = TempDir::new().unwrap();
        let real_file = dir.path().join("my.lanbridge-partial.notes.txt");
        let partial_file = dir.path().join("ready.txt.lanbridge-partial");
        std::fs::write(&real_file, "keep").unwrap();
        std::fs::write(&partial_file, "delete").unwrap();

        cleanup_lanbridge_transient_files(dir.path()).unwrap();

        assert!(real_file.exists());
        assert!(!partial_file.exists());
    }
}
