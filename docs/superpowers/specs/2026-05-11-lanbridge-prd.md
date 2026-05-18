# LanBridge PRD

## 1. Product Summary

Build a cross-platform desktop app for macOS and Windows that syncs configured folders across two devices on the same LAN. The first version uses a fixed primary-secondary model instead of fully automatic bidirectional sync.

The primary device is the authority. Primary changes sync automatically to the secondary device. Secondary changes are detected and shown as pending return changes. The user must explicitly click a return-sync action before secondary changes are copied back to the primary device.

This product must prioritize data safety over "magic" automation. It should avoid silent overwrite, silent permanent deletion, and ambiguous conflict resolution.

## 2. Target Users

- Users who own both a Mac and a Windows PC and need local-network folder synchronization.
- Users who do not want cloud sync for privacy, speed, or cost reasons.
- Users who can tolerate both computers being online on the same LAN.
- Users who prefer explicit confirmation before secondary-side changes modify the primary copy.

## 3. Product Goals

### P0 Goals

- Pair one Mac client with one Windows client on the same LAN.
- Configure one folder sync task between the two devices.
- Support manual IP connection for P0 pairing and transport setup.
- Automatically sync primary-side create, update, and delete events to the secondary side.
- Put secondary-side files deleted by primary-side deletion into a sync trash/history area instead of permanently deleting them immediately.
- Detect secondary-side create and update events without automatically changing the primary side.
- Let the user manually return-sync secondary-side create and update events to the primary side.
- Detect return-sync conflicts when the primary-side file changed after the last successful primary-to-secondary sync.
- Require explicit confirmation before overwriting a changed primary-side file.
- Back up the old primary-side file before any user-approved overwrite.
- Provide a UI for pairing, task setup, sync status, pending return changes, conflict confirmation, history/trash recovery, and logs.

### P1 Goals

- Support multiple folder sync tasks between the paired Mac and Windows devices.
- Support UDP LAN discovery with manual IP fallback.
- Support resumable large-file transfer.
- Support ignore rules for system junk and user-defined patterns.
- Support basic version history retention settings, such as 7 days, 30 days, or size cap.
- Support pause/resume per sync task.

### P2 Goals

- Support more than two paired devices while still limiting each sync task to a clear primary authority.
- Support bandwidth throttling.
- Support richer diff preview before return-sync overwrite.
- Support advanced diagnostics export.

## 4. Non-Goals For V1

- No cloud sync.
- No WAN/NAT traversal.
- No mobile clients.
- No fully automatic bidirectional sync.
- No merge logic for structured file formats.
- No guarantee of safe live sync for databases, VM images, mail stores, browser profiles, or files actively written by another application.
- No ACL/permission synchronization beyond normal file content and basic metadata.
- No SMB dependency as the main sync architecture.

## 5. Core Sync Model

Each sync task has:

- A primary device.
- A secondary device.
- A primary folder path.
- A secondary folder path.
- A fixed sync direction: primary to secondary.
- A manual return direction: secondary to primary.

The primary-secondary direction can be switched only while both configured folders are empty after applying platform ignore rules. "Empty" means both folders contain no non-ignored files and no non-ignored directory entries. Ignored entries such as `.DS_Store`, `Thumbs.db`, and `.lanbridge-history` do not block direction switching.

## 6. Sync Rules

### Primary To Secondary

- Primary file created: copy to secondary.
- Primary file modified: copy to secondary.
- Primary file deleted: move corresponding secondary file into the secondary sync trash/history area.
- Primary directory created: create on secondary.
- Primary directory deleted: move corresponding secondary directory contents into the secondary sync trash/history area.

### Secondary To Primary

- Secondary file created: mark as pending return-sync.
- Secondary file modified: mark as pending return-sync.
- Secondary file deleted: mark as a pending return/delete request; do not affect primary automatically.
- Manual return-sync: copy selected secondary creates/updates to primary, or send selected delete requests to primary.
- Manual return-delete must be explicit. Primary receive side moves deleted files into history instead of permanently deleting them.

### Conflict Rule

A return-sync conflict exists when:

- The secondary has a pending create/update for a relative path, and
- The primary version for the same relative path changed after the last successful primary-to-secondary sync baseline.

Default conflict behavior:

- Do not overwrite automatically.
- Show a conflict modal with path, primary modified time, secondary modified time, size, and available actions.
- If the user chooses overwrite, first move the current primary file to primary sync history.
- Then write the secondary file to the primary path.

### Delete Safety Rule

No synchronized delete operation may permanently delete user data immediately. Deletes must move files into a managed sync trash/history area first.

### File Identity And Conflict Detection

File identity uses content hashing with blake3 in addition to file size and modification time. Modification time must never be the only input for conflict detection.

When a content hash is available, hash comparison is authoritative. If modified time changes but the content hash is unchanged, the app must not treat that as a content conflict. If a content hash is unavailable for a large file, the app may fall back to size and modified time, but the UI/logs must mark that decision as hash-unverified.

### Operation Atomicity

P0 guarantees file-level atomicity, not directory-level transactions. Each file create, update, delete-to-history, return-sync, and overwrite-backup operation succeeds or fails independently. If a directory operation contains many files and fails partway through, already completed file operations remain completed, failed files are recorded as visible errors, and the app does not mark the entire directory operation as fully successful.

### P0 History Retention

P0 must prevent unbounded history growth. The default history safety policy is 30 days or 1 GB per sync task, whichever limit is reached first. If automatic cleanup is not implemented in P0, the app must at minimum warn before the limit is exceeded and block additional destructive sync operations that would require history storage until the user frees space or changes retention settings in a later version.

