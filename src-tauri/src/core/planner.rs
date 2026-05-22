use crate::core::model::{
    DeviceRole, EntryKind, FileSnapshot, HashStatus, SyncBaseline, SyncDecision,
};

/// Compare current snapshots with baselines to produce sync decisions.
///
/// For primary role: new/changed files and new directories → ApplyToSecondary,
/// deleted entries → MoveSecondaryToHistory.
/// For secondary role: new/changed/deleted files and new directories → MarkPendingReturn.
pub fn plan_sync(
    current_snapshots: &[FileSnapshot],
    baselines: &[SyncBaseline],
    local_role: DeviceRole,
) -> Vec<PlannedAction> {
    use std::collections::HashMap;

    // Index baselines by relative path
    let baseline_map: HashMap<&str, &SyncBaseline> = baselines
        .iter()
        .map(|b| (b.relative_path.as_str(), b))
        .collect();

    // Index current snapshots by relative path
    let snapshot_map: HashMap<&str, &FileSnapshot> = current_snapshots
        .iter()
        .filter(|s| !s.deleted && !s.is_symlink)
        .map(|s| (s.relative_path.as_str(), s))
        .collect();

    let mut actions = Vec::new();

    // Check current snapshots against baselines
    for snap in current_snapshots {
        if snap.deleted || snap.is_symlink {
            continue;
        }

        let decision = match baseline_map.get(snap.relative_path.as_str()) {
            Some(baseline) => {
                // Directories have no stable content hash. Once a directory has
                // a baseline, child entries carry later content changes.
                if snap.kind == EntryKind::Directory {
                    SyncDecision::Noop
                } else if has_changed(snap, baseline, local_role) {
                    match local_role {
                        DeviceRole::Primary => SyncDecision::ApplyToSecondary,
                        DeviceRole::Secondary => SyncDecision::MarkPendingReturn,
                    }
                } else {
                    SyncDecision::Noop
                }
            }
            None => {
                // New file not in baseline
                match local_role {
                    DeviceRole::Primary => SyncDecision::ApplyToSecondary,
                    DeviceRole::Secondary => SyncDecision::MarkPendingReturn,
                }
            }
        };

        if decision != SyncDecision::Noop {
            actions.push(PlannedAction {
                relative_path: snap.relative_path.clone(),
                decision,
                snapshot: Some(snap.clone()),
                baseline: baseline_map
                    .get(snap.relative_path.as_str())
                    .cloned()
                    .cloned(),
            });
        }
    }

    // Check for deletions: files in baseline but not in current snapshots
    for baseline in baselines {
        if snapshot_map.get(baseline.relative_path.as_str()).is_none() {
            let decision = match local_role {
                DeviceRole::Primary => SyncDecision::MoveSecondaryToHistory,
                DeviceRole::Secondary => SyncDecision::MarkPendingReturn,
            };

            if decision != SyncDecision::Noop {
                actions.push(PlannedAction {
                    relative_path: baseline.relative_path.clone(),
                    decision,
                    snapshot: None,
                    baseline: Some(baseline.clone()),
                });
            }
        }
    }

    actions.sort_by(|left, right| match (&left.decision, &right.decision) {
        (SyncDecision::MoveSecondaryToHistory, SyncDecision::MoveSecondaryToHistory) => {
            path_depth(&right.relative_path).cmp(&path_depth(&left.relative_path))
        }
        _ => std::cmp::Ordering::Equal,
    });

    actions
}

fn path_depth(path: &str) -> usize {
    path.split('/').filter(|part| !part.is_empty()).count()
}

/// A planned sync action.
#[derive(Debug, Clone)]
pub struct PlannedAction {
    pub relative_path: String,
    pub decision: SyncDecision,
    pub snapshot: Option<FileSnapshot>,
    pub baseline: Option<SyncBaseline>,
}

