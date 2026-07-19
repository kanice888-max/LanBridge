# AGENTS.md

Start here for LanBridge. This repository uses a software Harness: keep this file short, and treat `docs/` plus `plans/active/` as the source of truth for agent work.

Main is the coordination branch for product docs, architecture, active plans, and workflow rules. Platform implementation work should normally happen in `worktrees/macos` or `worktrees/windows`, then be synchronized through the matching platform branch and integration worktree when it is ready.

## What This Repo Contains

- Product requirements: `docs/product/PRD.md`
- Architecture map: `docs/architecture/index.md`
- Worktree and package map: `docs/architecture/monorepo-map.md`
- Sync/data-safety invariants: `docs/rules/invariants.md`
- Engineering principles: `docs/rules/golden-principles.md`
- Default task workflow: `docs/workflows/task-flow.md`
- Cleanup workflow: `docs/workflows/cleanup.md`
- Validation commands: `docs/validation/checks.md`
- Known debt and follow-ups: `docs/quality/debt-log.md`
- Active implementation plans: `plans/active/`

## Default Working Loop

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

- Never silently overwrite or permanently delete synchronized user files.
- Never call LanBridge "fully bidirectional sync"; the model is primary-secondary with explicit return-sync.
- Never treat chat memory as the only record of architecture or workflow decisions.
- Use Chinese for user-facing discussion in this environment.
