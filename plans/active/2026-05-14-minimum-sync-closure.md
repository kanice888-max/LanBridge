# Minimum Sync Closure

## Goal

Stabilize the smallest real-device flow:

```text
发现设备 → 连接 → 配对/身份 → 建任务 → 计划同步 → 传输协议 → 接收落盘 → ACK/重试/状态收尾
```

## Current Scope

- Primary to Secondary should auto-sync through the current Dashboard polling trigger.
- Secondary to Primary should return-sync immediately after the user clicks "回传到主机".
- Receiver task invites should refresh without manual user refresh.
- Windows and macOS worktrees should keep shared sync logic aligned.

## Current Status

- P0 transfer progress, immediate sync feedback, and transfer cancellation are implemented.
- P1 classified retry, generated-file stability preflight, and explicit secondary delete requests are implemented.
- P1/P2 transfer throughput work is implemented in code and covered by automated tests.
- Remaining work is primarily real-device validation, durable retry queue/replay, backend-managed auto-sync scheduling, and future P3/P4/P5 transfer features.

## 2026-05-17 Transfer Control Update

- Implemented active progress on both endpoints for send/serve and receive/download paths.
- Added a lightweight sync-stage progress state so the UI shows feedback immediately after `sync_now` starts, before the first file transfer begins.
- Added transfer cancellation plumbing: UI cancel button, `cancel_transfer` Tauri command, `TransferCancel` protocol message, sender/download cancellation checks, and receiver-side cleanup of `.lanbridge-partial` files.
- Kept true pause/resume out of scope for now. Resume needs persisted chunk manifests, offset negotiation, hash checkpoints, and stale-partial garbage collection; cancellation is the safer first milestone.
- Superseded by the P1 hardening update below: secondary delete is now modeled as an explicit pending return/delete request.
- Superseded by the P1 hardening update below: generated-file stability preflight is implemented.

## 2026-05-17 P1 Hardening Update

- Added the detailed P1 implementation document: `docs/architecture/p1-transfer-sync-hardening.md`.
- Added classified retry for authenticated scan, upload, and download. Non-retryable cases such as auth failure, conflict, invalid path, hash mismatch, and user cancellation fail fast.
- Added a source-file stability preflight for recently modified upload files. Files that keep changing fail as retryable instead of being marked successfully synced.
- Changed Secondary deletes from silent Noop to explicit pending return/delete requests.
- Guarded regular Secondary `sync_now` so delete requests remain pending and are not automatically propagated to Primary.
- Explicit return-delete sends `FileDelete` to Primary; Primary moves the file to history, and Secondary clears pending/baseline state only after success.

## 2026-05-17 Disconnection And Return-Sync UX Update

- Removed global sync-stage progress rows from the frontend. The global progress surface now shows only real file transfer progress/speed plus cancelled-transfer prompts.
- Added per-task peer status polling. If the peer is disconnected, the task detail disables sync and Primary auto-sync skips network sync attempts until the peer responds again.
- Primary auto-sync now performs a peer liveness check before `sync_now`; when a previously disconnected peer becomes reachable, the next 3-second poll immediately triggers sync.
- Cancelled transfers are now marked deferred. Incoming uploads/download serves reject the same task/path while deferred, so a receiver-side cancel cannot be immediately overwritten by the peer retrying.
- The cancelling side shows a top prompt: continue clears the deferred marker and starts sync again; declining leaves that path deferred for this app session.
- Secondary `sync_now` no longer auto-return-syncs local changes. New/modified/deleted secondary files stay in the pending return list until the user explicitly syncs them.
- Pending return list now supports one-file return-sync. Bulk selection still works for safe items.
- Successful pulls from Primary clear same-path pending return rows, preventing stale return items from bouncing a just-synced file back to Primary.
- Successful history restore removes the restored history card from the list.
- Conflict text is simplified around one decision: keep both, or use the Secondary version after backing up Primary.

## 2026-05-18 P1/P2 Transfer Optimization Update

- Completed P1/P2 backend protocol hardening in both Windows and macOS worktrees.
- Newer V1 uploads avoid the pre-transfer full-file hash, stream Blake3 while sending, and wait only for checkpoint ACKs instead of every chunk.
- Legacy V1 fallback intentionally keeps the old start-hash and per-chunk ACK contract so older packaged builds can still receive files.
- V2 upload/download use negotiated binary payload frames with checkpoint ACKs and V1 fallback on negotiation failure.
- Added protocol and authenticated transfer tests for V1 checkpoint ACKs, V2 upload, V2 download, and V2 fallback.

## Validation Already Used

- Windows `cargo test --manifest-path src-tauri/Cargo.toml`
- Windows `npm run build`
- Windows `npm run lint:names`
- Windows `npm run tauri build`
- macOS worktree `cargo test --manifest-path src-tauri/Cargo.toml`
- macOS worktree `npm run build`
- macOS worktree `npm run lint:names`

## Remaining Real-Device Checks

- Windows to Windows full invite and sync flow.
- Windows to macOS full invite and sync flow with the current packaged builds.
- Restart persistence for trust, tasks, registered roots, and pending return-sync.
- Real-device transfer performance rows in `docs/testing/transfer-performance-e2e.md`.
