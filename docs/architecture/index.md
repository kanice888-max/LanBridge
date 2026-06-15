# Architecture Overview

LanBridge is a local desktop sync application for Windows and macOS. It uses Tauri, React, TypeScript, Rust, SQLite, TCP transport, UDP discovery, and platform-specific filesystem rules.

## Product Model

LanBridge is not general bidirectional sync. It uses a fixed primary-secondary model:

- Primary create/update changes sync to Secondary.
- Primary deletes move Secondary files into `.lanbridge-history`.
- Secondary create/update changes become pending return-sync items.
- Secondary deletes become explicit pending return/delete requests and do not affect Primary automatically.
- Secondary-to-Primary return-sync requires explicit user action.
- Conflicts never overwrite silently.
- Confirmed overwrites must back up the old Primary file first.

## Runtime Flow

1. Discovery advertises device identity and TCP port.
2. Connection pins a real peer device identity and address.
3. Pairing/trust stores peer identity and server trust.
4. Task invite lets the receiver choose its own local folder.
5. Planning compares current scan results with baselines.
6. Transfer sends authenticated messages or chunked file payloads.
7. Receiver writes to temporary paths, verifies content, then moves into place.
8. ACK updates sender state; receiver also updates local DB state where supported.
9. Retryable failures are surfaced to the UI instead of silently skipped.

## Key Boundaries

- UI lives in `src/`.
- Tauri commands live in `src-tauri/src/commands.rs`.
- Sync model and planning live in `src-tauri/src/core/`.
- Transport and authenticated file exchange live in `src-tauri/src/transport/`.
- SQLite state lives in `src-tauri/src/state/`.
- Platform path and ignore rules live in `src-tauri/src/platform/`.
- Real-device test instructions live in `docs/testing/` when present.
- Transfer protocol details: `docs/architecture/transfer-protocol.md`
- Current transfer strategy and return-sync/conflict plan: `docs/architecture/current-transfer-and-return-sync-strategy.md`
- Local Tauri/Tao runtime patch note: `docs/architecture/tao-local-patch.md`

## Current Automation Level

The current minimum auto-sync path is UI-triggered polling from the Dashboard for Primary tasks. A true background OS watcher is tracked as follow-up debt.
