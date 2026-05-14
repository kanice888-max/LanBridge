use lanbridge::platform::traits::{IgnoreDecision, IgnoreReason, Platform};
use lanbridge::platform::windows::fs_rules;
use lanbridge::platform::windows::WinPlatform;
use std::path::PathBuf;
use tempfile::TempDir;

// ===== fs_rules tests =====

#[test]
fn test_thumbs_db_ignored() {
    assert_eq!(
        fs_rules::classify_entry("Thumbs.db", false),
        IgnoreDecision::Ignored(IgnoreReason::ExactName("Thumbs.db".to_string()))
    );
}

#[test]
fn test_desktop_ini_ignored() {
    assert_eq!(
        fs_rules::classify_entry("desktop.ini", false),
        IgnoreDecision::Ignored(IgnoreReason::ExactName("desktop.ini".to_string()))
    );
    assert_eq!(
        fs_rules::classify_entry("Desktop.ini", false),
        IgnoreDecision::Ignored(IgnoreReason::ExactName("Desktop.ini".to_string()))
    );
}

#[test]
fn test_ds_store_ignored() {
    assert_eq!(
        fs_rules::classify_entry(".DS_Store", false),
        IgnoreDecision::Ignored(IgnoreReason::ExactName(".DS_Store".to_string()))
    );
}

#[test]
fn test_recycle_bin_ignored() {
    assert_eq!(
        fs_rules::classify_entry("$RECYCLE.BIN", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory("$RECYCLE.BIN".to_string()))
    );
}

#[test]
fn test_system_volume_ignored() {
    assert_eq!(
        fs_rules::classify_entry("System Volume Information", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(
            "System Volume Information".to_string()
        ))
    );
}

#[test]
fn test_git_directory_ignored() {
    assert_eq!(
        fs_rules::classify_entry(".git", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(".git".to_string()))
    );
}

#[test]
fn test_node_modules_ignored() {
    assert_eq!(
        fs_rules::classify_entry("node_modules", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory("node_modules".to_string()))
    );
}

#[test]
fn test_history_directory_ignored() {
    assert_eq!(
        fs_rules::classify_entry(".lanbridge-history", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(
            ".lanbridge-history".to_string()
        ))
    );
}

#[test]
fn test_git_file_not_ignored() {
    assert_eq!(
        fs_rules::classify_entry(".gitignore", false),
        IgnoreDecision::Allowed
    );
    assert_eq!(
        fs_rules::classify_entry(".gitmodules", false),
        IgnoreDecision::Allowed
    );
}

#[test]
fn test_github_dir_not_ignored() {
    assert_eq!(
        fs_rules::classify_entry(".github", true),
        IgnoreDecision::Allowed
    );
}

#[test]
fn test_office_temp_ignored() {
    assert_eq!(
        fs_rules::classify_entry("~$report.docx", false),
        IgnoreDecision::Ignored(IgnoreReason::GlobPattern("~$*".to_string()))
    );
}

#[test]
fn test_word_temp_ignored() {
    // Word temp files like ~WRL0001.tmp are matched by *.tmp
    assert_eq!(
        fs_rules::classify_entry("~WRL0001.tmp", false),
        IgnoreDecision::Ignored(IgnoreReason::GlobPattern("*.tmp".to_string()))
    );
}

#[test]
fn test_tmp_file_ignored() {
    assert_eq!(
        fs_rules::classify_entry("scratch.tmp", false),
        IgnoreDecision::Ignored(IgnoreReason::GlobPattern("*.tmp".to_string()))
    );
}

#[test]
fn test_lnk_file_ignored() {
    assert_eq!(
        fs_rules::classify_entry("shortcut.lnk", false),
        IgnoreDecision::Ignored(IgnoreReason::GlobPattern("*.lnk".to_string()))
    );
}

#[test]
fn test_normal_files_allowed() {
    assert_eq!(
        fs_rules::classify_entry("readme.md", false),
        IgnoreDecision::Allowed
    );
    assert_eq!(
        fs_rules::classify_entry("src", true),
        IgnoreDecision::Allowed
    );
    assert_eq!(
        fs_rules::classify_entry("main.rs", false),
        IgnoreDecision::Allowed
    );
    assert_eq!(
        fs_rules::classify_entry("document.docx", false),
        IgnoreDecision::Allowed
    );
}

