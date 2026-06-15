# Local `tao` Patch

LanBridge currently vendors a local patch for `tao` at:

```text
src-tauri/patches/tao-0.16.11/
```

This folder is required because `src-tauri/Cargo.toml` uses a `[patch.crates-io]` override:

```toml
[patch.crates-io]
tao = { path = "patches/tao-0.16.11" }
```

Do not remove this directory when preparing the open-source repository.

## Why It Exists

LanBridge uses Tauri 1.x, which depends on the `tao` windowing stack. The local patch keeps the app compatible with LanBridge's current desktop window behavior while the project stays on the React 18 / Vite 6 / Tauri 1.6 baseline.

The patch is intentionally scoped to the vendored `tao` crate. It should not change LanBridge sync, transfer, history, pairing, or file-safety behavior.

## Maintenance Rules

- Treat the patch as platform/runtime infrastructure, not product logic.
- Keep changes minimal and documented in Git commits.
- Re-run macOS and Windows smoke tests after touching it.
- Do not update Tauri, Wry, or Tao major versions just to remove the patch.

## Upgrade Checklist

Before removing or replacing the patch:

- Confirm the upstream `tao` version contains the needed behavior.
- Build and run LanBridge on macOS and Windows.
- Verify window close-to-tray behavior, system tray behavior, aspect-ratio resizing, and normal app startup.
- Run `cargo test --manifest-path src-tauri/Cargo.toml`.
- Run `npm run build`.
- Remove the `[patch.crates-io]` override only after the patched behavior is no longer needed.

## Release Note

This vendored source is part of the build input and should be uploaded to GitHub with the application source. It is not a generated artifact, installer, cache, or temporary dependency folder.
