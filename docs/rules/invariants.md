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

## UI Rules

- The receiver chooses its own local folder for task invites.
- The sender should not require users to type the peer machine's absolute path.
- Pairing and task creation must surface waiting, rejected, and error states.
- Conflict, history, overwrite backup, and retryable error states must be visible.
