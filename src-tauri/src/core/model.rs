use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Device role in a sync task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceRole {
    Primary,
    Secondary,
}

/// Entry type on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    File,
    Directory,
}

/// Type of change detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    Created,
    Modified,
    Deleted,
}

/// Hash verification status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HashStatus {
    Verified,
    UnverifiedLargeFile,
    Unavailable,
}

/// Planner output for each file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncDecision {
    /// Copy primary file to secondary.
    ApplyToSecondary,
    /// Move secondary file to history (primary delete).
    MoveSecondaryToHistory,
    /// Mark secondary change as pending return-sync.
    MarkPendingReturn,
    /// Conflict: user must decide.
    RequireConflictDecision,
    /// Keep both files (conflict resolution).
    KeepBoth,
    /// No action needed.
    Noop,
}

/// A configured sync task between two devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTask {
    pub id: Uuid,
    pub name: String,
    pub primary_device_id: String,
    pub secondary_device_id: String,
    pub local_path: String,
    pub remote_path: String,
    pub local_role: DeviceRole,
    pub enabled: bool,
    pub created_unix_ms: i64,
    pub updated_unix_ms: i64,
}

/// Snapshot of a file or directory at scan time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub task_id: Uuid,
    pub relative_path: String,
    pub kind: EntryKind,
    pub size: i64,
    pub modified_unix_ms: i64,
    pub blake3_hash: Option<String>,
    pub hash_status: HashStatus,
    pub deleted: bool,
    pub is_symlink: bool,
}

/// Baseline state for a synced file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncBaseline {
    pub task_id: Uuid,
    pub relative_path: String,
    pub primary_hash: Option<String>,
    pub primary_hash_status: HashStatus,
    pub primary_size: i64,
    pub primary_modified_unix_ms: i64,
    pub secondary_hash: Option<String>,
    pub secondary_hash_status: HashStatus,
    pub secondary_modified_unix_ms: i64,
    pub last_synced_unix_ms: i64,
}

/// A pending change on the secondary side waiting for return-sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingReturnChange {
    pub task_id: Uuid,
    pub relative_path: String,
    pub change_kind: ChangeKind,
    pub secondary_hash: Option<String>,
    pub secondary_hash_status: HashStatus,
    pub secondary_modified_unix_ms: i64,
    pub created_unix_ms: i64,
}

/// A file moved to history (trash or overwritten backup).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: Uuid,
    pub task_id: Uuid,
    pub original_relative_path: String,
    pub stored_path: String,
    pub reason: HistoryReason,
    pub created_unix_ms: i64,
    pub size: i64,
}

/// Reason a file was moved to history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryReason {
    /// Primary deleted the file, secondary copy moved to trash.
    Trash,
    /// Old primary file backed up before overwrite.
    Overwritten,
}

/// An event log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: Option<i64>,
    pub level: LogLevel,
    pub task_id: Option<Uuid>,
    pub relative_path: Option<String>,
    pub message: String,
    pub created_unix_ms: i64,
}

/// Log severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// A paired device identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_id: String,
    pub display_name: String,
    pub public_key: Vec<u8>,
    pub last_seen_unix_ms: i64,
    pub trusted: bool,
    pub last_address: Option<String>,
}

/// Application error types.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
pub enum AppError {
    #[error("peer is offline")]
    PeerOffline,

    #[error("folder missing: {0}")]
    FolderMissing(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("disk full")]
    DiskFull,

    #[error("file locked: {0}")]
    FileLocked(String),

    #[error("hash mismatch for {0}")]
    HashMismatch(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("case collision: {0}")]
    CaseCollision(String),

    #[error("network interrupted")]
    NetworkInterrupted,

    #[error("conflict requires user decision for {0}")]
    ConflictRequired(String),

    #[error("history storage limit reached for task {0}")]
    HistoryLimitReached(String),
}
