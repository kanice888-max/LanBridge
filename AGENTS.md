# AGENTS.md

Start here for LanBridge. This repository uses a software Harness: keep this file short, and treat `docs/` plus `plans/active/` as the source of truth for agent work.

<<<<<<< HEAD
Build a local LanBridge desktop app for macOS and Windows. The product uses a fixed primary-secondary sync model:
=======
Main is the coordination branch for product docs, architecture, active plans, and workflow rules. Platform implementation work should normally happen in `worktrees/macos` or `worktrees/windows`, then be synchronized through the matching platform branch and integration worktree when it is ready.
>>>>>>> main

## What This Repo Contains

- Product requirements: `docs/superpowers/specs/2026-05-11-lanbridge-prd.md`
- Architecture map: `docs/architecture/index.md`
- Worktree and package map: `docs/architecture/monorepo-map.md`
- Sync/data-safety invariants: `docs/rules/invariants.md`
- Engineering principles: `docs/rules/golden-principles.md`
- Default task workflow: `docs/workflows/task-flow.md`
- Cleanup workflow: `docs/workflows/cleanup.md`
- Validation commands: `docs/validation/checks.md`
- Known debt and follow-ups: `docs/quality/debt-log.md`
- Active implementation plans: `plans/active/`

<<<<<<< HEAD
- `docs/superpowers/specs/2026-05-11-lanbridge-prd.md`
- `docs/superpowers/plans/2026-05-11-lanbridge-technical-development.md`
- `docs/superpowers/plans/2026-05-11-lanbridge-macos-development.md`
- `docs/superpowers/plans/2026-05-11-lanbridge-windows-development.md`
=======
## Default Working Loop
>>>>>>> main

1. Read the user request and the relevant active plan.
2. Work in `worktrees/macos` for macOS-first/shared implementation, or `worktrees/windows` for Windows-specific implementation.
3. Read `docs/architecture/index.md` and `docs/architecture/monorepo-map.md` before changing cross-platform behavior.
4. Read `docs/rules/invariants.md` before touching sync, transfer, pairing, delete, conflict, or history behavior.
5. Make the smallest change that preserves user data safety.
6. Run the checks from `docs/validation/checks.md`.
7. Update docs, plans, or `docs/quality/debt-log.md` when behavior or known risk changes.

## Worktrees

- `worktrees/windows`: Windows build and Windows-specific fixes.
- `worktrees/macos`: macOS build and macOS-specific fixes.
- `worktrees/integration`: integration, merge, and release checks.

Do not create extra worktrees unless a human explicitly approves.

## Non-Negotiables

<<<<<<< HEAD
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
- Ignore `.DS_Store`, `.AppleDouble`, `.DocumentRevisions-V100`, `.Spotlight-V100`, `.TemporaryItems`, `.Trashes`, and `.lanbridge-history`.
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
=======
- Never silently overwrite or permanently delete synchronized user files.
- Never call LanBridge "fully bidirectional sync"; the model is primary-secondary with explicit return-sync.
- Never treat chat memory as the only record of architecture or workflow decisions.
- Use Chinese for user-facing discussion in this environment.
>>>>>>> main
