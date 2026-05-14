# LanBridge 实机端到端测试清单

目标：在一台 Windows 和一台 macOS 上验证完整用户链路，而不只验证本机单元测试。

覆盖流程：

`发现设备 -> 连接 -> 配对/身份 -> 建任务 -> 计划同步 -> 传输协议 -> 接收落盘 -> ACK/重试/状态收尾`

## 1. 测试前准备

### 构建版本

两端必须使用同一批源码构建出的包。构建前在各自平台运行：

```bash
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --test e2e_full_flow
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

本地双节点 E2E 只证明后端 TCP 闭环可用；实机仍要验证防火墙、权限、网卡、VPN 和 UI 命令链路。

### 测试目录

Windows 建议：

```powershell
.\scripts\real-device\windows-prepare.ps1
```

默认创建：

- `%USERPROFILE%\LANSyncE2E\source`
- `%USERPROFILE%\LANSyncE2E\target`
- `%USERPROFILE%\LANSyncE2E\manifest.txt`

macOS 建议：

```bash
chmod +x scripts/real-device/macos-prepare.sh
./scripts/real-device/macos-prepare.sh
```

默认创建：

- `$HOME/LANSyncE2E/source`
- `$HOME/LANSyncE2E/target`
- `$HOME/LANSyncE2E/manifest.txt`

### 应用数据位置

需要排查时优先看这些文件：

- Windows：`%APPDATA%\LanBridge`
- macOS：`$HOME/Library/Application Support/LanBridge`
- 数据库：`lanbridge.db`
- 日志文件：`lanbridge.log`
- 远端目录注册：`remote_task_roots.json`
- 待处理邀请：`pending_task_invites.json`

不要在测试中直接删除真实用户数据。需要干净环境时，先退出应用，再备份上述目录。

## 2. 网络前置检查

两端在同一局域网下先记录：

- Windows IP：`ipconfig`
- macOS IP：`ifconfig` 或 `ipconfig getifaddr en0`
- Windows 能否 ping macOS
- macOS 能否 ping Windows
- Windows 防火墙是否允许应用监听 TCP `9527`
- macOS 防火墙是否允许应用接受传入连接
- discovery 使用 UDP `239.10.10.10:53530` 和广播兜底
- 文件传输服务使用 TCP `9527`

如果自动发现失败，但手动 IP 能连接，问题集中在 UDP 组播/广播、网卡选择、防火墙或 VPN 路由。

## 3. 主流程测试：Windows -> macOS

1. 两端启动正式包。
2. 两端打开配对页面。
3. Windows 点击刷新发现设备。
4. 期望：列表出现 macOS，显示设备名、IP、端口。
5. Windows 连接 macOS。
6. 期望：连接成功，后端保存 macOS 的真实 device_id，不是临时 IP id。
7. 完成配对/信任。
8. 期望：两端 paired device 都有对端身份。
9. Windows 创建同步任务：
   - 本地目录：`%USERPROFILE%\LANSyncE2E\source`
   - 模式：单向备份，此电脑 -> 目标
   - 远端目录：优先走邀请/对端选择；若必须手填，则填 macOS 的 `$HOME/LANSyncE2E/target`
10. macOS 接受任务邀请并选择 target 目录。
11. Windows 点击立即同步。
12. 期望：
   - macOS target 出现 `small/hello.txt`
   - macOS target 出现 `nested/a/b/report.txt`
   - macOS target 出现 `many/file-001.txt` 到 `many/file-020.txt`
   - macOS target 出现 `large.bin`
   - Windows UI 显示成功 ACK
   - macOS 日志出现 received file
   - macOS DB 有对应 snapshots/baseline

校验 hash：

```bash
# macOS
(
  cd "$HOME/LANSyncE2E/target"
  find . -type f -not -path '*/.lanbridge-history/*' |
    LC_ALL=C sort |
    while IFS= read -r file; do
      shasum -a 256 "$file"
    done
)
```

```powershell
# Windows
Get-ChildItem "$env:USERPROFILE\LANSyncE2E\source" -Recurse -File |
  Where-Object { $_.FullName -notmatch '\\.lanbridge-history\\' } |
  Sort-Object FullName |
  Get-FileHash -Algorithm SHA256
