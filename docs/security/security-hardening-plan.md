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

### Rust Dependency Audit Exceptions

The CI audit blocks known vulnerabilities. Any narrowly scoped exception must be documented in
`docs/security/cargo-audit-exceptions.md` with dependency path, safety boundary, and expiry.

### SyncServer Port Fallback

Implemented plan: bind `9527` when available and fall back to a system-assigned port when needed. Discovery advertises the real listening port, and network diagnostics show the real port.

### Discovery Privacy Mode

Implemented plan: add a setting for automatic discovery:

- On: broadcast on the LAN and listen for peers.
- Off: do not actively broadcast; manual connection remains available.

This reduces exposure on networks the user does not trust.

### CI Security Audit

Implemented plan: add GitHub Actions for:

- `npm ci`
- `npm run build`
- `cargo test`
- `npm audit`
- `cargo audit`

Initial audit rules may report or block only critical findings so the repository can converge without creating noisy false blockers.

### Local `tao` Patch Documentation

Implemented plan: document why `src-tauri/patches/tao-0.16.11/` is vendored, what must be verified before removing the patch, and why the directory must remain in the GitHub source release. See `docs/architecture/tao-local-patch.md`.

## Deferred Items

### Unified Command Permission Layer

Commands such as delete, restore, import, history, logging, and file-manager actions should eventually share a central permission and path-safety layer. This is larger than the first open-source pass and should be handled as a focused refactor.

### Automatic Log, History, and Database Cleanup

LanBridge needs a retention strategy for logs, diagnostics, history entries, and database growth. This requires UX design for maximum size, retention time, manual cleanup, and restore expectations.

Crash diagnostics are bounded to 8 MiB with one rotated file; startup diagnostics are bounded to
1 MiB. Grossly oversized legacy diagnostics are discarded on the next write so an old unbounded
log does not remain on disk. Normal per-file and per-directory scan progress is not written to the
crash diagnostics channel.

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

## Transfer And Path Hardening (2026-07)

- `TaskRegister` 只接受本地已批准且 peer/root 完全匹配的任务。
- 普通网络传输拒绝 LanBridge 内部命名空间；冲突 staging 使用受限入口。
- 路径组件拒绝 symlink 与 Windows reparse point，并在创建父目录及最终 mutation 前重新核验边界。
- incoming 使用连接级状态、目标 lease 和 UUID partial；所有失败/断线/注销路径统一清理。
- legacy V1 不得覆盖或删除已有目标；V2 CAS 失败返回 `TargetChanged`。
- 接收提交使用持久化 journal，数据库迁移前为已有数据库创建备份。
- 运行日志使用 `UnsafePath`、`TransferAlreadyInProgress`、`TargetChanged`、`AtomicReplaceFailed`、`PartialCleanupFailed` 和 `LegacyProtocolFallback` 作为可检索事件名。

发布前仍必须在真实 Windows NTFS 验证 Junction/reparse point、文件锁重试与 `ReplaceFileW`，并在真实 macOS APFS 验证 symlink、替换、重启恢复和打包首次运行。本机或交叉编译不能替代这些结果。

## Peer Connection Intent (2026-07)

- 手动断开状态按可信设备持久化，本机与对端意图分别保存。
- 状态控制消息必须先完成可信设备认证，并使用单调 revision 防止延迟消息回滚。
- 断开期间仅保留 Ping、身份验证和状态控制通道；文件、目录、删除、扫描和冲突操作统一拒绝。
- 旧版无 revision 消息只按会话兼容，不得覆盖持久化的新状态。
