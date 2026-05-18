# P1 Transfer And Sync Hardening

Date: 2026-05-17

## Implemented Behavior

P1 improves safety and user clarity without changing LanBridge into fully bidirectional sync. The behaviors below are implemented in the current macOS and Windows worktrees.

- Network interruption should retry only retryable operations.
- Files that are still being generated should not be marked as successfully synced.
- Secondary-side deletion should become an explicit pending return/delete request instead of being silently restored without explanation.

## Completed Scope

1. Classified retry for scan/upload/download.
2. Source-file stability preflight before transfer.
3. Secondary delete as explicit pending return/delete.
4. True pause/resume remains out of scope.

## Classified Retry

Retryable:

- connection refused/reset/aborted
- broken pipe
- timed out
- unexpected EOF
- peer disconnected
- temporary network failure
- file changed while preparing transfer

Not retryable:

- authentication rejected
- peer is not trusted
- conflict requires user decision
- hash mismatch
- invalid path
- permission denied
- transfer cancelled
- remote path exists or changed since last sync

Implementation:

- Add a single retry classifier in the Tauri sync command layer.
- Use it for authenticated scan, upload, and download.
- Keep primary delete and directory create single-shot because retrying a delete after a timeout can duplicate history moves.

## Generated File Stability

Transfer protection verifies the source file did not change during transfer and now also performs a preflight:

- Read source metadata.
- If the file was modified very recently, wait a short sample interval.
- Re-read metadata.
- If size or modified time changed, fail as retryable with a clear message.
- If it stayed stable during the sample, allow the transfer.

This avoids transferring actively written files while still allowing fresh but stable files to sync.

## Secondary Delete Pending Request

Existing invariant stays intact: secondary delete does not affect primary automatically.

New behavior:

- If a secondary baseline entry is missing locally, planner emits `MarkPendingReturn`.
- Executor records `PendingReturnChange { change_kind: Deleted }`.
- Return-sync UI shows the item as a delete request.
- Only after the user selects it and clicks return-sync does the app send `FileDelete` to the primary.
- Primary receive side moves the file to history, not permanent deletion.
- After successful return-delete, secondary removes the baseline and pending row so future sync does not restore the file.

Conflict rule:

- If primary still matches baseline, delete request is safe.
- If primary changed since baseline, delete request is blocked as conflict.
- If primary is already missing, return-delete is treated as success and local pending state is cleared.

## Validation

- Planner tests for secondary delete pending request.
- Executor tests for deleted pending rows.
- E2E tests for secondary delete request and return-delete cleanup.
- Existing full Rust test suites on Windows and macOS worktrees.
- Frontend build and naming lint on both worktrees.
