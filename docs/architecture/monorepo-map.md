# Worktree And Package Map

LanBridge is maintained through platform worktrees rather than separate packages.

## Top-Level Layout

```text
AGENTS.md
docs/
plans/
worktrees/
  windows/
  macos/
  integration/
```

## Worktree Inventory

| Path | Purpose | Notes |
| --- | --- | --- |
| `worktrees/windows` | Windows app, Windows packaging, Windows filesystem rules | Produces MSI/NSIS bundles. |
| `worktrees/macos` | macOS app, macOS packaging, macOS filesystem rules | Build final `.app/.dmg` on a real Mac. |
| `worktrees/integration` | Merge and release verification | Use after both platform branches pass local checks. |

## Application Layout Inside Each Worktree

| Path | Purpose | Dependency Direction |
| --- | --- | --- |
| `src/` | React UI and Tauri API wrappers | Calls Tauri commands only through `src/lib/tauriApi.ts`. |
| `src/features/` | UI screens and workflows | May depend on `src/lib`, not Rust internals. |
| `src-tauri/src/core/` | Sync domain model, scanner, planner, executor, conflict logic | Must not depend on UI or platform-specific command state. |
| `src-tauri/src/transport/` | Discovery, TCP server/client, authenticated protocol, chunk transfer | Depends on pairing and core models only when needed. |
| `src-tauri/src/state/` | SQLite migrations and repositories | Owns persistence schema. |
| `src-tauri/src/platform/` | Windows/macOS filesystem rules, app dirs, watcher adapters | Platform-specific behavior stays here. |
| `src-tauri/tests/` | Rust integration and flow tests | Add tests for sync semantics before broad behavior changes. |
| `docs/` | Product, testing, architecture, workflow docs | Keep in sync across platform worktrees. |

## Boundary Rules

- Cross-platform sync semantics belong in shared Rust modules, not in platform folders.
- Platform folders should only own app paths, ignore rules, path validation, tray/watch adapters, and OS-specific behavior.
- UI should not invent sync semantics; it should call commands and render results.
- When the same fix is needed on Windows and macOS, apply and test it in both worktrees.
