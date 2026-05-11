# LAN Folder Sync macOS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first working macOS client and shared sync foundation for the LAN folder sync app.

**Architecture:** Implement the shared Rust/Tauri engine while targeting macOS first. macOS establishes domain models, SQLite state, scanner, history/trash behavior, encrypted pairing, manual IP transport, React UI, and macOS-specific filesystem/watcher/menu-bar behavior.

**Tech Stack:** Rust, Tauri, React, TypeScript, SQLite, Tokio, notify on macOS/FSEvents, blake3, tokio-rustls, Ed25519 identity, Vitest, Rust tests, Playwright.

---

## 1. Worktree

All macOS work happens in:

- Path: `.worktrees/macos`
- Branch: `codex/lan-sync-macos`

The macOS worker owns the first implementation of shared modules because macOS is built first.

## 2. macOS-Specific Requirements

- Use app data directory for identity, SQLite state, logs, and peer pins.
- Use a per-sync-root `.lan-sync-history/` directory for user-restorable trash and overwritten files.
- Use `notify` with macOS FSEvents support for live watching.
- Use scanner fallback because FSEvents may coalesce or miss details after sleep.
- Ignore `.DS_Store`, `.AppleDouble`, `.DocumentRevisions-V100`, `.Spotlight-V100`, `.TemporaryItems`, `.Trashes`, exact directory `.git/`, exact directory `node_modules/`, and `.lan-sync-history/` by default. `.gitignore`, `.gitmodules`, and `.github/` are not ignored by the `.git/` rule.
- Do not follow or synchronize symlinks in P0. Record skipped symlinks as warnings.
- Preserve file contents and basic modified time; do not attempt full macOS metadata/resource fork sync in P0.
- Show a menu bar/tray entry with open, pause all, sync now, and quit.
- Handle macOS file access permission errors with clear UI copy.
- Prepare for signing/notarization later, but P0 can use development builds.

## 3. Target File Structure

```text
src-tauri/src/
├── app_state.rs
├── commands.rs
├── core/
├── history/
├── pairing/
├── platform/
│   ├── mod.rs
│   ├── traits.rs
│   └── macos/
│       ├── mod.rs
│       ├── fs_rules.rs
│       ├── watcher.rs
│       ├── app_dirs.rs
│       └── tray.rs
├── state/
└── transport/
```

## 4. P0 macOS Task Plan

### Task 1: Scaffold Project On macOS

**Files:**
- Create: `README.md`
- Create: Tauri + React scaffold

- [ ] **Step 1: Verify worktree**

Run:

```bash
git branch --show-current
```

Expected: `codex/lan-sync-macos`.

- [ ] **Step 2: Verify root ignores**

Confirm root `.gitignore` already contains:

```gitignore
.worktrees/
node_modules/
dist/
target/
src-tauri/target/
.DS_Store
Thumbs.db
```

- [ ] **Step 3: Scaffold Tauri React app into an existing docs repo**

The macOS worktree already contains committed docs, so a generator may refuse to write into `.`. Prefer the generator's force/existing-directory option if available. If it is not available, scaffold into a temporary sibling directory, then copy only generated project files into the worktree without deleting `docs/`, `AGENTS.md`, or `CLAUDE.md`.

Preferred command when supported:

```bash
npm create tauri-app@latest . -- --template react-ts --force
```

Expected: project contains `src/`, `src-tauri/`, `package.json`, and `src-tauri/Cargo.toml`.

If the generator reports that `--force` is unsupported or refuses the non-empty directory, use this fallback:

```bash
npm create tauri-app@latest __tmp_tauri_scaffold -- --template react-ts
cp -R __tmp_tauri_scaffold/src __tmp_tauri_scaffold/src-tauri __tmp_tauri_scaffold/package.json .
cp -R __tmp_tauri_scaffold/package-lock.json . 2>/dev/null || true
rm -rf __tmp_tauri_scaffold
```

Expected after either path: existing docs remain intact and project contains `src/`, `src-tauri/`, `package.json`, and `src-tauri/Cargo.toml`.

- [ ] **Step 4: Install dependencies**

Run:

```bash
npm install
```

Expected: dependencies install without errors.

