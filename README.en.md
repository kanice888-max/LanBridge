<div align="center">

# LanBridge

**Safe folder synchronization between Mac and Windows on your local network.**

No cloud storage. No third-party relay. You stay in control of every important change.

[中文（默认）](README.md) · [English](README.en.md)

</div>

> [!IMPORTANT]
> LanBridge uses a **Primary / Secondary + explicit return-sync** model. It is not fully automatic bidirectional sync. Changes from the Primary sync automatically to the Secondary; changes made on the Secondary affect the Primary only after the user explicitly returns them.

LanBridge is an open-source desktop app for synchronizing selected folders between macOS and Windows devices on the same trusted local network. It is for people who use both platforms, value privacy, and want to keep control over their files: nothing needs to be uploaded to a cloud service, and a change on the Secondary can never silently overwrite the Primary.

## Why LanBridge

- **Keep data on your local network** — Discover devices on the LAN or connect by manual IP. No cloud drive or third-party relay is required.
- **A clear source of truth** — Every task has a designated Primary. New, updated, and deleted Primary content syncs automatically to the Secondary, removing ambiguity about which device is authoritative.
- **Secondary changes stay under your control** — Creates, edits, and deletes on the Secondary appear as pending return items. They reach the Primary only when you choose to return them.
- **No silent conflict overwrite** — When both sides change the same path, LanBridge asks you to decide. A confirmed overwrite first preserves the old Primary file.
- **Recoverable deletes** — Synchronized deletes and files replaced during conflict resolution go to task history before removal, so they can be restored.
- **Pair devices deliberately** — Pairing requires matching a verification code on both devices. A task root is registered only after the receiving device accepts its invitation.
- **Built for desktop folders** — File trees, sync state, transfer progress, pending returns, conflict handling, history restore, and system-tray access are included in the workflow.

## How it works

```text
Primary folder ── automatic sync ──▶ Secondary folder
      ▲                                  │
      └──── user-approved return-sync ◀──┘
```

1. Pair two devices on the same trusted LAN through discovery or a manual IP address.
2. Create a task, choose its Primary, and let the receiving device choose its own local folder.
3. Primary changes transfer automatically to the Secondary. A Primary deletion moves the corresponding Secondary content to LanBridge history first.
4. Secondary changes appear in a pending-return list. Before returning them, LanBridge checks whether the Primary has changed since the last sync.
5. Conflicts require a user choice; neither restore nor overwrite silently discards an existing file.

## Good fits

- Keep work folders, creative assets, or project documents aligned between a Mac and a Windows PC.
- Use a local-network workflow without cloud storage, accounts, or continuous internet uploads.
- Keep automatic synchronization convenient while requiring an explicit decision before Secondary changes alter the Primary.

## What LanBridge is not

- It is not a cloud drive and does not support WAN or NAT-traversal synchronization.
- It is not fully automatic bidirectional sync. Do not treat it as a two-way replacement for Dropbox, iCloud Drive, or Syncthing.
- It does not guarantee safe live synchronization of databases, virtual-machine images, browser profiles, mail stores, dependency caches, or files continuously written by another app.
- It does not synchronize symlinks, and it does not apply synchronized deletes as immediate permanent deletion.

## Get started

LanBridge is currently pre-1.0. Before you begin, make sure both devices are on the same trusted local network and that the app is running on macOS and Windows.

## Download and install

Download the installer for your device and `SHA256SUMS.txt` from [GitHub Releases](https://github.com/kanice888-max/LanBridge/releases/latest):

| Platform | Download | Notes |
| --- | --- | --- |
| macOS Intel | `LanBridge_0.2.0_x64.dmg` | For Intel-based Macs. |
| macOS Apple Silicon | `LanBridge_0.2.0_aarch64.dmg` | For Apple M-series Macs. |
| Windows x64 | `.exe` or `.msi` | Use `.exe` for a personal install or `.msi` for managed deployment. |

Verify the SHA-256 checksum after downloading. The macOS app is ad-hoc signed and not notarized:
the first launch may require Control-click **Open** or **Open Anyway** in System Settings → Privacy &
Security. Never disable Gatekeeper globally. See the [macOS installation guide](docs/release/macos-installation.md)
and [Windows installation guide](docs/release/windows-installation.md) for complete steps.

1. Open LanBridge on both devices.
2. Start pairing through LAN discovery or a manual IP address, then verify the code displayed on both devices.
3. Create a task and select the Primary folder. Accept the invitation on the receiving device and choose its target folder.
4. Create or edit a test file on the Primary and confirm it arrives on the Secondary.
5. Edit that file on the Secondary, review it in Pending Returns, and explicitly return it when ready.

## Run from source

### Prerequisites

- Node.js 18 or later
- Rust stable toolchain
- Tauri 1.x development prerequisites for your platform
  - macOS: Xcode Command Line Tools
  - Windows: Microsoft C++ Build Tools and WebView2 Runtime

```bash
npm install
npm run tauri dev
```

Common checks and build commands:

```bash
npm run lint:names
npm run build
npm test
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

See [validation checks](docs/validation/checks.md) for full platform validation and release requirements.

## Security and data protection

LanBridge is designed for **trusted local networks**. Discovery is not trust: pair only after confirming the peer's identity. The app maintains a local device identity, task invitations need receiver approval, and every conflict requires an explicit decision.

- [Sync and data-safety invariants](docs/rules/invariants.md)
- [Security policy](SECURITY.md)
- [Security hardening plan](docs/security/security-hardening-plan.md)

Please do not disclose security issues publicly. Follow [SECURITY.md](SECURITY.md) to report them privately.

## Project layout and contributing

```text
src/              React frontend
src-tauri/        Rust / Tauri backend and integration tests
docs/             Product, architecture, security, and workflow documentation
scripts/          Project utility scripts
```

Before contributing changes to synchronization, transfer, pairing, deletion, conflicts, or history, read the [architecture overview](docs/architecture/index.md), [data-safety invariants](docs/rules/invariants.md), and [task workflow](docs/workflows/task-flow.md). Validate the change in the worktree that owns it.

Read the [contribution guide](CONTRIBUTING.md), [changelog](CHANGELOG.md), and [code of conduct](CODE_OF_CONDUCT.md)
before opening an issue or pull request.

## License

LanBridge is released under the [MIT License](LICENSE).
