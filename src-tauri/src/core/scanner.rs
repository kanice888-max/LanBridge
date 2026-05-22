use anyhow::Result;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::model::{EntryKind, FileSnapshot, HashStatus};
use crate::platform::traits::{IgnoreDecision, Platform};

/// Files larger than this use size+mtime fallback instead of hashing.
pub const EAGER_HASH_LIMIT: i64 = 100 * 1024 * 1024; // 100 MB

/// Scan a sync root directory and produce file snapshots.
///
/// Walks the directory tree, applies platform ignore rules,
/// records metadata, and hashes files up to the eager hash limit.
/// Symlinks are skipped and recorded as warnings.
pub fn scan_root(sync_root: &Path, platform: &dyn Platform) -> Result<Vec<ScanResult>> {
    scan_root_with_cache(sync_root, platform, &HashMap::new())
}

/// Scan a sync root, reusing previously verified hashes when metadata matches.
pub fn scan_root_with_cache(
    sync_root: &Path,
    platform: &dyn Platform,
    cached_snapshots: &HashMap<String, FileSnapshot>,
) -> Result<Vec<ScanResult>> {
    let mut results = Vec::new();
    walk_dir(
        sync_root,
        sync_root,
        platform,
        cached_snapshots,
        &mut results,
    )?;
    Ok(results)
}

/// Scan metadata only, without hashing file contents.
///
/// This is used by auto-sync readiness checks where walking the tree is useful
/// but hashing large files would be too expensive.
pub fn scan_root_metadata(sync_root: &Path, platform: &dyn Platform) -> Result<Vec<FileSnapshot>> {
    let mut snapshots = Vec::new();
    walk_dir_metadata(sync_root, sync_root, platform, &mut snapshots)?;
    Ok(snapshots)
}

/// Result of scanning a single entry.
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub snapshot: FileSnapshot,
    pub skipped_symlink: bool,
}

fn walk_dir(
    dir: &Path,
    sync_root: &Path,
    platform: &dyn Platform,
    cached_snapshots: &HashMap<String, FileSnapshot>,
    results: &mut Vec<ScanResult>,
) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("cannot read directory '{}': {}", dir.display(), e);
            return Ok(());
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("cannot read directory entry: {}", e);
                continue;
            }
        };

        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        let path = entry.path();

        // Check if symlink
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                tracing::warn!("cannot get file type for '{}': {}", path.display(), e);
                continue;
            }
        };

        if file_type.is_symlink() {
            results.push(ScanResult {
                snapshot: FileSnapshot {
                    task_id: uuid::Uuid::nil(),
                    relative_path: relative_path(sync_root, &path),
                    kind: if file_type.is_dir() {
                        EntryKind::Directory
                    } else {
                        EntryKind::File
                    },
                    size: 0,
                    modified_unix_ms: 0,
                    blake3_hash: None,
                    hash_status: HashStatus::Unavailable,
                    deleted: false,
                    is_symlink: true,
                },
                skipped_symlink: true,
            });
            continue;
        }

        let is_dir = file_type.is_dir();

        // Apply ignore rules
        if let IgnoreDecision::Ignored(_) = platform.classify_ignored_entry(&name_str, is_dir) {
            continue;
        }

        let rel_path = relative_path(sync_root, &path);

        if is_dir {
            results.push(ScanResult {
                snapshot: FileSnapshot {
                    task_id: uuid::Uuid::nil(),
                    relative_path: rel_path,
                    kind: EntryKind::Directory,
                    size: 0,
                    modified_unix_ms: 0,
                    blake3_hash: None,
                    hash_status: HashStatus::Unavailable,
                    deleted: false,
                    is_symlink: false,
                },
                skipped_symlink: false,
            });
            // Recurse into subdirectory
            walk_dir(&path, sync_root, platform, cached_snapshots, results)?;
        } else {
            // File: get metadata
            let metadata = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("cannot read metadata for '{}': {}", path.display(), e);
                    continue;
                }
            };

            let size = metadata.len() as i64;
            let modified_unix_ms = metadata
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;

            // Hash or fallback
            let (hash, hash_status) = if size > EAGER_HASH_LIMIT {
                (None, HashStatus::UnverifiedLargeFile)
            } else if let Some(cached) = cached_snapshots.get(&rel_path) {
                if cached.kind == EntryKind::File
                    && cached.size == size
                    && cached.modified_unix_ms == modified_unix_ms
                    && cached.hash_status == HashStatus::Verified
                    && cached.blake3_hash.is_some()
                {
                    (cached.blake3_hash.clone(), HashStatus::Verified)
                } else {
                    hash_small_file(&path)
                }
            } else {
                hash_small_file(&path)
            };

            results.push(ScanResult {
                snapshot: FileSnapshot {
                    task_id: uuid::Uuid::nil(),
                    relative_path: rel_path,
                    kind: EntryKind::File,
                    size,
                    modified_unix_ms,
                    blake3_hash: hash,
                    hash_status,
                    deleted: false,
                    is_symlink: false,
                },
                skipped_symlink: false,
            });
        }
    }

    Ok(())
}

fn walk_dir_metadata(
    dir: &Path,
    sync_root: &Path,
    platform: &dyn Platform,
    snapshots: &mut Vec<FileSnapshot>,
) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("cannot read directory '{}': {}", dir.display(), e);
            return Ok(());
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("cannot read directory entry: {}", e);
                continue;
            }
        };

        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                tracing::warn!("cannot get file type for '{}': {}", path.display(), e);
                continue;
            }
        };
        let is_dir = file_type.is_dir();
        if file_type.is_symlink() {
            snapshots.push(FileSnapshot {
                task_id: uuid::Uuid::nil(),
                relative_path: relative_path(sync_root, &path),
                kind: if is_dir {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
                size: 0,
                modified_unix_ms: 0,
                blake3_hash: None,
                hash_status: HashStatus::Unavailable,
                deleted: false,
                is_symlink: true,
            });
            continue;
        }
        if let IgnoreDecision::Ignored(_) = platform.classify_ignored_entry(&name_str, is_dir) {
            continue;
        }

        let rel_path = relative_path(sync_root, &path);
        if is_dir {
            snapshots.push(FileSnapshot {
                task_id: uuid::Uuid::nil(),
                relative_path: rel_path,
                kind: EntryKind::Directory,
                size: 0,
                modified_unix_ms: 0,
                blake3_hash: None,
                hash_status: HashStatus::Unavailable,
                deleted: false,
                is_symlink: false,
            });
            walk_dir_metadata(&path, sync_root, platform, snapshots)?;
        } else {
            let metadata = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("cannot read metadata for '{}': {}", path.display(), e);
                    continue;
                }
            };
            snapshots.push(FileSnapshot {
                task_id: uuid::Uuid::nil(),
                relative_path: rel_path,
                kind: EntryKind::File,
                size: metadata.len() as i64,
                modified_unix_ms: metadata
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH)
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
                blake3_hash: None,
                hash_status: HashStatus::Unavailable,
                deleted: false,
                is_symlink: false,
            });
        }
    }

    Ok(())
}

fn hash_small_file(path: &Path) -> (Option<String>, HashStatus) {
    match hash_file(path) {
        Ok(h) => (Some(h), HashStatus::Verified),
        Err(e) => {
            tracing::warn!("cannot hash '{}': {}", path.display(), e);
            (None, HashStatus::Unavailable)
        }
    }
}

/// Compute blake3 hash of a file.
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Compute the relative path from sync_root to path, using forward slashes.
fn relative_path(sync_root: &Path, path: &Path) -> String {
    path.strip_prefix(sync_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
