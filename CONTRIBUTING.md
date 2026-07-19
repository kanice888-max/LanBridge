# Contributing to LanBridge

Thank you for helping improve LanBridge. Please open an issue before starting a large feature or
behavior change so the data-safety implications can be discussed first.

## Development

```bash
npm ci --include=optional
npm run lint:names
npm run build
npm test
cargo test --manifest-path src-tauri/Cargo.toml
```

Use the platform worktree that owns a change. Before touching synchronization, pairing, deletion,
conflicts, or history, read `docs/architecture/index.md` and `docs/rules/invariants.md`.

## Pull requests

- Keep changes focused and explain user-facing behavior.
- Add or update tests for behavior changes.
- Never describe LanBridge as fully bidirectional sync.
- Do not include build products, logs, databases, credentials, personal paths, or test files.
- Complete the PR template and keep required CI checks green.
