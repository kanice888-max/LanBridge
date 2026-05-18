use lanbridge::platform::macos::fs_rules;
use lanbridge::platform::macos::MacPlatform;
use lanbridge::platform::traits::{IgnoreDecision, IgnoreReason, Platform};
use std::path::PathBuf;

// ===== fs_rules tests =====

#[test]
fn test_ds_store_ignored() {
    assert_eq!(
        fs_rules::classify_entry(".DS_Store", false),
        IgnoreDecision::Ignored(IgnoreReason::ExactName(".DS_Store".to_string()))
    );
}

#[test]
fn test_apple_double_ignored() {
    assert_eq!(
        fs_rules::classify_entry(".AppleDouble", false),
        IgnoreDecision::Ignored(IgnoreReason::ExactName(".AppleDouble".to_string()))
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
fn test_lanbridge_temp_directory_ignored() {
    assert_eq!(
        fs_rules::classify_entry(".lanbridge-temp", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(".lanbridge-temp".to_string()))
    );
}

#[test]
fn test_lanbridge_partial_files_ignored() {
    assert!(matches!(
        fs_rules::classify_entry("photo.jpg.lanbridge-partial", false),
        IgnoreDecision::Ignored(_)
    ));
    assert!(matches!(
        fs_rules::classify_entry("photo.jpg.lanbridge-partial.lanbridge-partial", false),
        IgnoreDecision::Ignored(_)
    ));
}

#[test]
fn test_browser_and_download_temp_files_ignored() {
    assert!(matches!(
        fs_rules::classify_entry("video.mp4.part", false),
        IgnoreDecision::Ignored(_)
    ));
    assert!(matches!(
        fs_rules::classify_entry("archive.zip.crdownload", false),
        IgnoreDecision::Ignored(_)
    ));
    assert!(matches!(
        fs_rules::classify_entry("installer.download", false),
        IgnoreDecision::Ignored(_)
    ));
}

#[test]
fn test_git_file_not_ignored() {
    // .gitignore is NOT ignored by the .git/ rule
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
    // .github is NOT ignored by the .git/ rule
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
fn test_tmp_file_ignored() {
    assert_eq!(
        fs_rules::classify_entry("scratch.tmp", false),
        IgnoreDecision::Ignored(IgnoreReason::GlobPattern("*.tmp".to_string()))
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
}

#[test]
fn test_macos_specific_dirs_ignored() {
    assert_eq!(
        fs_rules::classify_entry(".DocumentRevisions-V100", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(
            ".DocumentRevisions-V100".to_string()
        ))
    );
    assert_eq!(
        fs_rules::classify_entry(".Spotlight-V100", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(".Spotlight-V100".to_string()))
    );
    assert_eq!(
        fs_rules::classify_entry(".TemporaryItems", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(".TemporaryItems".to_string()))
    );
    assert_eq!(
        fs_rules::classify_entry(".Trashes", true),
        IgnoreDecision::Ignored(IgnoreReason::ExactDirectory(".Trashes".to_string()))
    );
}

// ===== Platform trait tests =====

#[test]
fn test_normalize_relative_path() {
    let platform = MacPlatform::with_data_dir(PathBuf::from("/tmp/test"));
    assert_eq!(platform.normalize_relative_path("a\\b\\c"), "a/b/c");
    assert_eq!(platform.normalize_relative_path("a/b/c"), "a/b/c");
}

#[test]
fn test_validate_target_relative_path() {
    let platform = MacPlatform::with_data_dir(PathBuf::from("/tmp/test"));

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
fn test_detect_case_collisions() {
    let platform = MacPlatform::with_data_dir(PathBuf::from("/tmp/test"));

    let paths = vec![
        "File.txt".to_string(),
        "file.TXT".to_string(),
        "other.md".to_string(),
    ];

    let collisions = platform.detect_case_collisions(&paths);
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].len(), 2);

    // No collision when cases differ
    let no_collision = vec!["File.txt".to_string(), "Other.txt".to_string()];
    assert!(platform.detect_case_collisions(&no_collision).is_empty());
}

#[test]
fn test_classify_via_platform_trait() {
    let platform = MacPlatform::with_data_dir(PathBuf::from("/tmp/test"));

    assert_eq!(
        platform.classify_ignored_entry(".DS_Store", false),
        IgnoreDecision::Ignored(IgnoreReason::ExactName(".DS_Store".to_string()))
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
