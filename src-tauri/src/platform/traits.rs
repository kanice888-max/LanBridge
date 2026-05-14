use anyhow::Result;
use std::path::{Path, PathBuf};

/// Classification of an ignored entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoreReason {
    /// Exact file name match (e.g., `.DS_Store`).
    ExactName(String),
    /// Exact directory name match (e.g., `.git/`, `node_modules/`).
    ExactDirectory(String),
    /// Glob pattern match (e.g., `*.tmp`, `~$*`).
    GlobPattern(String),
}

/// Result of checking whether an entry should be ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoreDecision {
    /// Entry should be skipped.
    Ignored(IgnoreReason),
    /// Entry should be processed.
    Allowed,
}

/// Shared platform abstraction trait.
///
/// Both macOS and Windows must implement this interface.
/// Shared core modules depend on this trait, not on platform-specific code.
pub trait Platform: Send + Sync {
    /// Return the application data directory for storing state, keys, logs.
    fn app_data_dir(&self) -> Result<PathBuf>;

    /// Return the path to the SQLite database file.
    fn database_path(&self) -> Result<PathBuf>;

    /// Return the path to the device identity key file.
    fn identity_key_path(&self) -> Result<PathBuf>;

    /// Return the path to the peer pins file.
    fn peer_pins_path(&self) -> Result<PathBuf>;

    /// Return the path to the log file.
    fn log_path(&self) -> Result<PathBuf>;

    /// Normalize a relative path to use forward slashes.
    fn normalize_relative_path(&self, path: &str) -> String;

    /// Validate that a sync root path is acceptable for this platform.
    /// Returns the canonicalized path on success.
    fn validate_sync_root(&self, path: &Path) -> Result<PathBuf>;

    /// Validate that a target relative path is valid on this platform.
    /// Checks for invalid characters, reserved names, path length, etc.
    fn validate_target_relative_path(&self, relative_path: &str) -> Result<()>;

    /// Classify whether a directory entry should be ignored.
    /// `entry_name` is the file/directory name (not full path).
    /// `is_dir` indicates whether the entry is a directory.
    fn classify_ignored_entry(&self, entry_name: &str, is_dir: bool) -> IgnoreDecision;

    /// Detect case-only collisions in a set of relative paths.
    /// Returns list of colliding path groups.
    fn detect_case_collisions(&self, paths: &[String]) -> Vec<Vec<String>>;
}
