# LanBridge Technical Development Index

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement the platform plans task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Coordinate development of the LanBridge app across macOS and Windows while keeping platform-specific work isolated.

**Architecture:** Build the macOS client first to establish the shared Rust/Tauri sync engine, product behavior, UI flow, encrypted pairing, and safety model. Then implement the Windows client by reusing shared engine interfaces and adding Windows-specific filesystem, watcher, tray, firewall, startup, and packaging behavior.

**Tech Stack:** Rust, Tauri, React, TypeScript, SQLite, Tokio, notify, blake3, tokio-rustls, macOS FSEvents via `notify`, Windows ReadDirectoryChangesW via `notify`, Vitest, Rust tests, Playwright.

---

## Required Reading Order

1. Product requirements: `docs/superpowers/specs/2026-05-11-lanbridge-prd.md`
2. macOS implementation plan: `docs/superpowers/plans/2026-05-11-lanbridge-macos-development.md`
3. Windows implementation plan: `docs/superpowers/plans/2026-05-11-lanbridge-windows-development.md`
4. Next-stage optimization plan: `docs/superpowers/plans/2026-05-13-lanbridge-next-stage-optimization.md`

## Required UI Skill

Any AI worker assigned to UI design, UI implementation, UI refinement, UI review, platform-specific copy, tray/menu UX, empty states, or error states must use the `UI-UX-Pro-Max` skill before making UI decisions.

UI workers must use that skill to define:

- Product interface direction.
- Typography and spacing.
- Color palette.
- Component hierarchy.
- Dashboard information architecture.
- Conflict and destructive-action UX.
- Platform-specific copy for macOS and Windows.

Do not implement generic placeholder UI for P0. The UI must make sync state, pending return-sync, conflicts, history/trash recovery, and failures visible.

## Worktree Strategy

Keep the worktree count small. Use one platform worktree per OS plus one integration worktree.

### macOS Worktree

- Path: `.worktrees/macos`
- Branch: `codex/lanbridge-macos`
- Purpose: design and implement the first working client, including shared engine modules where needed.
- Owned files:
  - `src-tauri/src/core/**`
  - `src-tauri/src/state/**`
  - `src-tauri/src/history/**`
  - `src-tauri/src/pairing/**`
  - `src-tauri/src/transport/**`
  - `src-tauri/src/platform/macos/**`
  - `src/**`
  - `tests/**`
  - macOS-specific Tauri config and packaging files

### Windows Worktree

- Path: `.worktrees/windows`
- Branch: `codex/lanbridge-windows`
- Purpose: adapt the working macOS baseline to Windows and implement Windows-only filesystem, watcher, tray, firewall, startup, and packaging behavior.
- Owned files:
  - `src-tauri/src/platform/windows/**`
  - Windows-specific Tauri config and packaging files
  - Windows-specific tests
  - shared code only when required for platform abstraction compatibility

### Integration Worktree

- Path: `.worktrees/integration`
- Branch: `codex/lanbridge-integration`
- Purpose: merge macOS and Windows work, resolve interface mismatches, and run two-device integration verification.
- Owned files:
  - any file required to resolve merge conflicts
  - integration test harnesses
  - release documentation

## Worktree Creation Commands

This repository starts as an empty Git repo, so worktree setup needs a bootstrap commit before platform work begins.

### Bootstrap Before Worktrees

Run these from the repository root before creating worktrees. The bootstrap commit must include `.gitignore` before `.worktrees/` is created.

Create root `.gitignore` with at least:

```gitignore
.worktrees/
node_modules/
dist/
target/
src-tauri/target/
.DS_Store
Thumbs.db
```

Then commit the documentation baseline:

```powershell
git add .gitignore AGENTS.md CLAUDE.md docs
git commit -m "docs: add LanBridge product and development plans"
```

Expected: the repository has its first commit containing the PRD, platform plans, AI rules, and generated documentation bundle.

### Create Platform Worktrees

Run these from repository root after the bootstrap docs commit exists:

The following `git worktree` commands are shell-neutral and can be run from Bash, Git Bash, or PowerShell. Platform-specific plans use Bash for macOS commands and PowerShell for Windows commands.

```bash
git worktree add .worktrees/macos -b codex/lanbridge-macos
git worktree add .worktrees/windows -b codex/lanbridge-windows
git worktree add .worktrees/integration -b codex/lanbridge-integration
```

Before creating project-local worktrees, ensure `.worktrees/` is ignored:

```gitignore
.worktrees/
```

## Development Order

1. Complete the macOS plan first.
2. Stabilize shared interfaces and commit the macOS baseline.
3. Start the Windows plan from the macOS baseline.
4. Merge both platform branches in the integration worktree.
5. Verify Mac-to-Windows and Windows-to-Mac task roles over LAN.
6. Implement the next-stage optimization plan after the basic transport and sync loop are passing.

## Shared Sync Semantics

Both clients must implement exactly the same product semantics:

