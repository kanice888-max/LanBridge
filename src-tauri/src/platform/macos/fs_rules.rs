use crate::platform::traits::{IgnoreDecision, IgnoreReason};

/// Exact file names to ignore on macOS.
const EXACT_FILE_NAMES: &[&str] = &[".DS_Store", ".AppleDouble"];

/// Exact directory names to ignore (matched with trailing slash).
const EXACT_DIR_NAMES: &[&str] = &[
    ".DocumentRevisions-V100",
    ".Spotlight-V100",
    ".TemporaryItems",
    ".Trashes",
    ".git",
    "node_modules",
    ".lanbridge-history",
];

/// Glob patterns to ignore.
const GLOB_PATTERNS: &[&str] = &[
    "~$*",   // Office temp files
    "*.tmp", // Temp files
];

/// Check whether an entry should be ignored on macOS.
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

    // Pattern starts with *
    let starts_with_wildcard = pattern.starts_with('*');
    // Pattern ends with *
    let ends_with_wildcard = pattern.ends_with('*');

    let mut pos = 0;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if i == 0 && !starts_with_wildcard {
            // First part must match at start
            if !text[pos..].starts_with(part) {
                return false;
            }
            pos += part.len();
        } else if i == parts.len() - 1 && !ends_with_wildcard {
            // Last part must match at end
            if !text.ends_with(part) {
                return false;
            }
        } else {
            // Middle part: find in remaining text
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
    fn test_exact_file_ignored() {
        assert_eq!(
            classify_entry(".DS_Store", false),
            IgnoreDecision::Ignored(IgnoreReason::ExactName(".DS_Store".to_string()))
        );
    }

    #[test]
    fn test_exact_dir_ignored() {
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
        // `.git` dir is ignored, but `.gitignore` and `.github` are NOT
        assert_eq!(classify_entry(".gitignore", false), IgnoreDecision::Allowed);
        assert_eq!(classify_entry(".github", true), IgnoreDecision::Allowed);
        assert_eq!(
            classify_entry(".gitmodules", false),
            IgnoreDecision::Allowed
        );
    }

    #[test]
    fn test_glob_patterns() {
        assert_eq!(
            classify_entry("~$report.docx", false),
            IgnoreDecision::Ignored(IgnoreReason::GlobPattern("~$*".to_string()))
        );
        assert_eq!(
            classify_entry("temp.tmp", false),
            IgnoreDecision::Ignored(IgnoreReason::GlobPattern("*.tmp".to_string()))
        );
    }

    #[test]
    fn test_allowed_entries() {
        assert_eq!(classify_entry("readme.txt", false), IgnoreDecision::Allowed);
        assert_eq!(classify_entry("src", true), IgnoreDecision::Allowed);
        assert_eq!(classify_entry(".gitignore", false), IgnoreDecision::Allowed);
    }
}
