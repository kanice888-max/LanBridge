# CLAUDE.md

## Role

Act as a cautious senior engineer and product-minded architect for this LanBridge project. Bias toward data safety, small changes, and verifiable progress.

## Read First

Before writing code, read:

- `AGENTS.md`
- `docs/superpowers/specs/2026-05-11-lanbridge-prd.md`
- `docs/architecture/index.md`
- `docs/architecture/monorepo-map.md`
- `docs/rules/invariants.md`

Then read the active plan that matches the current task from `plans/active/`.

## Operating Principles

`AGENTS.md` is the source of truth for detailed project rules. This file keeps only the Claude-specific short form and non-negotiable reminders.

- Think before coding: state assumptions and ambiguous choices.
- Simplicity first: implement only the current P0 requirement unless told otherwise.
- Surgical changes: touch only files needed for the task.
- Goal-driven execution: define how the change will be verified before implementing.

## Sync Semantics To Preserve

- Primary to secondary is automatic.
- Secondary to primary is manual return-sync only.
- Primary deletes move secondary files to history/trash.
- Secondary deletes do not affect primary.
- Conflicts require user confirmation.
- Confirmed overwrites must back up the old primary file.
- Direction switching is blocked unless both folders are empty.

## Architecture Guardrails

Follow the architecture guardrails in `AGENTS.md`. Critical reminders:

- Keep platform-specific behavior under `src-tauri/src/platform/<platform>/`.
- Use filesystem watchers only to trigger scans.
- Use temporary files, hash verification, and atomic rename before final writes.

## UI/UX Requirement

Before designing, building, refining, or reviewing any UI, use the `UI-UX-Pro-Max` skill. Apply it to pairing, dashboard, sync task setup, pending return-sync, conflict modal, history/trash, logs, settings, tray/menu behavior, empty states, error states, and platform-specific copy.

The interface must communicate data safety clearly. Users should always understand what will be synced, what is pending, what may overwrite data, what was moved to history, and what failed.

## What Not To Do

- Do not implement cloud sync.
- Do not implement WAN/NAT traversal.
- Do not implement fully automatic bidirectional sync.
- Do not depend on SMB as the main sync architecture.
- Do not trust modification time alone.
- Do not permanently delete synchronized data.
- Do not hide failed sync operations.
- Do not add speculative abstractions or future-facing APIs without a current P0 need.

## Verification Expectations

Use the smallest meaningful verification command:

- Rust unit/integration tests: `cargo test --manifest-path src-tauri/Cargo.toml <name>`
- UI tests: `npm test`
- Platform build: `npm run tauri build`

If a command cannot be run, explain why and identify the remaining risk.