- [ ] **Step 5: Commit scaffold**

```bash
git add -A
git commit -m "chore: scaffold macOS LAN sync baseline"
```

### Task 2: Implement Shared Domain Model And State Database

**Files:**
- Create: `src-tauri/src/core/model.rs`
- Create: `src-tauri/src/core/mod.rs`
- Create: `src-tauri/src/state/db.rs`
- Create: `src-tauri/src/state/migrations.rs`
- Create: `src-tauri/src/state/repository.rs`
- Create: `src-tauri/src/state/mod.rs`
- Test: `src-tauri/tests/core/state_repository_test.rs`

- [ ] **Step 1: Add Rust dependencies**

Add to `src-tauri/Cargo.toml`:

```toml
anyhow = "1"
blake3 = "1"
chrono = { version = "0.4", features = ["serde"] }
notify = "6"
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
tokio = { version = "1", features = ["full"] }
uuid = { version = "1", features = ["v4", "serde"] }
```

- [ ] **Step 2: Define model types**

Create types for `DeviceRole`, `EntryKind`, `ChangeKind`, `SyncDecision`, `SyncTask`, `FileSnapshot`, `SyncBaseline`, `PendingReturnChange`, `HistoryEntry`, `AppError`, and `LogEntry`.

Required fields:

- `SyncTask`: `id`, `name`, `primary_device_id`, `secondary_device_id`, `local_path`, `remote_path`, `local_role`, `enabled`, `created_unix_ms`, `updated_unix_ms`.
- `FileSnapshot`: `task_id`, `relative_path`, `kind`, `size`, `modified_unix_ms`, `blake3_hash`, `hash_status`, `deleted`, `is_symlink`.
- `SyncBaseline`: `task_id`, `relative_path`, `primary_hash`, `primary_hash_status`, `primary_modified_unix_ms`, `secondary_hash`, `secondary_hash_status`, `secondary_modified_unix_ms`, `last_synced_unix_ms`.
- `PendingReturnChange`: `task_id`, `relative_path`, `change_kind`, `secondary_hash`, `secondary_hash_status`, `secondary_modified_unix_ms`, `created_unix_ms`.
- `HistoryEntry`: `id`, `task_id`, `original_relative_path`, `stored_path`, `reason`, `created_unix_ms`, `size`.
- `LogEntry`: `id`, `level`, `task_id`, `relative_path`, `message`, `created_unix_ms`.
- `PairedDevice`: `device_id`, `display_name`, `public_key`, `last_seen_unix_ms`, `trusted`.
- `AppError`: top-level variants `PeerOffline`, `FolderMissing`, `PermissionDenied`, `DiskFull`, `FileLocked`, `HashMismatch`, `InvalidPath`, `CaseCollision`, `NetworkInterrupted`, `ConflictRequired`, and `HistoryLimitReached`.
- `SyncDecision`: planner output enum with variants `ApplyToSecondary`, `MoveSecondaryToHistory`, `MarkPendingReturn`, `RequireConflictDecision`, `KeepBoth`, and `Noop`.

Use `hash_status` values `Verified`, `UnverifiedLargeFile`, and `Unavailable`.

- [ ] **Step 3: Create SQLite migrations**

Create tables for sync tasks, file snapshots, sync baselines, pending return changes, history entries, paired devices, event logs, and schema version tracking.

SQLite must enable WAL mode on database open:

```sql
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;
```

Schema migration tracking must use either `PRAGMA user_version` or a `schema_version` table. Pick one and document it in `migrations.rs`.

- [ ] **Step 4: Implement repository methods**

Implement create/read/update methods for sync tasks, baselines, pending returns, history entries, and logs. Log retention must keep the latest 10,000 entries or 7 days, whichever keeps fewer entries.

