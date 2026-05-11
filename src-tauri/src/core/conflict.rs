use crate::core::model::{FileSnapshot, HashStatus, PendingReturnChange, SyncBaseline};

/// Result of conflict detection.
#[derive(Debug, Clone)]
pub enum ConflictResult {
    /// No conflict: safe to return-sync.
    NoConflict,
    /// Conflict detected: primary file changed since last sync.
    Conflict {
        relative_path: String,
        primary_hash: Option<String>,
        primary_hash_status: HashStatus,
        primary_modified_unix_ms: i64,
        secondary_hash: Option<String>,
        secondary_hash_status: HashStatus,
        secondary_modified_unix_ms: i64,
        hash_unverified: bool,
    },
}

/// Detect whether a pending return-sync change conflicts with the current primary state.
///
/// A conflict exists when:
/// - The secondary has a pending create/update for a relative path, AND
/// - The primary version for the same path changed after the last successful sync baseline.
///
/// Hash comparison is authoritative: if mtime changes but hash is identical, NOT a conflict.
pub fn detect_conflict(
    pending: &PendingReturnChange,
    current_primary: Option<&FileSnapshot>,
    baseline: Option<&SyncBaseline>,
) -> ConflictResult {
    // If no baseline, this is a new file from secondary — no conflict
    let baseline = match baseline {
        Some(b) => b,
        None => return ConflictResult::NoConflict,
    };

    // If primary file doesn't currently exist, no conflict
    let primary = match current_primary {
        Some(p) if !p.deleted => p,
        _ => return ConflictResult::NoConflict,
    };

    // Check if primary changed since baseline
    let primary_changed = has_primary_changed_since_baseline(primary, baseline);

    if !primary_changed {
        return ConflictResult::NoConflict;
    }

    // Primary has changed since last sync — conflict
    ConflictResult::Conflict {
        relative_path: pending.relative_path.clone(),
        primary_hash: primary.blake3_hash.clone(),
        primary_hash_status: primary.hash_status,
        primary_modified_unix_ms: primary.modified_unix_ms,
        secondary_hash: pending.secondary_hash.clone(),
        secondary_hash_status: pending.secondary_hash_status,
        secondary_modified_unix_ms: pending.secondary_modified_unix_ms,
        hash_unverified: primary.hash_status != HashStatus::Verified
            || pending.secondary_hash_status != HashStatus::Verified,
    }
}

/// Check if the primary file has changed since the baseline.
///
/// Hash comparison is authoritative. If both hashes are verified and identical,
/// mtime changes alone do NOT constitute a change.
fn has_primary_changed_since_baseline(primary: &FileSnapshot, baseline: &SyncBaseline) -> bool {
    // If both hashes are verified, compare hashes
    if primary.hash_status == HashStatus::Verified
        && baseline.primary_hash_status == HashStatus::Verified
    {
        if let (Some(ph), Some(bh)) = (&primary.blake3_hash, &baseline.primary_hash) {
            return ph != bh;
        }
    }

    // Fallback: size or mtime differs
    primary.size != baseline.primary_modified_unix_ms as i64
        || primary.modified_unix_ms != baseline.primary_modified_unix_ms
}

/// Generate a conflict-safe filename for KeepBoth resolution.
///
/// Format: `<stem> (conflict from <device-name> <YYYY-MM-DD HHmmss>)<extension>`
/// Appends `-2`, `-3`, etc. if the name already exists.
pub fn conflict_filename(
    original_path: &str,
    device_name: &str,
    timestamp_unix_ms: i64,
    mut path_exists: impl FnMut(&str) -> bool,
) -> String {
    let dt = chrono::DateTime::from_timestamp_millis(timestamp_unix_ms)
        .unwrap_or_default()
        .naive_utc();
    let timestamp_str = dt.format("%Y-%m-%d %H%M%S").to_string();

    let (stem, ext) = split_stem_ext(original_path);

    let mut candidate = format!(
        "{} (conflict from {} {}){}",
        stem, device_name, timestamp_str, ext
    );

    if !path_exists(&candidate) {
        return candidate;
    }

    // Try -2, -3, etc.
    for i in 2.. {
        candidate = format!(
            "{} (conflict from {} {})-{}{}",
            stem, device_name, timestamp_str, i, ext
        );
        if !path_exists(&candidate) {
            return candidate;
        }
    }

    candidate
}

/// Split a filename into stem and extension.
fn split_stem_ext(path: &str) -> (&str, &str) {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(pos) if pos > 0 => {
            let dot_pos = path.len() - (name.len() - pos);
            (&path[..dot_pos], &path[dot_pos..])
        }
        _ => (path, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conflict_filename_basic() {
        let result = conflict_filename("doc.txt", "MacBook", 1715400000000, |_| false);
        assert!(result.starts_with("doc (conflict from MacBook"));
        assert!(result.ends_with(").txt"));
    }

    #[test]
    fn test_conflict_filename_no_ext() {
        let result = conflict_filename("Makefile", "PC", 1715400000000, |_| false);
        assert!(result.starts_with("Makefile (conflict from PC"));
    }

    #[test]
    fn test_conflict_filename_collision() {
        let mut count = 0;
        let result = conflict_filename("file.txt", "Mac", 1715400000000, |_| {
            count += 1;
            count <= 2 // First two names "exist"
        });
        assert!(result.contains("-3") || result.contains("-2"));
    }

    #[test]
    fn test_split_stem_ext() {
        assert_eq!(split_stem_ext("file.txt"), ("file", ".txt"));
        assert_eq!(split_stem_ext("archive.tar.gz"), ("archive.tar", ".gz"));
        assert_eq!(split_stem_ext("noext"), ("noext", ""));
        assert_eq!(split_stem_ext("dir/file.txt"), ("dir/file", ".txt"));
    }
}
