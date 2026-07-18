# LanBridge Invariants

These rules protect user data. Do not weaken them without updating the PRD and tests.

## Sync Semantics

- Primary create/update syncs to Secondary.
- Primary delete moves Secondary content to history/trash, never permanent delete first.
- Secondary create/update becomes pending return-sync.
- Secondary delete does not affect Primary.
- Secondary return-sync requires explicit user action.
- The app must not describe the product as fully bidirectional sync.

## Conflict And Safety Rules

- Never overwrite a changed Primary file silently.
- Never use modification time as the only conflict detector.
- Hash comparison is authoritative when hashes are verified.
- Confirmed overwrite must back up the old Primary file before replacement.
- Failed files must be visible in results/logs.
- Directories should not be treated as file transfer actions; create parent directories as needed for files.

## Transport Rules

- TCP services must be running before discovery advertises a port.
- Authenticated operations must verify paired/trusted peer identity.
- File receive must write to a temporary path, verify, then atomically rename where possible.
- Large files must use chunked/streaming transfer rather than single JSON payloads.
- Receiver state should be updated after successful receive/ACK where supported.
- File hashing and scanning must be streaming/heap-buffered, not backed by large stack arrays. A stack overflow can crash packaged Windows builds without reaching the Rust panic hook.
- LanBridge runtime files and folders must not enter the sync model. At minimum, `.lanbridge-history`, `.lanbridge-temp`, partial files, `lanbridge.log`, `startup-crash.log`, and `crash-diagnostics.log` must be ignored by scanner, watcher, transfer, and pending-return flows.
- A remote task registration may only confirm an already approved `(task, peer, root)` tuple; it must never create an arbitrary remote-selected root.
- Incoming paths may not address `.lanbridge-history`, ordinary `.lanbridge-temp`, diagnostics, logs, or the partial suffix. Conflict staging is a separate restricted operation.
- One `(task, relative path)` may have only one incoming writer. Append, finish, cancel, and disconnect cleanup are scoped to the owning connection.
- Every receive uses a unique partial file. Size/hash/precondition validation must complete before replacement, and every non-committed exit removes the partial and releases its lease.
- `expected_target_hash = Some("")` means “must be missing”; a non-empty value is CAS; a missing precondition is legacy and may not overwrite or delete an existing target.
- Existing targets are copied to a unique, flushed overwritten-history entry before atomic replacement. Filesystem and metadata commit are joined by a durable recovery journal.
- Symlinks and Windows reparse points are not valid path components for network mutation, delete, conflict application, or history restore.
- Manual disconnect intent is peer-scoped and durable. Local and remote intent are independent, and task traffic is allowed only when both sides allow it.
- Discovery, Ping, identity/authentication, and connection-state control may continue while manually disconnected; sync task operations must return `PeerDisconnected`.
- Only the device that set its local disconnect intent may clear that intent. Repeated or stale state messages may not roll back a newer revision.

## Conflict And Return-Sync State

- Secondary manual deletion retains baseline and pending-delete state until Primary ACK succeeds.
- Keep Both keeps Primary at the original path and Secondary at the server-selected conflict path.
- Keep Both is idempotent by `resolution_id`; retries resume the same resolution and may not create another conflict copy.
- A successful resolution updates both baseline/snapshot paths and clears pending state transactionally.
- Delete-conflict Keep Both restores Primary content, records that Secondary's delete intent was abandoned, and stops re-emitting the delete.

## UI Rules

- The receiver chooses its own local folder for task invites.
- The sender should not require users to type the peer machine's absolute path.
- Pairing and task creation must surface waiting, rejected, and error states.
- Conflict, history, overwrite backup, and retryable error states must be visible.
- Preview Tauri commands must use the same argument shape as real commands and return explicit values for pairing confirmation.