- [ ] **Step 5: Verify repository persistence**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml state_repository
```

Expected: repository tests pass.

### Task 3: Implement macOS Platform Layer

**Files:**
- Create: `src-tauri/src/platform/mod.rs`
- Create: `src-tauri/src/platform/traits.rs`
- Create: `src-tauri/src/platform/macos/mod.rs`
- Create: `src-tauri/src/platform/macos/app_dirs.rs`
- Create: `src-tauri/src/platform/macos/fs_rules.rs`
- Create: `src-tauri/src/platform/macos/watcher.rs`
- Create: `src-tauri/src/platform/macos/tray.rs`
- Test: `src-tauri/tests/core/macos_fs_rules_test.rs`

- [ ] **Step 1: Implement app directory resolution**

Store app state under the Tauri app data directory. The platform API must return paths for database, logs, identity key, and peer pins.

Before implementing macOS-specific helpers, define the shared platform interface in `src-tauri/src/platform/traits.rs`. Shared core code must depend on this interface, not on `platform::macos` directly. The interface must cover app paths, sync-root validation, relative path normalization, ignore classification, case-collision detection, watcher startup, and tray/menu installation.

- [ ] **Step 2: Implement macOS ignore rules**

Default ignored names:

```text
.DS_Store
.AppleDouble
.DocumentRevisions-V100
.Spotlight-V100
.TemporaryItems
.Trashes
.git/
node_modules/
.lan-sync-history/
```

These are exact directory matches where a trailing slash is shown. The `.git/` rule does not ignore `.gitignore`, `.gitmodules`, or `.github/`.

- [ ] **Step 3: Implement path validation**

Reject paths that escape the sync root after normalization and canonicalization. Canonicalize the sync root and candidate path before prefix comparison so symlink escape cannot leave the configured root. Preserve case as typed. Normalize internal relative paths to `/` separators.

- [ ] **Step 4: Implement watcher wrapper**

Use `notify` and debounce events for 500 ms before sending scan requests to the core engine. Watcher events trigger scans, not direct sync decisions. In this task, implement only the watcher wrapper and a minimal scan-request interface or stub so the platform layer compiles; the full scanner implementation belongs to Task 4.

- [ ] **Step 5: Implement menu bar/tray commands**

Menu items: Open App, Pause All, Sync Now, Quit.

- [ ] **Step 6: Test macOS rules**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml macos_fs_rules
```

Expected: ignored names and path normalization tests pass.

### Task 4: Implement Scanner, Planner, Conflict Detection, And History

**Files:**
- Create: `src-tauri/src/core/scanner.rs`
- Create: `src-tauri/src/core/planner.rs`
- Create: `src-tauri/src/core/conflict.rs`
- Create: `src-tauri/src/history/store.rs`
- Create: `src-tauri/src/history/mod.rs`
- Test: `src-tauri/tests/core/scanner_planner_history_test.rs`

- [ ] **Step 1: Implement scanner**

Scanner recursively walks the configured root, applies platform ignore rules, stores relative paths with `/`, records size and modified time, and hashes files up to the 100 MB eager hash limit. Files above 100 MB use size and modified time as a hash-unverified fallback and must be marked `UnverifiedLargeFile`. Symlinks are skipped and logged as warnings.

- [ ] **Step 2: Implement planner**

Planner emits `ApplyToSecondary`, `MoveSecondaryToHistory`, `MarkPendingReturn`, `RequireConflictDecision`, `KeepBoth`, or `Noop` by comparing current snapshots with baselines. Directory renames are treated as delete plus create. If `KeepBoth` is selected in the UI, write the incoming file to `<stem> (conflict from <device-name> <YYYY-MM-DD HHmmss>)<extension>` instead of overwriting primary. If that path exists, append `-2`, `-3`, and so on.

- [ ] **Step 3: Implement history store**

Use:

```text
.lan-sync-history/trash/<unix-ms>/<relative-path>
.lan-sync-history/overwritten/<unix-ms>/<relative-path>
```

History retention defaults to 30 days or 1 GB per sync task. Restore to the original relative path when free; if occupied, restore to a timestamped path such as `name (restored 2026-05-11 143000).ext`.

If history storage is full, block only operations that require new history writes, such as delete-to-history and overwrite-backup. Create/update operations may continue. The UI must expose a cleanup entry point for deleting entries older than 30 days or opening the history folder.

- [ ] **Step 4: Implement conflict detection**

A conflict exists when secondary has pending create/update and primary current content differs from baseline. If hashes are available, hash comparison is authoritative: modified time changes with identical hash are not conflicts. If hashes are unavailable for large files, fall back to size and modified time and mark the conflict decision as hash-unverified.

