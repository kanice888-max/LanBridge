# LanBridge Windows Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Windows client after the macOS baseline is stable, reusing shared sync semantics while implementing Windows-specific filesystem, watcher, tray, firewall, startup, and packaging behavior.

**Architecture:** Start from the macOS/shared baseline. Add a Windows platform layer behind the same platform abstraction, then verify Mac-to-Windows and Windows-to-Mac primary/secondary roles using the same encrypted transport and sync engine. Verify UDP multicast auto-discovery works on Windows with proper firewall handling.

**Tech Stack:** Rust, Tauri, React, TypeScript, SQLite, Tokio, notify on Windows/ReadDirectoryChangesW, blake3, tokio-rustls, socket2, local-ip-address, Windows tray/autostart integration, Vitest, Rust tests, Playwright.

---

## 1. Worktree

All Windows work happens in:

- Path: `.worktrees/windows`
- Branch: `codex/lanbridge-windows`

Do not start this plan until the macOS branch has a passing baseline and stable shared interfaces.

All Windows commands in this plan assume PowerShell unless stated otherwise.

## 2. Windows-Specific Requirements

- Use app data directory for identity, SQLite state, logs, and peer pins.
- Use `.lanbridge-history/` inside each sync root for trash and overwritten files.
- Use `notify` on Windows, backed by ReadDirectoryChangesW.
- Use scanner fallback because watcher events may be incomplete during sleep, heavy writes, or network interruptions.
- Ignore `Thumbs.db`, `desktop.ini`, `$RECYCLE.BIN`, `System Volume Information`, exact directory `.git/`, exact directory `node_modules/`, temporary Office lock files, Windows shortcut files, and `.lanbridge-history/` by default.
- Handle Windows path restrictions, including reserved names, invalid characters, drive roots, long paths, trailing spaces, and trailing dots.
- Treat path comparison carefully because default Windows filesystems are case-insensitive.
- Do not follow or synchronize symlinks, junctions, or reparse points in P0. Record skipped entries as warnings.
- Provide tray entry with open, pause all, sync now, and quit.
- Provide clear UI for Windows firewall/network permission issues.
- Support startup-at-login after explicit user opt-in.
- Prepare MSI/NSIS packaging later; P0 can use development builds.

## 3. Target File Structure

```text
src-tauri/src/
├── platform/
│   ├── mod.rs
│   └── windows/
│       ├── mod.rs
│       ├── fs_rules.rs
│       ├── watcher.rs
│       ├── app_dirs.rs
│       ├── tray.rs
│       ├── firewall.rs
│       └── startup.rs
```

## 4. P0 Windows Task Plan

### Task 1: Rebase From macOS Baseline

**Files:**
- Modify only as needed to resolve Windows compile issues.

- [ ] **Step 1: Verify worktree**

Run:

```powershell
git branch --show-current
```

Expected: `codex/lanbridge-windows`.

- [ ] **Step 2: Bring in macOS baseline**

Merge or rebase from the completed macOS branch according to repository policy.

Expected: shared core, transport, state, history, commands, and UI files exist.

The Windows worker must not redesign shared interfaces from scratch. Implement the existing platform abstraction created by the macOS baseline, then request integration review if the abstraction is insufficient.

- [ ] **Step 3: Install dependencies**

Run:

```powershell
npm install
```

Expected: dependencies install without errors.

- [ ] **Step 4: Run baseline tests**

Run:

```powershell
npm test
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: failures, if any, are platform-specific and listed before implementation continues.

### Task 2: Implement Windows Platform Layer

**Files:**
- Create: `src-tauri/src/platform/windows/mod.rs`
- Create: `src-tauri/src/platform/windows/app_dirs.rs`
- Create: `src-tauri/src/platform/windows/fs_rules.rs`
- Create: `src-tauri/src/platform/windows/watcher.rs`
- Test: `src-tauri/tests/core/windows_fs_rules_test.rs`

- [ ] **Step 1: Implement app directory resolution**

Store app state under the Windows app data directory. The platform API must return paths for database, logs, identity key, and peer pins.

Implement the shared platform interface from `src-tauri/src/platform/traits.rs`. Do not call macOS platform helpers from Windows code.

- [ ] **Step 2: Implement Windows ignore rules**

Default exact file/name matches:

```text
Thumbs.db
desktop.ini
```

Default exact directory matches:

```text
$RECYCLE.BIN
System Volume Information
.git/
node_modules/
.lanbridge-history/
```

Default glob patterns:

```text
~$*
*.tmp
*.lnk
```

The `.git/` rule is an exact directory match and does not ignore `.gitignore`, `.gitmodules`, or `.github/`. Windows shortcut files (`*.lnk`) are skipped because their target paths are machine-local and usually invalid on the peer device; record a warning when skipping them.

- [ ] **Step 3: Implement Windows path validation**

Reject invalid Windows path segments containing:

```text
< > : " | ? *
```

Reject reserved device names:

```text
CON
PRN
AUX
NUL
COM1
COM2
COM3
COM4
COM5
COM6
COM7
COM8
COM9
LPT1
LPT2
LPT3
LPT4
LPT5
LPT6
LPT7
LPT8
LPT9
```

Reject target paths with trailing spaces or trailing dots. Reject reserved names case-insensitively by checking the filename stem before the first `.`, so `CON`, `CON.txt`, and `con.out` are all invalid. Reject drive roots such as `C:\` as sync roots in P0. Normalize internal relative paths to `/` separators while using native separators for filesystem operations.

P0 must not silently fail on Windows long paths. If long path support is not enabled through the app manifest and runtime configuration, reject target paths above the safe Windows path length before transfer and show a visible error. P1 may add `longPathAware` and `\\?\` support.

- [ ] **Step 4: Implement case-insensitive collision detection**

If two incoming relative paths differ only by case, such as `Readme.md` and `README.md`, block sync and show a path collision error instead of overwriting either file. Detect collisions during Windows scans and again immediately before writing an incoming transfer from macOS.

- [ ] **Step 5: Implement watcher wrapper**

Use `notify` and debounce events for 500 ms before sending scan requests to the core engine. Watcher events trigger scans, not direct sync decisions.

- [ ] **Step 6: Test Windows rules**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml windows_fs_rules
```

