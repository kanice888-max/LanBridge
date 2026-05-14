use crate::core::model::{DeviceRole, FileSnapshot, HashStatus, SyncBaseline, SyncDecision};

/// Compare current snapshots with baselines to produce sync decisions.
///
/// For primary role: new/changed files → ApplyToSecondary, deleted → MoveSecondaryToHistory.
/// For secondary role: new/changed files → MarkPendingReturn, deleted → Noop.
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
                // File exists in baseline: check if changed
                if has_changed(snap, baseline, local_role) {
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
                DeviceRole::Secondary => SyncDecision::Noop, // Secondary delete doesn't affect primary
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

    actions
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
    let (baseline_hash, baseline_hash_status, baseline_modified_unix_ms) = match local_role {
        DeviceRole::Primary => (
            &baseline.primary_hash,
            baseline.primary_hash_status,
            baseline.primary_modified_unix_ms,
        ),
        DeviceRole::Secondary => (
            &baseline.secondary_hash,
            baseline.secondary_hash_status,
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
    snapshot.size != baseline.primary_size || snapshot.modified_unix_ms != baseline_modified_unix_ms
}