- Each task has a fixed primary and secondary device.
- Primary create/update/delete automatically syncs to secondary.
- Primary delete moves secondary data into sync history/trash.
- Secondary create/update becomes pending return-sync.
- Secondary delete does not affect primary.
- Manual return-sync copies selected secondary create/update changes to primary.
- Return-sync conflict requires user confirmation.
- Confirmed overwrite backs up the old primary file first.
- Direction switching is blocked unless both folders are empty.
- "Empty" means no non-ignored files or directory entries after applying platform ignore rules.
- Sync operations are atomic per file, not per directory transaction.
- P0 processes sync actions serially per task. Concurrency is P1.
- Successful files update baseline immediately after target-side write and hash verification succeed.
- Failed files are recorded as visible errors and do not roll back previously successful files.
- Network failures, temporary I/O failures, file-locked errors, and still-changing-file errors retry up to 3 times with 1s, 2s, and 4s backoff. Permission errors, invalid paths, and case collisions do not retry.
- Content hash comparison is authoritative when available. Modified time changes with identical hash are not conflicts.
- Large files above the eager hash limit use size and modified time as a hash-unverified fallback.
- P0 does not follow or synchronize symlinks.
- P0 treats directory rename/move as delete plus create.
- P0 cleans stale `.lanbridge-partial` files on startup. During graceful shutdown, the app stops accepting new work and waits up to 30 seconds for the current single-file transfer; if it does not finish, interrupt it and let startup cleanup/retry handle the partial file.

## Shared Defaults

- Eager hash limit: 100 MB.
- Watcher debounce: 500 ms.
- History retention: 30 days or 1 GB per sync task.
- Log retention: latest 10,000 entries or 7 days, whichever keeps fewer entries.
- Transfer resumability: not supported in P0; failed transfers restart from byte 0.
- High-risk default ignores: exact directory `.git/`, exact directory `node_modules/`, `.lanbridge-history/`, platform system trash/history folders, Windows shortcuts `*.lnk`, and platform-specific junk files.

## Ignore Rule Matching

Platform ignore rules must distinguish exact name matches, exact directory matches, and glob patterns:

- Exact file/name match examples: `.DS_Store`, `Thumbs.db`, `desktop.ini`.
- Exact directory match examples: `.git/`, `node_modules/`, `.lanbridge-history/`.
- Glob pattern examples: `~$*`, `*.tmp`, `*.lnk`.

The `.git/` rule applies only to a directory named `.git`. Do not treat `.gitignore`, `.gitmodules`, or `.github/` as ignored by this rule.

## Shared Platform Abstraction

The macOS baseline must define a platform boundary before platform-specific code grows. Windows must implement the same boundary rather than calling macOS helpers.

Minimum platform interface:

- `app_data_dir()`
- `database_path()`
- `identity_key_path()`
- `peer_pins_path()`
- `log_path()`
- `normalize_relative_path(path)`
- `validate_sync_root(path)`
- `validate_target_relative_path(path)`
- `classify_ignored_entry(path, entry_type)`
- `detect_case_collisions(snapshots)`
- `start_watcher(sync_root, event_sender)`
- `install_tray_menu(commands)`

Shared core modules may depend on this interface, not on `src-tauri/src/platform/macos/**` or `src-tauri/src/platform/windows/**` directly.

## Shared Task Setup Protocol

Creating a sync task is a two-device operation:

1. The local user chooses a local folder and local role.
2. The app sends a task proposal to the paired peer containing task name, proposed primary device ID, proposed secondary device ID, and local relative metadata only. Do not send file contents during proposal.
3. The peer user chooses or confirms the peer folder.
4. Both sides validate that the chosen folders are empty according to platform ignore rules before allowing initial direction selection.
5. Both sides persist the same `task_id`, device roles, local path, peer path, and enabled state.
6. The first scan establishes baseline before automatic sync starts.

If either side rejects the proposal or folder validation fails, no task is created on either side.

## Shared Conflict Naming

When the user chooses `KeepBoth`, write the incoming file next to the target path using:

```text
<stem> (conflict from <device-name> <YYYY-MM-DD HHmmss>)<extension>
```

Do not overwrite an existing conflict file. If the generated name already exists, append `-2`, `-3`, and so on.

## Shared Error Model

`AppError` must include at least these top-level variants so Rust, Tauri commands, and TypeScript UI can share one error language:

- `PeerOffline`
- `FolderMissing`
- `PermissionDenied`
- `DiskFull`
- `FileLocked`
- `HashMismatch`
- `InvalidPath`
- `CaseCollision`
- `NetworkInterrupted`
- `ConflictRequired`
- `HistoryLimitReached`

## Platform Plans

Use the macOS plan for initial architecture and working behavior:

- `docs/superpowers/plans/2026-05-11-lanbridge-macos-development.md`

Use the Windows plan only after the macOS baseline exists:

- `docs/superpowers/plans/2026-05-11-lanbridge-windows-development.md`

## AI Worker Rules

- Do not implement Windows before the macOS baseline is complete.
- Do not create more worktrees unless a human explicitly approves.
- Do not change sync semantics in only one platform plan.
- Do not use SMB as the main sync architecture.
- Do not use modification time as the only conflict detector.
- Do not permanently delete synchronized user files.
- Do not write received files directly to final paths; always use temporary files and hash verification.
