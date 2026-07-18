use anyhow::{anyhow, Result};
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
        ScanMode::Full { cached_snapshots },
        &mut results,
    )?;
    Ok(results)
}

/// Scan metadata only, without hashing file contents.
///
/// This is used by auto-sync readiness checks where walking the tree is useful
/// but hashing large files would be too expensive.
pub fn scan_root_metadata(sync_root: &Path, platform: &dyn Platform) -> Result<Vec<FileSnapshot>> {
    let mut results = Vec::new();
    walk_dir(
        sync_root,
        sync_root,
        platform,
        ScanMode::Metadata,
        &mut results,
    )?;
    Ok(results.into_iter().map(|result| result.snapshot).collect())
}

/// Result of scanning a single entry.
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub snapshot: FileSnapshot,
    pub skipped_symlink: bool,
}

#[derive(Clone, Copy)]
enum ScanMode<'a> {
    Full {
        cached_snapshots: &'a HashMap<String, FileSnapshot>,
    },
    Metadata,
}

fn walk_dir(
    dir: &Path,
    sync_root: &Path,
    platform: &dyn Platform,
    mode: ScanMode<'_>,
    results: &mut Vec<ScanResult>,
) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).map_err(|error| scan_io_error("read directory", dir, error))?;

    for entry in entries {
        let entry = entry.map_err(|error| scan_io_error("read directory entry", dir, error))?;

        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        let path = entry.path();

        // Check if symlink
        let file_type = entry
            .file_type()
            .map_err(|error| scan_io_error("read file type", &path, error))?;

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
            walk_dir(&path, sync_root, platform, mode, results)?;
        } else {
            let metadata = std::fs::metadata(&path)
                .map_err(|error| scan_io_error("read metadata", &path, error))?;

            let size = metadata.len() as i64;
            let modified_unix_ms = metadata
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;

            // Hash or fallback
            let (hash, hash_status) = match mode {
                ScanMode::Metadata => (None, HashStatus::Unavailable),
                ScanMode::Full { cached_snapshots } => {
                    if size > EAGER_HASH_LIMIT {
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
                    }
                }
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

fn hash_small_file(path: &Path) -> (Option<String>, HashStatus) {
    match hash_file(path) {
        Ok(h) => (Some(h), HashStatus::Verified),
        Err(e) => {
            crate::diagnostics::record_operation(
                "hash_file_failed",
                format!("path={} error={}", path.display(), e),
            );
            tracing::warn!("cannot hash '{}': {}", path.display(), e);
            (None, HashStatus::Unavailable)
        }
    }
}

fn scan_io_error(action: &str, path: &Path, error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        anyhow!(
            "PermissionDenied: cannot {} '{}': {}",
            action,
            path.display(),
            error
        )
    } else {
        anyhow!("cannot {} '{}': {}", action, path.display(), error)
    }
}

/// Compute blake3 hash of a file.
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors_are_structured_for_callers() {
        let error = scan_io_error(
            "read directory",
            Path::new("/protected"),
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );

        assert!(error.to_string().starts_with("PermissionDenied:"));
    }
}