When the history limit is reached, only destructive operations that require new history storage are blocked, such as delete-to-history and overwrite-backup. Create and update operations that do not require history storage may continue. The UI must show a clear "history storage full" warning and provide a P0 cleanup entry point, such as deleting history entries older than 30 days or opening the history folder.

History restore defaults to restoring the file to its original relative path. If the original path is occupied, the app must not overwrite it; it must restore to a timestamped conflict-safe name such as `name (restored 2026-05-11 143000).ext`.

### Log Retention

P0 must prevent unbounded event log growth. The app should retain the latest 10,000 log entries or 7 days of logs, whichever keeps fewer entries.

### Symlink Policy

P0 does not follow or synchronize symlinks. Symlink entries must be skipped and recorded as warnings. Path validation must ensure canonicalized paths remain inside the configured sync root to prevent symlink escape.

### Directory Rename And Move Policy

P0 does not detect directory rename or move operations as a special operation. A renamed directory is treated as deletion of the old tree and creation of the new tree, with file-level safety and history rules applied independently to each affected file.

### Unsafe Content Warning Policy

During task setup and scan summaries, the UI must warn when obvious high-risk content is detected, including `.git`, `node_modules`, virtual machine files such as `.vmdk` or `.vmwarevm`, and common database files. P0 may default-ignore high-risk directories such as `.git` and `node_modules` rather than attempting to synchronize them.

The `.git/` ignore rule applies only to an exact directory named `.git`; files such as `.gitignore` and `.gitmodules`, and directories such as `.github/`, are treated as normal project content unless another rule excludes them.

Windows shortcut files (`*.lnk`) are ignored by default because their targets usually point to machine-local paths that are invalid or misleading on the peer device. The app must record a warning when shortcuts are skipped.

## 7. File Support Policy

The app transfers arbitrary file bytes and does not restrict file extensions.

However, the UI and documentation must warn that the following categories are not guaranteed safe for real-time synchronization:

- Databases and database directories.
- Virtual machine images.
- Browser profiles.
- Mail stores.
- Large monolithic project caches.
- Files currently being written by another program.
- Dependency folders such as `node_modules`.
- Git repositories with active concurrent operations.

## 8. Pairing And Security

### Requirements

- LAN communication must be encrypted.
- Devices must have persistent local device identities.
- Pairing must require user confirmation on both devices.
- Pairing must prevent accidental connection to an unknown device on the same LAN.
- After pairing, each device must pin the peer identity.
- The pairing verification code is the MITM protection for the initial unpinned pairing connection. If the codes do not match, both users must reject pairing and investigate.

### Recommended V1 Pairing Flow

1. User opens pairing screen on both devices.
2. One device starts "Create Pairing Code".
3. The other device discovers it by LAN broadcast or accepts manual IP input.
4. Both apps show the same short verification code.
5. User confirms the code on both devices.
6. Apps exchange and store pinned device identities.
7. Future connections only trust the pinned peer identity.

## 9. UI Requirements

### Main Screens

- Welcome/setup screen.
- Pair device screen.
- Create sync task screen.
- Dashboard.
- Task detail screen.
- Pending return changes screen.
- Conflict modal.
- Sync trash/history screen.
- Logs/diagnostics screen.
- Settings screen.

### Dashboard Must Show

- Peer device connection state.
- Active sync tasks.
- Current sync direction.
- Last successful sync time.
- Number of pending return changes.
- Number of conflicts.
- Number of errors.
- Current transfer progress.

### Return-Sync Screen Must Show

- List of pending secondary-side creates and updates.
- File path, size, modified time, and status.
- Select all / select individual files.
- "Return sync selected" button.
- Conflict summary before overwriting anything.

## 10. Error Handling Requirements

The app must make errors visible and actionable. It must not silently skip files.

Required error categories:

- Peer offline.
- Folder missing.
- Permission denied.
- Disk full.
- File locked or still changing.
- Hash mismatch after transfer.
- Unsupported path name on target OS.
- Case-only path collision on a case-insensitive filesystem.
- Network interrupted.
- Conflict requires user decision.
- History/trash retention limit reached.

## 11. Acceptance Criteria

### P0 Acceptance

- A Mac and Windows app can pair over LAN with encrypted communication.
- A user can configure one sync task with a fixed primary and secondary folder.
- Creating a file on primary copies it to secondary.
- Editing a file on primary updates secondary.
- Deleting a file on primary moves the secondary copy into sync trash/history.
- Creating a file on secondary marks it as pending return-sync.
- Editing a file on secondary marks it as pending return-sync.
- Deleting a file on secondary creates an explicit pending delete request and does not delete the primary file automatically.
- Clicking return-sync copies selected secondary creates/updates to primary.
- Clicking return-sync on a selected delete request moves the primary copy into history only after user action.
- If primary changed since baseline, return-sync shows a conflict modal.
- If the user confirms overwrite, the old primary file is saved to history before replacement.
- Sync state survives app restart.
- The UI shows connection, sync, pending return, conflict, and error status.

## 12. Critical Product Risks

- Users may misunderstand "sync" and expect full bidirectional automation.
- Users may configure the wrong folder and propagate deletes.
- LAN discovery may fail on networks with firewall, AP isolation, or VPN.
- File watchers may miss events; scanning is required as a fallback.
- Large files and many small files may expose performance issues early.
- Cross-platform path rules may produce invalid target paths.
- Conflict prompts may annoy users if too frequent.

## 13. Product Guardrails

- Prefer preserving extra copies over overwriting user data.
- Prefer explicit user action over hidden automation when primary-side data may change.
- Never treat modification time as the only source of truth.
- Never permanently delete synchronized data immediately.
- Always make pending destructive actions visible in UI.
