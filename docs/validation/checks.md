# Validation Checks

Run the smallest set of checks that proves the change. Use the worktree that owns the change.

## Integration Release Gate

在全新依赖环境执行：

```bash
npm ci --include=optional
npm run lint:names
npm run build
npm test
cargo test --manifest-path src-tauri/Cargo.toml
git diff --check <base>...HEAD
npm run tauri build -- --debug
```

当前 GitHub macOS 发布渠道使用免费 ad-hoc 签名、但不使用 Developer ID 且不公证；付费签名和公证不作为发布门槛。
发布时仍必须执行 `docs/release/macos-installation.md` 中的 Gatekeeper 手动放行、受保护
目录授权、重启验证，并为 DMG 生成和发布 `shasum -a 256` 校验值。不得引导用户全局关闭
Gatekeeper。安装包由发布者在真实设备上从发布标签手动构建并上传；GitHub Actions 只校验版本并
创建草稿 Release，不构建或上传安装包。

CI 在 Ubuntu 执行前端、命名、差异与依赖安全检查；在 Windows 与 macOS 执行 Rust 测试和原生 debug build。以下专项失败时不得发布：existing-target replacement、symlink/Junction 越界、未授权 TaskRegister、Secondary delete return、Keep Both baseline、partial/lease cleanup、commit journal recovery、V1 safe fallback、peer disconnect state persistence/revision ordering。

## Windows Worktree

```powershell
cd "<repo>\worktrees\windows"
npm ci --include=optional
npm run preflight:win
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
npm ci --include=optional
npm run package:mac
```

发布前还要在 Intel 与 Apple Silicon 各验证一次 DMG；具体安装与 Gatekeeper 说明见
[`docs/release/macos-installation.md`](../release/macos-installation.md)。

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
10. 更新已有文件并确认旧版本进入唯一 overwritten history。
11. 传输中断线并重启，确认 partial/lease 清理与 commit journal 恢复。
12. 验证 Keep Both 重试只产生一个冲突副本，删除冲突恢复 Primary 且不再重复扫描。
13. Windows 验证文件锁重试和 Junction；macOS 验证 APFS replacement 和 symlink 拒绝。
14. macOS 全新用户验证 ad-hoc 签名、未公证 DMG 手动放行后，本地网络及受保护目录授权行为。
15. Windows 主动断开后重启，确认 Windows 仍保持本机断开，Mac 显示“对端已主动断开”。
16. Windows 恢复后确认 Mac 无需重启即恢复；交换角色后重复一次。
17. 双方分别主动断开，确认必须双方分别恢复；再验证单端/双端重启、断网重连和乱序重发不会回滚状态。

## Validation Note Template

```text
Changed:
Checks run:
Passed:
Not validated:
Follow-up risk:
```
