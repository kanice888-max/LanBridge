# LanBridge

LanBridge is a local-network folder synchronization app built with Tauri, React, and Rust.

It is designed for private LAN transfer between a Primary device and a Secondary device. LanBridge is not fully bidirectional sync: the Primary sends changes to the Secondary automatically, while Secondary-side changes require explicit return-sync back to the Primary.

## Features

- Local-network device discovery and manual connection.
- Primary/Secondary task model with explicit return-sync.
- Folder-level file tree, pending return state, conflict handling, and history restore.
- Transfer progress cards, notifications, and desktop tray support.
- macOS and Windows platform implementations.

## Safety Model

LanBridge is built around a conservative data-safety model:

- Primary is the source of automatic sync to Secondary.
- Secondary changes are never silently pushed back; the user must explicitly return-sync.
- Conflict resolution requires an explicit choice.
- Overwriting Primary files must preserve backup semantics.
- Project deletion only deletes task configuration, not local files.

See [sync invariants](docs/rules/invariants.md) for the detailed rules.

## Security

See [SECURITY.md](SECURITY.md) for supported versions, vulnerability reporting, and LanBridge's local-network security boundary.

The current security hardening plan is tracked in [docs/security/security-hardening-plan.md](docs/security/security-hardening-plan.md).

## Requirements

- Node.js 18 or newer.
- Rust stable toolchain.
- Tauri 1.x development prerequisites for your platform.

Platform-specific Tauri setup:

- macOS: Xcode Command Line Tools.
- Windows: Microsoft C++ Build Tools and WebView2 runtime.

## Development

Install dependencies:

```bash
npm install
```

Run the frontend dev server:

```bash
npm run dev
```

Run the Tauri desktop app:

```bash
npm run tauri dev
```

## Build

Build the frontend:

```bash
npm run build
```

Build the desktop app:

```bash
npm run tauri build
```

Run Rust tests:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Run project naming checks:

```bash
npm run lint:names
```

## Repository Layout

```text
src/              React frontend
src-tauri/        Tauri and Rust backend
scripts/          Project utility scripts
docs/             Architecture, rules, validation, and workflows
redesign/         Public design guide
```

`src-tauri/patches/tao-0.16.11/` is intentionally included because the Rust manifest patches the upstream `tao` crate from this local path.

## What Is Not Committed

Generated dependencies, build outputs, installers, logs, databases, identity keys, local worktrees, and temporary promo/video projects are intentionally ignored. See [.gitignore](.gitignore) and [open-source release workflow](docs/workflows/open-source-github-release.md).

## Contributing

Before changing sync, transfer, pairing, delete, conflict, or history behavior, read:

- [Architecture map](docs/architecture/index.md)
- [Sync/data-safety invariants](docs/rules/invariants.md)
- [Validation commands](docs/validation/checks.md)

Prefer small changes that preserve user data safety.

## License

MIT. See [LICENSE](LICENSE).