Expected: invalid path, reserved name, ignore rule, and case collision tests pass.

### Task 3: Implement Windows Tray, Startup, And Firewall Guidance

**Files:**
- Create: `src-tauri/src/platform/windows/tray.rs`
- Create: `src-tauri/src/platform/windows/startup.rs`
- Create: `src-tauri/src/platform/windows/firewall.rs`
- Modify: `src-tauri/src/commands.rs`
- Test: `src-tauri/tests/core/windows_platform_test.rs`

- [ ] **Step 1: Implement tray menu**

Menu items: Open App, Pause All, Sync Now, Start At Login, Quit.

- [ ] **Step 2: Implement startup-at-login setting**

Only enable startup after explicit user opt-in. Store the setting in app config. Use the current-user startup mechanism, such as `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` or the equivalent Tauri plugin capability, and do not require administrator privileges.

- [ ] **Step 3: Implement firewall guidance detection**

When listening or connecting fails with likely firewall errors, return a structured error with Windows-specific help text. Do not silently retry forever. P0 must not automatically create firewall rules or request elevation. It may show guidance text and a copyable PowerShell command that the user can run manually as administrator.

Additionally, detect when UDP multicast discovery fails (no peers discovered but network is available) and show guidance that Windows Firewall may be blocking multicast UDP on port 53530. Provide a copyable PowerShell command:

```powershell
New-NetFirewallRule -DisplayName "LanBridge Discovery" -Direction Inbound -Protocol UDP -LocalPort 53530 -Action Allow
```

- [ ] **Step 4: Test platform commands**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml windows_platform
```

Expected: startup setting and firewall error mapping tests pass.

### Task 4: Verify Shared Sync Engine On Windows Filesystem

**Files:**
- Create: `src-tauri/tests/core/windows_scanner_planner_history_test.rs`
- Modify shared core only if platform abstraction requires it.

- [ ] **Step 1: Test scanner with Windows paths**

Verify scanner handles normal drive paths, nested folders, ignored system files, skipped reparse points, and normalized relative paths. Drive roots such as `C:\` are rejected as sync roots in P0.

- [ ] **Step 2: Test history movement**

Verify primary delete moves secondary file into:

```text
.lanbridge-history/trash/<unix-ms>/<relative-path>
```

- [ ] **Step 3: Test overwrite backup**

Verify confirmed return-sync overwrite moves old primary file into:

```text
.lanbridge-history/overwritten/<unix-ms>/<relative-path>
```

- [ ] **Step 4: Test locked file behavior**

Simulate a file that cannot be opened for reading or writing by opening it with an exclusive lock during the test, then attempting sync. Verify the app records a visible `File locked or permission denied` error and does not mark sync successful.

- [ ] **Step 5: Run Windows core tests**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml windows_scanner_planner_history
```

Expected: Windows filesystem tests pass.

### Task 5: Verify Auto-Discovery On Windows

**Files:**
- Create: `src-tauri/tests/transport/windows_discovery_test.rs`

- [ ] **Step 1: Test multicast socket on Windows**

Verify that `socket2` correctly configures UDP multicast on Windows: join group `239.10.10.10`, bind to port `53530`, set `SO_REUSEADDR`. Verify two local instances discover each other.

- [ ] **Step 2: Test local IP detection on Windows**

Verify `local-ip-address` returns a valid LAN IP on Windows. If multiple adapters exist, verify the announce message includes a reachable IP. This validates the crate's Windows compatibility, not the crate's internal selection logic.

- [ ] **Step 3: Test firewall blocked scenario**

Simulate or manually block multicast UDP and verify the app detects no peers and shows the firewall guidance message with a copyable PowerShell command.

- [ ] **Step 4: Test manual IP fallback**