- [ ] **Step 5: Test safety behavior**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml scanner_planner_history
```

Expected: delete moves to history, secondary delete does not affect primary, and conflicts are detected.

### Task 5: Implement Pairing And Transport On macOS

**Files:**
- Create: `src-tauri/src/pairing/identity.rs`
- Create: `src-tauri/src/pairing/handshake.rs`
- Create: `src-tauri/src/pairing/mod.rs`
- Create: `src-tauri/src/transport/protocol.rs`
- Create: `src-tauri/src/transport/connection.rs`
- Create: `src-tauri/src/transport/transfer.rs`
- Create: `src-tauri/src/transport/discovery.rs`
- Create: `src-tauri/src/transport/mod.rs`
- Test: `src-tauri/tests/transport/pairing_transfer_test.rs`

- [ ] **Step 1: Add crypto and TLS dependencies**

Add:

```toml
ed25519-dalek = { version = "2", features = ["rand_core"] }
rand = "0.8"
rustls = "0.23"
sha2 = "0.10"
tokio-rustls = "0.26"
```

- [ ] **Step 2: Implement persistent identity**

Create an Ed25519 identity on first launch and store it in the app data directory.

- [ ] **Step 3: Implement pairing code**

Derive a six-digit code from both public keys and a session nonce. Sort the two public keys lexicographically, then compute `SHA256("lan-sync-pairing-v1" || nonce || min_public_key || max_public_key)`, convert the first bytes to an integer, and take modulo `1_000_000`. Both devices must show the same zero-padded six-digit code.

- [ ] **Step 4: Implement pinned encrypted connection**

Use TLS and reject a peer if its identity does not match the pinned public key after pairing. During the initial pairing session, allow a temporary unpinned encrypted connection only for exchanging public keys and verification data; do not mark the peer trusted until both users confirm the same six-digit code. The six-digit code confirmation is the MITM protection for this initial unpinned connection; if codes differ, reject pairing.

- [ ] **Step 5: Implement manual IP connection first**

Manual IP and port connection is P0. `src-tauri/src/transport/discovery.rs` may be created in P0 only as an interface or disabled stub. Do not implement real UDP discovery in P0; UDP LAN discovery is P1 and may only be implemented after manual connection works.

- [ ] **Step 6: Implement safe file transfer**

Write incoming data to `<target>.lan-sync-partial`, verify blake3 hash, then atomically rename. P0 does not support resumable transfer; failed transfers restart from byte 0. On startup, clean stale `.lan-sync-partial` files that are not owned by an active transfer.

- [ ] **Step 7: Test local loopback transfer**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml pairing_transfer
```

Expected: paired local peers transfer a file and destination hash matches source hash.

### Task 6: Implement macOS Sync Executor

**Files:**
- Create: `src-tauri/src/core/executor.rs`
- Modify: `src-tauri/src/core/mod.rs`
- Test: `src-tauri/tests/core/macos_executor_test.rs`

- [ ] **Step 1: Apply primary create/update**

Send file to secondary and update baseline only after secondary confirms write completion and hash verification. Same-path operations must be serialized so a rapid second scan cannot update baseline out of order.

- [ ] **Step 2: Apply primary delete**

Send delete notice to secondary. Secondary moves target file into `.lan-sync-history/trash`.

- [ ] **Step 3: Record secondary pending returns**

Secondary create/update records pending return entries. Secondary delete records no primary action.

- [ ] **Step 4: Execute manual return-sync**

Selected secondary files copy to primary only when no conflict exists.

- [ ] **Step 5: Execute confirmed overwrite**

Before overwriting primary, move old primary file into `.lan-sync-history/overwritten`.