// ===== Platform trait tests =====

#[test]
fn test_normalize_relative_path() {
    let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp/test"));
    assert_eq!(platform.normalize_relative_path("a\\b\\c"), "a/b/c");
    assert_eq!(platform.normalize_relative_path("a/b/c"), "a/b/c");
}

#[test]
fn test_validate_target_relative_path() {
    let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp/test"));

    // Valid paths
    assert!(platform.validate_target_relative_path("file.txt").is_ok());
    assert!(platform
        .validate_target_relative_path("dir/file.txt")
        .is_ok());
    assert!(platform.validate_target_relative_path("a/b/c.txt").is_ok());

    // Invalid paths
    assert!(platform.validate_target_relative_path("").is_err());
    assert!(platform.validate_target_relative_path("/absolute").is_err());
    assert!(platform.validate_target_relative_path("../escape").is_err());
    assert!(platform.validate_target_relative_path("a/../b").is_err());
    assert!(platform.validate_target_relative_path("a/\0b").is_err());
}

#[test]
fn test_validate_windows_invalid_chars() {
    let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp/test"));

    assert!(platform.validate_target_relative_path("file<name").is_err());
    assert!(platform.validate_target_relative_path("file>name").is_err());
    assert!(platform.validate_target_relative_path("file:name").is_err());
    assert!(platform
        .validate_target_relative_path("file\"name")
        .is_err());
    assert!(platform.validate_target_relative_path("file|name").is_err());
    assert!(platform.validate_target_relative_path("file?name").is_err());
    assert!(platform.validate_target_relative_path("file*name").is_err());
}

#[test]
fn test_validate_reserved_names() {
    let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp/test"));

    assert!(platform.validate_target_relative_path("CON").is_err());
    assert!(platform.validate_target_relative_path("con").is_err());
    assert!(platform.validate_target_relative_path("con.txt").is_err());
    assert!(platform.validate_target_relative_path("PRN").is_err());
    assert!(platform.validate_target_relative_path("LPT1").is_err());
    assert!(platform.validate_target_relative_path("COM9.log").is_err());
    assert!(platform.validate_target_relative_path("COM10").is_ok()); // COM10 is valid
}

#[test]
fn test_validate_trailing_dot_space() {
    let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp/test"));

    assert!(platform.validate_target_relative_path("file.").is_err());
    assert!(platform.validate_target_relative_path("file ").is_err());
    assert!(platform.validate_target_relative_path("dir./file").is_err());
}

#[test]
fn test_detect_case_collisions() {
    let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp/test"));

    let paths = vec![
        "File.txt".to_string(),
        "file.TXT".to_string(),
        "other.md".to_string(),
    ];

    let collisions = platform.detect_case_collisions(&paths);
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].len(), 2);

    let no_collision = vec!["File.txt".to_string(), "Other.txt".to_string()];
    assert!(platform.detect_case_collisions(&no_collision).is_empty());
}

#[test]
fn test_classify_via_platform_trait() {
    let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp/test"));

    assert_eq!(
        platform.classify_ignored_entry("Thumbs.db", false),
        IgnoreDecision::Ignored(IgnoreReason::ExactName("Thumbs.db".to_string()))
    );
    assert_eq!(
        platform.classify_ignored_entry(".git", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(".git".to_string()))
    );
    assert_eq!(
        platform.classify_ignored_entry(".gitignore", false),
        IgnoreDecision::Allowed
    );
}

// ===== validate_sync_root tests =====

#[test]
fn test_validate_sync_root_accepts_directory() {
    let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp/test"));
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();

    let result = platform.validate_sync_root(&sub);
    assert!(result.is_ok(), "should accept existing directory");
}

#[test]
fn test_validate_sync_root_rejects_file() {
    let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp/test"));
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notadir.txt");
    std::fs::write(&file, "hello").unwrap();

    let result = platform.validate_sync_root(&file);
    assert!(result.is_err(), "should reject file as sync root");
}

#[test]
fn test_validate_sync_root_rejects_nonexistent() {
    let platform = WinPlatform::with_data_dir(PathBuf::from("/tmp/test"));
    let result = platform.validate_sync_root(PathBuf::from("/nonexistent/path/here").as_path());
    assert!(result.is_err(), "should reject non-existent path");
}