Verify manual IP connection works on Windows when multicast is blocked, using the existing TLS pairing flow.

**Known issue:** 手动 IP 模式将 IP 地址字符串当作 `peer_device_id`，`approvePairing` 存储的不是真正的 Ed25519 device_id。后续实现 TLS 握手后需用真实 device_id 替换。详见技术开发索引 "Auto-Discovery Known Issues" 第 2 条。

- [ ] **Step 5: Run discovery tests**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml windows_discovery
```

Expected: multicast discovery works on Windows, firewall guidance appears when blocked, manual fallback succeeds.

### Task 6: Verify TLS Transport And Pairing On Windows

**Files:**
- Create: `src-tauri/tests/transport/windows_pairing_transfer_test.rs`
- Modify transport only if Windows socket behavior requires it.

**Pre-condition:** 在此任务中必须将 `SyncServer` 集成到 `main.rs`，启动后获取实际端口，再传入 `DiscoveryService`。当前 DiscoveryService 端口硬编码 `9527`，需在此任务中修正。详见技术开发索引 "Auto-Discovery Known Issues" 第 1 和第 3 条。

- [ ] **Step 1: Test persistent identity storage**

Verify device identity survives restart and is stored under Windows app data.

- [ ] **Step 2: Test manual IP loopback connection**

Start two local peers on random ports and verify encrypted connection succeeds after pairing.

- [ ] **Step 3: Test file transfer**

Transfer a file through the protocol, verify partial file cleanup, final hash, and atomic replacement.

- [ ] **Step 4: Test firewall error mapping**

Mock or simulate connection denied/timeouts and verify structured errors reach UI.

- [ ] **Step 5: Test Windows port binding behavior**

Verify the transport layer detects port conflicts explicitly and does not rely on Unix `SO_REUSEADDR` behavior.

- [ ] **Step 6: Run transport tests**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml windows_pairing_transfer
```

Expected: Windows transport tests pass.

### Task 7: Windows UI Adaptation

**Files:**
- Modify: `src/features/settings/SettingsScreen.tsx`
- Modify: `src/features/pairing/PairingScreen.tsx`
- Modify: `src/features/sync-task/TaskDetail.tsx`
- Create: `tests/ui/windows_smoke.spec.ts`

- [ ] **Step 1: Use UI-UX-Pro-Max before Windows UI adaptation**

Before changing Windows UI or copy, the worker must use the `UI-UX-Pro-Max` skill. Use it to adapt the macOS baseline without creating a separate visual language. Focus on Windows firewall guidance, invalid path messaging, startup-at-login settings, tray behavior, error states, and safety-critical confirmation copy.

Expected output in the worker notes: Windows-specific UX decisions, copy tone, error hierarchy, and any differences from the macOS baseline.

- [ ] **Step 2: Add Windows-specific copy**

Pairing and connection errors must mention firewall/network permission when appropriate. When multicast discovery returns no results, show a message suggesting firewall may be blocking UDP multicast with a link to settings or a copyable fix command.

- [ ] **Step 3: Add invalid path messages**

When a user selects or receives an invalid Windows target path, show the exact invalid segment and reason.

- [ ] **Step 4: Add startup-at-login setting**

Expose the explicit opt-in toggle through the existing settings UI.

- [ ] **Step 5: Adapt task proposal UI for Windows folders**

When accepting a sync task proposal, validate the selected Windows folder with Windows path rules and empty-folder rules before accepting. Show case-collision, long-path, reserved-name, and drive-root errors before task creation.

- [ ] **Step 6: Test UI**

Run:

```powershell
npm test
```

Expected: UI tests pass.

### Task 8: Windows Build Verification

**Files:**
- Modify: Windows-specific Tauri config only if required
- Create: `docs/windows-build-notes.md`

- [ ] **Step 1: Build Windows app**

Run:

```powershell
npm run tauri build
```

Expected: Windows development build succeeds.

- [ ] **Step 2: Document Windows runtime notes**

Document firewall prompt behavior, app data location, startup setting behavior, and known P0 limitations.

- [ ] **Step 3: Commit Windows platform work**

```powershell
git add -A
git commit -m "feat: add Windows platform support"
```

## 5. Integration Handoff

Windows branch is ready for integration when:

- Windows platform tests pass.
- Shared core tests pass on Windows.
- Transport loopback tests pass on Windows.
- UI smoke tests pass on Windows.
- Windows app builds.
- Windows runtime notes are documented.

## 6. Integration Scenarios To Verify Later

- Mac discovers Windows via UDP multicast and vice versa.
- Mac primary to Windows secondary.
- Windows primary to Mac secondary.
- Mac primary delete moves Windows secondary file to history.
- Windows primary delete moves Mac secondary file to history.
- Windows secondary change returns manually to Mac primary.
- Mac secondary change returns manually to Windows primary.
- Case collision from Mac to Windows is blocked.
- Windows invalid path is rejected before transfer.
- Manual IP fallback works when multicast is blocked on either side.
