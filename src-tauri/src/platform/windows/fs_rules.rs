use crate::platform::traits::{IgnoreDecision, IgnoreReason};

/// Exact file names to ignore on Windows.
const EXACT_FILE_NAMES: &[&str] = &[
    "Thumbs.db",
    "desktop.ini",
    "Desktop.ini",
    ".DS_Store",
    crate::diagnostics::APP_LOG_FILE_NAME,
    crate::diagnostics::STARTUP_CRASH_FILE_NAME,
    crate::diagnostics::CRASH_DIAGNOSTICS_FILE_NAME,
];

/// Exact directory names to ignore (matched with trailing slash).
const EXACT_DIR_NAMES: &[&str] = &[
    "$RECYCLE.BIN",
    "System Volume Information",
    ".DocumentRevisions-V100",
    ".Spotlight-V100",
    ".TemporaryItems",
    ".Trashes",
    ".git",
    "node_modules",
    ".lanbridge-history",
    ".lanbridge-temp",
];

/// Glob patterns to ignore.
const GLOB_PATTERNS: &[&str] = &[
    "~$*",   // Office temp files (~$report.docx)
    "*.tmp", // Temp files (covers ~WRL*.tmp and others)
    "*.lnk", // Windows shortcuts (target is machine-local)
];

/// Check whether an entry should be ignored on Windows.
///
/// `entry_name` is the bare file or directory name (not a full path).
/// `is_dir` indicates whether the entry is a directory.
pub fn classify_entry(entry_name: &str, is_dir: bool) -> IgnoreDecision {
    // Check exact directory matches first
    if is_dir {
        for dir in EXACT_DIR_NAMES {
            if entry_name == *dir {
                return IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(dir.to_string()));
            }
        }
    }

    // Check exact file name matches
    for name in EXACT_FILE_NAMES {
        if entry_name == *name {
            return IgnoreDecision::Ignored(IgnoreReason::ExactName(name.to_string()));
        }
    }

    if !is_dir && crate::core::transient::is_lanbridge_partial_file_name(entry_name) {
        return IgnoreDecision::Ignored(IgnoreReason::GlobPattern(
            "*.lanbridge-partial*".to_string(),
        ));
    }

    if !is_dir && crate::core::transient::is_common_incomplete_download_name(entry_name) {
        return IgnoreDecision::Ignored(IgnoreReason::GlobPattern(
            "*.part|*.crdownload|*.download".to_string(),
        ));
    }

    // Check glob patterns
    for pattern in GLOB_PATTERNS {
        if matches_glob(entry_name, pattern) {
            return IgnoreDecision::Ignored(IgnoreReason::GlobPattern(pattern.to_string()));
        }
    }

    IgnoreDecision::Allowed
}

/// Simple glob matching for `*` wildcards.
fn matches_glob(text: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.len() == 1 {
        return text == pattern;
    }

    let starts_with_wildcard = pattern.starts_with('*');
    let ends_with_wildcard = pattern.ends_with('*');

    let mut pos = 0;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if i == 0 && !starts_with_wildcard {
            if !text[pos..].starts_with(part) {
                return false;
            }
            pos += part.len();
        } else if i == parts.len() - 1 && !ends_with_wildcard {
            if !text.ends_with(part) {
                return false;
            }
        } else {
            match text[pos..].find(part) {
                Some(found) => pos = found + part.len(),
                None => return false,
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windows_exact_file_ignored() {
        assert_eq!(
            classify_entry("Thumbs.db", false),
            IgnoreDecision::Ignored(IgnoreReason::ExactName("Thumbs.db".to_string()))
        );
        assert_eq!(
            classify_entry("desktop.ini", false),
            IgnoreDecision::Ignored(IgnoreReason::ExactName("desktop.ini".to_string()))
        );
        assert_eq!(
            classify_entry("lanbridge.log", false),
            IgnoreDecision::Ignored(IgnoreReason::ExactName("lanbridge.log".to_string()))
        );
        assert_eq!(
            classify_entry("startup-crash.log", false),
            IgnoreDecision::Ignored(IgnoreReason::ExactName("startup-crash.log".to_string()))
        );
        assert_eq!(
            classify_entry("crash-diagnostics.log", false),
            IgnoreDecision::Ignored(IgnoreReason::ExactName("crash-diagnostics.log".to_string()))
        );
    }

    #[test]
    fn test_windows_exact_dir_ignored() {
        assert_eq!(
            classify_entry("$RECYCLE.BIN", true),
            IgnoreDecision::Ignored(IgnoreReason::ExactDirectory("$RECYCLE.BIN".to_string()))
        );
        assert_eq!(
            classify_entry("System Volume Information", true),
            IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(
                "System Volume Information".to_string()
            ))
        );
        assert_eq!(
            classify_entry(".git", true),
            IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(".git".to_string()))
        );
        assert_eq!(
            classify_entry("node_modules", true),
            IgnoreDecision::Ignored(IgnoreReason::ExactDirectory("node_modules".to_string()))
        );
    }

    #[test]
    fn test_git_dir_only_exact_match() {
        assert_eq!(classify_entry(".gitignore", false), IgnoreDecision::Allowed);
        assert_eq!(classify_entry(".github", true), IgnoreDecision::Allowed);
        assert_eq!(
            classify_entry(".gitmodules", false),
            IgnoreDecision::Allowed
        );
    }

    #[test]
    fn test_windows_glob_patterns() {
        assert_eq!(
            classify_entry("~$report.docx", false),
            IgnoreDecision::Ignored(IgnoreReason::GlobPattern("~$*".to_string()))
        );
        assert_eq!(
            classify_entry("temp.tmp", false),
            IgnoreDecision::Ignored(IgnoreReason::GlobPattern("*.tmp".to_string()))
        );
        assert_eq!(
            classify_entry("shortcut.lnk", false),
            IgnoreDecision::Ignored(IgnoreReason::GlobPattern("*.lnk".to_string()))
        );
    }

    #[test]
    fn test_allowed_entries() {
        assert_eq!(classify_entry("readme.txt", false), IgnoreDecision::Allowed);
        assert_eq!(classify_entry("src", true), IgnoreDecision::Allowed);
        assert_eq!(classify_entry(".gitignore", false), IgnoreDecision::Allowed);
        assert_eq!(
            classify_entry("document.docx", false),
            IgnoreDecision::Allowed
        );
    }
}
