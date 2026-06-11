# LanBridge Security Hardening Plan

This document tracks security work before and after the initial open-source release.

LanBridge is a trusted-LAN desktop sync tool. Security work must preserve the Primary/Secondary sync model: Primary syncs automatically to Secondary, while Secondary changes require explicit return-sync.

## Current Scope

The first open-source hardening pass focuses on low-risk, high-value fixes:

- Publish a clear security policy.
- Disable generic Tauri `shell.open`.
- Keep file-manager opening behind a path allowlist.
- Restrict local identity key permissions on macOS/Unix.
- Confirm task invites do not create trusted task roots before acceptance.
- Tighten the packaged-app CSP.

No database migration, sync protocol rewrite, or command permission framework is included in this pass.

## Adopted Changes

### Security Policy

`SECURITY.md` documents supported versions, vulnerability reporting, local-network assumptions, identity-key handling, and the data-safety model.

### Tauri Shell Allowlist

Generic `shell.open` is disabled. Frontend code should not open arbitrary URLs, files, or protocol handlers through the Tauri shell API.

The app keeps `open_in_file_manager(path)` as a controlled command. The command validates the requested path before delegating to Finder or Explorer.

Allowed paths:

- A configured task local root.
- A descendant of a configured task local root.
- A task-owned `.lanbridge-history` path.
- The LanBridge log and diagnostics directory.

Rejected paths:

- URL-like values such as `https://...` or `file://...`.
- Paths outside known task roots or the app diagnostics directory.
- Paths that cannot be canonicalized.

### Device Identity Key Permissions

On macOS and Unix-like systems, `identity.key` is created with `0600` permissions. Existing key files with broader permissions are repaired during startup.

Windows continues to store the key under the current user's application data directory.

### Task Invite Safety

Production startup keeps task-invite auto-accept disabled. A task invite must be accepted before LanBridge creates a task, registers a task root, or starts watching a folder.

Existing tests cover the disabled auto-accept path and should remain required when pairing code changes.

### Content Security Policy

The packaged app CSP is tightened so `connect-src` is limited to `'self'`. Development must continue to work through Tauri's dev configuration; if a future Tauri 1 limitation requires different dev/prod CSP handling, document it here before loosening production policy.

## Near-Term Plan

### SyncServer Port Fallback

Current behavior prefers the default TCP port. A future change should bind `9527` when available and fall back to a system-assigned port when needed. Discovery must advertise the real listening port, and the UI must show the real port.

### Discovery Privacy Mode

Add a setting for automatic discovery:

- On: broadcast on the LAN and listen for peers.
- Off: do not actively broadcast; manual connection remains available.

This reduces exposure on networks the user does not trust.

### CI Security Audit

Add GitHub Actions for:

- `npm ci`
- `npm run build`
- `cargo test`
- `npm audit`
- `cargo audit`

Initial audit rules may report or block only critical findings so the repository can converge without creating noisy false blockers.

### Local `tao` Patch Documentation

Document why `src-tauri/patches/tao-0.16.11/` is vendored, what was changed, and what must be verified before removing the patch.

## Deferred Items

### Unified Command Permission Layer

Commands such as delete, restore, import, history, logging, and file-manager actions should eventually share a central permission and path-safety layer. This is larger than the first open-source pass and should be handled as a focused refactor.

### Automatic Log, History, and Database Cleanup

LanBridge needs a retention strategy for logs, diagnostics, history entries, and database growth. This requires UX design for maximum size, retention time, manual cleanup, and restore expectations.

### Public Network Detection

The app should eventually warn when discovery appears to run on a public or untrusted network. This needs careful tuning to avoid noisy false positives and should be paired with discovery privacy mode.

## Review Checklist

Before release:

- `shell.open` remains disabled.
- `open_in_file_manager` rejects task-external paths.
- `identity.key` is not logged and has private permissions on macOS.
- Task invites do not create roots before acceptance.
- Packaged CSP does not include broad localhost or WebSocket wildcards.
- README links to `SECURITY.md`.
- No private keys, databases, logs, crash reports, or installers are tracked by Git.
