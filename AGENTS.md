# AGENTS.md

## Project Mission

Build a local LAN folder sync desktop app for macOS and Windows. The product uses a fixed primary-secondary sync model:

- Primary changes automatically sync to secondary.
- Primary deletes move secondary files into sync history/trash.
- Secondary create/update changes become pending return-sync items.
- Secondary deletes do not affect primary.
- Return-sync from secondary to primary requires explicit user action.
- Conflicts never overwrite silently.
- Confirmed overwrites must back up the old primary file first.

Read these documents before implementation:

- `docs/superpowers/specs/2026-05-11-lan-folder-sync-prd.md`
- `docs/superpowers/plans/2026-05-11-lan-folder-sync-technical-development.md`
- `docs/superpowers/plans/2026-05-11-lan-folder-sync-macos-development.md`
- `docs/superpowers/plans/2026-05-11-lan-folder-sync-windows-development.md`

## Required Development Order

1. Implement and stabilize the macOS baseline first.
2. Reuse the macOS/shared baseline for Windows.
3. Integrate both platforms only after platform tests pass.

Use these worktrees:

- `.worktrees/macos` for macOS and shared baseline work.
- `.worktrees/windows` for Windows platform adaptation.
- `.worktrees/integration` for merge, cross-platform verification, and release checks.

Do not create extra worktrees unless a human explicitly approves.

## Karpathy Guidelines For This Project

### Think Before Coding

- State assumptions before implementing.
- If a requirement can be interpreted multiple ways, stop and ask or document the chosen interpretation.
- Push back on unsafe sync behavior, especially silent overwrite, silent delete, and timestamp-only conflict resolution.
- If a simpler implementation can satisfy P0, choose it over a speculative architecture.

### Simplicity First

- Build the minimum P0 behavior that protects user data.
- Do not add cloud sync, WAN traversal, multi-device mesh sync, auto-update, rich diff previews, or full bidirectional automation in P0.
- Do not add abstractions before a second real use case exists.
- Prefer scanner correctness before watcher cleverness.
- Prefer manual IP connection before UDP discovery.

### Surgical Changes

- Touch only files owned by the current worktree plan.
- Do not refactor unrelated code.
- Do not change product sync semantics without updating the PRD first.
- Do not "clean up" adjacent code unless your change made it incorrect or unused.
- Match existing style once the scaffold exists.

### Goal-Driven Execution

Every task must have a verification step:

- Rust logic: run targeted `cargo test --manifest-path src-tauri/Cargo.toml <test-name>`.
- UI logic: run `npm test`.
- Build/package changes: run `npm run tauri build` on the current platform.
- Cross-platform behavior: verify in `.worktrees/integration`.

If verification fails, report the failing command and exact failure before changing direction.

## Non-Negotiable Safety Rules

- Never permanently delete synchronized user files immediately.
- Never write received files directly to final paths; write to a temporary file, verify hash, then rename atomically.
- Never use modification time as the only conflict detector.
- Never overwrite a changed primary file without explicit user confirmation.
- Always back up the old primary file before confirmed overwrite.
- Never silently skip failed files; record visible errors for the UI.
- Never call the product "fully bidirectional sync" in user-facing UI.

## UI/UX Design Rules

- Any agent designing, building, refining, or reviewing UI must use the `UI-UX-Pro-Max` skill before making UI decisions.
- For this project, UI work includes pairing screens, dashboard, sync task setup, pending return-sync lists, conflict modals, history/trash views, logs, settings, tray/menu interactions, empty states, error states, and platform-specific copy.
- The UI must make data safety obvious: pending destructive actions, conflicts, overwrite backup behavior, history/trash recovery, and sync errors must be visible and understandable.
- Do not ship generic placeholder UI. Use `UI-UX-Pro-Max` to choose an intentional product interface direction, typography, color palette, spacing, and interaction patterns appropriate for a safety-critical desktop utility.

## Platform Rules

### macOS First

- macOS establishes the shared engine and UI baseline.
- Use `notify`/FSEvents as a trigger for scans, not as the source of truth.
- Ignore `.DS_Store`, `.AppleDouble`, `.DocumentRevisions-V100`, `.Spotlight-V100`, `.TemporaryItems`, `.Trashes`, and `.lan-sync-history`.
- Do not attempt full macOS resource fork or metadata sync in P0.

### Windows Second

- Windows starts only after macOS/shared interfaces are stable.
- Treat default Windows filesystems as case-insensitive.
- Reject invalid path characters, reserved device names, trailing spaces, and trailing dots.
- Detect case-only collisions from macOS before writing to Windows.
- Surface firewall/network permission problems clearly.

## Definition Of Done

A change is not done until:

- It matches the PRD sync semantics.
- It includes targeted tests where practical.
- Verification commands were run and results are known.
- It does not broaden P0 scope.
- It preserves user data safety guarantees.