/// Check if a file has changed compared to its baseline.
///
/// Hash comparison is authoritative: if both hashes are available and match,
/// the file has NOT changed even if mtime differs.
/// If hashes are unavailable, fall back to size + mtime comparison.
fn has_changed(snapshot: &FileSnapshot, baseline: &SyncBaseline, local_role: DeviceRole) -> bool {
    let (baseline_hash, baseline_hash_status, baseline_size, baseline_modified_unix_ms) =
        match local_role {
            DeviceRole::Primary => (
                &baseline.primary_hash,
                baseline.primary_hash_status,
                baseline.primary_size,
                baseline.primary_modified_unix_ms,
            ),
            DeviceRole::Secondary => (
                &baseline.secondary_hash,
                baseline.secondary_hash_status,
                baseline.secondary_size,
                baseline.secondary_modified_unix_ms,
            ),
        };

    // If both hashes are verified and available, compare hashes
    if snapshot.hash_status == HashStatus::Verified && baseline_hash_status == HashStatus::Verified
    {
        if let (Some(snap_hash), Some(base_hash)) = (&snapshot.blake3_hash, baseline_hash) {
            return snap_hash != base_hash;
        }
    }

    // Fallback: compare size and modified time
    snapshot.size != baseline_size || snapshot.modified_unix_ms != baseline_modified_unix_ms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::*;

    fn make_snapshot(
        path: &str,
        kind: EntryKind,
        size: i64,
        mtime: i64,
        hash: Option<&str>,
        hash_status: HashStatus,
    ) -> FileSnapshot {
        FileSnapshot {
            task_id: uuid::Uuid::nil(),
            relative_path: path.to_string(),
            kind,
            size,
            modified_unix_ms: mtime,
            blake3_hash: hash.map(|s| s.to_string()),
            hash_status,
            deleted: false,
            is_symlink: false,
        }
    }

    fn make_baseline(
        path: &str,
        primary_hash: Option<&str>,
        primary_hash_status: HashStatus,
        primary_size: i64,
        primary_mtime: i64,
        secondary_hash: Option<&str>,
        secondary_mtime: i64,
    ) -> SyncBaseline {
        SyncBaseline {
            task_id: uuid::Uuid::nil(),
            relative_path: path.to_string(),
            primary_hash: primary_hash.map(|s| s.to_string()),
            primary_hash_status,
            primary_size,
            secondary_size: primary_size,
            primary_modified_unix_ms: primary_mtime,
            secondary_hash: secondary_hash.map(|s| s.to_string()),
            secondary_hash_status: primary_hash_status,
            secondary_modified_unix_ms: secondary_mtime,
            last_synced_unix_ms: 1000,
        }
    }

    #[test]
    fn has_changed_different_hash_is_change() {
        let snap = make_snapshot(
            "a.txt",
            EntryKind::File,
            100,
            2000,
            Some("aaa"),
            HashStatus::Verified,
        );
        let baseline = make_baseline(
            "a.txt",
            Some("bbb"),
            HashStatus::Verified,
            100,
            1000,
            Some("bbb"),
            1000,
        );
        assert!(has_changed(&snap, &baseline, DeviceRole::Primary));
    }

    #[test]
    fn has_changed_same_hash_is_not_change_even_if_mtime_differs() {
        let snap = make_snapshot(
            "a.txt",
            EntryKind::File,
            100,
            9999,
            Some("abc"),
            HashStatus::Verified,
        );
        let baseline = make_baseline(
            "a.txt",
            Some("abc"),
            HashStatus::Verified,
            100,
            1000,
            Some("abc"),
            1000,
        );
        assert!(!has_changed(&snap, &baseline, DeviceRole::Primary));
    }

    #[test]
    fn has_changed_fallback_size_differs_no_hash() {
        let snap = make_snapshot(
            "a.txt",
            EntryKind::File,
            200,
            2000,
            None,
            HashStatus::UnverifiedLargeFile,
        );
        let baseline = make_baseline(
            "a.txt",
            None,
            HashStatus::UnverifiedLargeFile,
            100,
            1000,
            None,
            1000,
        );
        assert!(has_changed(&snap, &baseline, DeviceRole::Primary));
    }

    #[test]
    fn has_changed_fallback_mtime_differs_no_hash() {
        let snap = make_snapshot(
            "a.txt",
            EntryKind::File,
            100,
            3000,
            None,
            HashStatus::Unavailable,
        );
        let baseline = make_baseline(
            "a.txt",
            None,
            HashStatus::Unavailable,
            100,
            1000,
            None,
            1000,
        );
        assert!(has_changed(&snap, &baseline, DeviceRole::Primary));
    }

    #[test]
    fn has_changed_secondary_fallback_uses_secondary_size() {
        let snap = make_snapshot(
            "a.txt",
            EntryKind::File,
            200,
            3000,
            None,
            HashStatus::UnverifiedLargeFile,
        );
        let mut baseline = make_baseline(
            "a.txt",
            None,
            HashStatus::UnverifiedLargeFile,
            100,
            1000,
            None,
            3000,
        );
        baseline.secondary_size = 200;

        assert!(!has_changed(&snap, &baseline, DeviceRole::Secondary));
        assert!(has_changed(&snap, &baseline, DeviceRole::Primary));
    }

    #[test]
    fn has_changed_secondary_role_uses_secondary_baseline() {
        let snap = make_snapshot(
            "a.txt",
            EntryKind::File,
            100,
            2000,
            Some("xxx"),
            HashStatus::Verified,
        );
        let baseline = make_baseline(
            "a.txt",
            Some("xxx"),
            HashStatus::Verified,
            100,
            1000,
            Some("yyy"),
            1000,
        );
        // secondary_hash differs → change detected
        assert!(has_changed(&snap, &baseline, DeviceRole::Secondary));
    }

    #[test]
    fn plan_sync_new_primary_file_becomes_apply() {
        let snap = make_snapshot(
            "new.txt",
            EntryKind::File,
            50,
            1000,
            Some("hash1"),
            HashStatus::Verified,
        );
        let actions = plan_sync(&[snap], &[], DeviceRole::Primary);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].decision, SyncDecision::ApplyToSecondary);
    }

    #[test]
    fn plan_sync_new_secondary_file_becomes_pending_return() {
        let snap = make_snapshot(
            "new.txt",
            EntryKind::File,
            50,
            1000,
            Some("hash1"),
            HashStatus::Verified,
        );
        let actions = plan_sync(&[snap], &[], DeviceRole::Secondary);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].decision, SyncDecision::MarkPendingReturn);
    }

    #[test]
    fn plan_sync_unchanged_file_is_noop() {
        let snap = make_snapshot(
            "same.txt",
            EntryKind::File,
            100,
            2000,
            Some("abc"),
            HashStatus::Verified,
        );
        let baseline = make_baseline(
            "same.txt",
            Some("abc"),
            HashStatus::Verified,
            100,
            1000,
            Some("abc"),
            1000,
        );
        let actions = plan_sync(&[snap], &[baseline], DeviceRole::Primary);
        assert!(actions.is_empty());
    }

    #[test]
    fn plan_sync_primary_delete_moves_secondary_to_history() {
        let baseline = make_baseline(
            "gone.txt",
            Some("hash"),
            HashStatus::Verified,
            100,
            1000,
            Some("hash"),
            1000,
        );
        let actions = plan_sync(&[], &[baseline], DeviceRole::Primary);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].decision, SyncDecision::MoveSecondaryToHistory);
    }

    #[test]
    fn plan_sync_secondary_delete_becomes_pending_return() {
        let baseline = make_baseline(
            "gone.txt",
            Some("hash"),
            HashStatus::Verified,
            100,
            1000,
            Some("hash"),
            1000,
        );
        let actions = plan_sync(&[], &[baseline], DeviceRole::Secondary);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].decision, SyncDecision::MarkPendingReturn);
        assert_eq!(actions[0].relative_path, "gone.txt");
        assert!(actions[0].snapshot.is_none());
    }

    #[test]
    fn plan_sync_skips_symlinks() {
        let snap = FileSnapshot {
            task_id: uuid::Uuid::nil(),
            relative_path: "link".to_string(),
            kind: EntryKind::File,
            size: 0,
            modified_unix_ms: 0,
            blake3_hash: None,
            hash_status: HashStatus::Unavailable,
            deleted: false,
            is_symlink: true,
        };
        let actions = plan_sync(&[snap], &[], DeviceRole::Primary);
        assert!(actions.is_empty(), "symlinks should be skipped");
    }
}
