# Validation Checks

Run the smallest set of checks that proves the change. Use the worktree that owns the change.

## Windows Worktree

```powershell
cd "<repo>\worktrees\windows"
npm run lint:names
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

Useful targeted checks:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --test e2e_full_flow
cargo test --manifest-path src-tauri/Cargo.toml --test e2e_full_flow secondary_sync_now
```

## macOS Worktree

On Windows, run compile/test level checks:

```powershell
cd "<repo>\worktrees\macos"
npm run lint:names
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

On a real Mac, also run:

```bash
npm install
npm run tauri build
```

## Real-Device Smoke Flow

1. Start both devices on the same network.
2. Confirm discovery or manual connection.
3. Send a task invite.
4. Confirm receiver sees invite without manual refresh.
5. Receiver chooses a local folder and accepts.
6. Primary creates a file; confirm it reaches Secondary.
7. Secondary creates a file; click return-sync; confirm it reaches Primary.
8. Create same-name conflicts; confirm no silent overwrite.
9. Restart both apps; confirm task, trust, and registered roots still work.

## Validation Note Template

```text
Changed:
Checks run:
Passed:
Not validated:
Follow-up risk:
```