```

## 4. 反向测试：macOS -> Windows

重复第 3 节，但由 macOS 发起任务：

- macOS source：`$HOME/LANSyncE2E/source`
- Windows target：`%USERPROFILE%\LANSyncE2E\target`

重点观察：

- Windows 是否弹防火墙提示
- Windows 路径是否合法
- Windows 是否拒绝保留名、非法字符、尾随空格/点
- Windows target 是否成功落盘并写入 DB 状态

## 5. 单向拉取测试：对方 -> 此电脑

1. 在设备 A 上选择“对方 -> 此电脑”。
2. 设备 B 的 source 内放入样本文件。
3. 设备 A 的 target 保持空目录。
4. 在 A 上点击立即同步。
5. 期望：A 主动扫描 B，下载 B 上的文件。
6. 如果 A 本地已有同名不同内容文件，期望：不覆盖，进入冲突/失败结果。

这是专门验证 Secondary 拉取逻辑的测试。

## 6. 删除与历史测试

1. 完成一次成功同步。
2. 在 Primary 删除 `small/hello.txt`。
3. 再次同步。
4. 期望：
   - Secondary 正常目录中不再有该文件。
   - Secondary `.lanbridge-history/trash/small/hello.txt` 存在。
   - Secondary DB 中该 snapshot 标记为 deleted。
   - 日志中出现 received delete。

## 7. 中断与重试测试

1. 准备 `large.bin`，建议至少 64MB。
2. 开始同步。
3. 传输中关闭接收端应用，或临时断开 Wi-Fi。
4. 期望：
   - 发送端显示失败，错误应可读。
   - 结果标记为可重试，不能误报成功。
   - 接收端不应留下最终文件；最多留下 partial 文件。
5. 恢复网络并重新点击同步。
6. 期望：最终文件 hash 一致，状态收尾成功。

## 8. 网络矩阵

至少覆盖：

- Windows Wi-Fi <-> macOS Wi-Fi
- Windows 有线 <-> macOS Wi-Fi
- Windows Wi-Fi <-> macOS 有线
- Windows 开 VPN，macOS 不开 VPN
- macOS 开 VPN，Windows 不开 VPN
- 两端都开 VPN
- 自动发现失败后，手动 IP 连接
- 路由器开启 AP 隔离时的失败表现

记录每组：

- 是否自动发现
- 是否手动 IP 可连接
- 配对是否成功
- 任务邀请是否成功
- 文件传输是否成功
- 是否出现错误提示

## 9. 失败定位顺序

### 发现不到设备

优先看：

- TCP 服务是否监听 `9527`
- UDP `53530` 是否被防火墙阻断
- VPN 是否抢默认路由
- 两端是否在同一网段
- 路由器是否开启 AP 隔离

### 能发现但连接失败

优先看：

- 对端 TCP `9527` 是否能连
- Windows 防火墙入站规则
- macOS 防火墙入站权限
- discovery 中展示的 IP 是否是 VPN/虚拟网卡地址

### 能连接但同步失败

优先看：

- paired device 是否存在
- remote_task_roots.json 是否包含任务 id
- target 目录是否存在、为空、可写
- DB 中 sync_tasks 是否两端都有
- event_logs 是否记录 received file / received delete

### UI 成功但文件没到

这是高优先级问题。必须同时收集：

- 两端 app 数据目录
- 两端 `lanbridge.db`
- 两端 `remote_task_roots.json`
- 两端 UI 截图
- 发生时间点
- 源文件 hash 和目标文件 hash

## 10. 通过标准

一轮测试通过必须满足：

- 自动发现成功，或手动 IP 兜底成功。
- 两端都能连接并建立可信身份。
- 任务邀请和接受可用。
- Windows -> macOS 成功。
- macOS -> Windows 成功。
- 单向拉取成功。
- 删除进入 history。
- 大文件 chunk 传输 hash 一致。
- 临时中断不会误报成功。
- 恢复后能重试成功。
- 两端 DB 状态和日志能反映最终结果。
