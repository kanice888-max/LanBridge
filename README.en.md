<div align="center">

# LanBridge

**Keep Mac and Windows folders in sync on a trusted local network.**

No cloud storage. No third-party relay. You confirm every important change.

[中文（默认）](README.md) · [English](README.en.md)

</div>

> [!IMPORTANT]
> LanBridge uses a **Primary / Secondary + explicit return-sync** model. Changes from the Primary sync automatically to the Secondary. Changes on the Secondary first appear as pending returns and reach the Primary only after you confirm them. It is not fully automatic bidirectional sync.

LanBridge is an open-source desktop app for keeping selected folders aligned between macOS and Windows devices on the same trusted local network. It is for people who want local-file convenience without handing their files to a cloud service. Its clear sync direction helps prevent accidental changes or conflicts from silently overwriting files.

## Highlights

- **Keep files on your local network** — Discover devices on the LAN or connect by manual IP. No cloud drive or third-party relay is required.
- **A clear sync direction** — Every task has a Primary. Creates, edits, and deletes on the Primary sync automatically to the Secondary.
- **You decide about Secondary changes** — Creates, edits, and deletes on the Secondary first appear as pending returns. They affect the Primary only after you confirm them.
- **No silent conflict overwrite** — If both sides change the same path, LanBridge asks you how to proceed and preserves the old Primary file before an overwrite.
- **Recoverable deletes** — Deleted and replaced files are kept in task history so they can be restored when needed.
- **Built for desktop folders** — File trees, sync state, transfer progress, pending returns, conflict handling, history restore, and system-tray access are part of the workflow.

## How it works

```text
Primary folder ── automatic sync ──▶ Secondary folder
      ▲                                  │
      └──── user-approved return-sync ◀──┘
```

1. Put both devices on the same trusted local network and connect through discovery or a manual IP address.
2. Create a task and choose its Primary. The receiving device accepts the task and chooses its own local folder.
3. Primary changes transfer automatically to the Secondary. Secondary changes first appear as pending returns.
4. Return Secondary changes when you choose. If a conflict occurs, you decide how to keep the files.

## Good fits

- Keep work folders, creative assets, or project documents aligned between a Mac and a Windows PC.
- Use a local-network workflow without cloud storage, accounts, or continuous uploads.
- Keep the convenience of automatic sync without letting Secondary changes immediately alter the Primary.

## Before you start

- LanBridge does not support WAN or NAT-traversal synchronization.
- It is not a fully automatic two-way replacement for Dropbox, iCloud Drive, or Syncthing.
- It is not recommended for databases, virtual-machine images, browser profiles, mail stores, dependency caches, or files another app writes continuously.
- It does not synchronize symlinks, and synchronized deletes do not immediately and permanently delete your files.

## Download and install

Download the installer for your device from [GitHub Releases](https://github.com/kanice888-max/LanBridge/releases/latest):

| Platform | Download | Notes |
| --- | --- | --- |
| macOS Intel | `LanBridge_0.2.0_x64.dmg` | For Intel-based Macs. |
| macOS Apple Silicon | `LanBridge_0.2.0_aarch64.dmg` | For Apple M-series Macs. |
| Windows x64 | `.exe` or `.msi` | Use `.exe` for a personal install or `.msi` for managed deployment. |

The macOS app is not notarized by Apple. If macOS blocks the first launch, Control-click LanBridge.app and choose **Open**, or select **Open Anyway** in System Settings → Privacy & Security. You do not need to disable your Mac's security protection. See the [macOS installation guide](docs/release/macos-installation.md) and [Windows installation guide](docs/release/windows-installation.md) for details.

## Quick start

1. Open LanBridge on both devices and make sure they are on the same trusted network.
2. Connect through discovery or a manual IP address, and make sure you are connecting to your own device.
3. Create a sync task, choose the Primary folder, then accept the task and choose the target folder on the other device.
4. Create or edit a test file on the Primary and confirm it appears on the Secondary.
5. Edit a file on the Secondary. Review it in Pending Returns, then return it only when you are ready.

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

## Security boundary

Use LanBridge only on trusted home or office networks, and make sure you are connecting to your own device. The app stores device identity locally; the receiving device must accept a task before its folder can be used; conflicts and returns always require an explicit choice.

- [Sync and data-safety invariants](docs/rules/invariants.md)
- [Security policy](SECURITY.md)
- [Security hardening plan](docs/security/security-hardening-plan.md)

Please do not disclose security issues publicly. Follow [SECURITY.md](SECURITY.md) to report them privately.

## Project layout

```text
src/              React frontend
src-tauri/        Rust / Tauri backend and integration tests
docs/             Product, architecture, security, and workflow documentation
scripts/          Project utility scripts
```

## License

LanBridge is released under the [MIT License](LICENSE).