- [ ] **Step 6: Test executor**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml macos_executor
```

Expected: no overwrite occurs without confirmation, no synchronized delete is permanent, successful files update baseline independently, failed files record visible errors, and previously successful files are not rolled back.

P0 executor behavior:

- Process files serially per sync task.
- Retry network failures, temporary I/O failures, file-locked errors, and still-changing-file errors 3 times with 1s, 2s, and 4s backoff.
- Do not retry permission errors, invalid paths, case collisions, or explicit user conflicts.
- On graceful shutdown, stop accepting new file operations and wait up to 30 seconds for the current single-file transfer to complete before exiting. If it does not finish, interrupt it and let startup cleanup/retry handle the partial file.

### Task 7: Implement macOS UI And Tauri Commands

**Files:**
- Create: `src-tauri/src/app_state.rs`
- Create: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/main.rs`
- Create: `src/app/App.tsx`
- Create: `src/features/pairing/PairingScreen.tsx`
- Create: `src/features/dashboard/Dashboard.tsx`
- Create: `src/features/sync-task/TaskDetail.tsx`
- Create: `src/features/return-sync/ReturnSyncScreen.tsx`
- Create: `src/features/conflicts/ConflictModal.tsx`
- Create: `src/features/history/HistoryScreen.tsx`
- Create: `src/features/logs/LogsScreen.tsx`
- Create: `src/features/settings/SettingsScreen.tsx`
- Create: `src/lib/tauriApi.ts`

- [ ] **Step 1: Use UI-UX-Pro-Max before UI design**

Before creating or modifying UI files, the worker must use the `UI-UX-Pro-Max` skill. Use it to define the macOS baseline interface direction, typography, color palette, dashboard hierarchy, conflict modal UX, history/trash recovery UX, empty states, and error states.

Expected output in the worker notes: chosen UI direction, palette, typography, primary dashboard layout, and how destructive actions are visually distinguished.

- [ ] **Step 2: Expose backend commands**

Commands must cover identity, pairing, manual IP connect, task create/list, scan now, sync now, pending return list, return-sync selected, conflict decision, history list/restore, and logs.

Task creation commands must implement a two-device proposal flow: local creates proposal, peer accepts and chooses peer folder, both sides validate folders are empty after ignore rules, both sides persist the same `task_id`, then the first baseline scan runs before automatic sync starts.

- [ ] **Step 3: Implement typed TypeScript API wrapper**

UI code must call `src/lib/tauriApi.ts` instead of invoking Tauri commands directly. Tauri API functions must return `Result<T, AppError>`-style objects in TypeScript so UI error handling is consistent.

- [ ] **Step 4: Implement setup and dashboard UI**

Show pairing state, peer state, task status, pending return count, conflict count, transfer progress, and errors. P0 may use simple conditional rendering instead of React Router.

- [ ] **Step 5: Implement return-sync and conflict UI**

Return-sync screen lets users select pending files. Conflict modal offers cancel, keep both, and overwrite primary with backup.

- [ ] **Step 6: Implement history and logs UI**

Users can view history entries, restore files, inspect recent sync events, and edit P0-safe settings such as history/log visibility and startup preferences where platform support exists.

- [ ] **Step 7: Test UI**

Run:

```bash
npm test
```

Expected: UI tests pass.

### Task 8: macOS Baseline Verification

**Files:**
- Create: `src-tauri/tests/integration/macos_two_peer_sync_test.rs`
- Create: `tests/ui/macos_smoke.spec.ts`

- [ ] **Step 1: Test primary flow**

Verify create, update, and delete from a macOS primary to a local secondary test peer. The integration test peer is a Rust-layer loopback peer using temporary directories, not two full Tauri GUI instances.

- [ ] **Step 2: Test return-sync flow**

Verify secondary create/update becomes pending and manual return-sync copies to primary.

- [ ] **Step 3: Test conflict and overwrite backup**

Verify primary conflict blocks overwrite until confirmed, and confirmed overwrite creates history backup.

- [ ] **Step 4: Build macOS app**

Run:

```bash
npm test
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

Expected: tests pass and a macOS development build is produced.

All integration tests must use temporary directories and clean them up after completion.

## 5. Handoff To Windows

Before Windows work starts, macOS branch must provide:

- Stable Rust model types.
- Stable database schema migrations.
- Stable Tauri command names and response shapes.
- Passing scanner/planner/history/executor tests.
- Passing local loopback transport tests.
- A working UI smoke flow.
- A written list of platform assumptions that Windows must not inherit blindly.
