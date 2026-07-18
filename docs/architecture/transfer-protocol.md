# LanBridge 传输协议与技术架构

> 更新日期：2026-05-16

## 1. 总体架构

```
┌─────────────────┐          ┌─────────────────┐
│  设备 A (主端)   │          │  设备 B (副端)   │
│                 │  TCP/IP  │                 │
│  UDP 组播发现 ◄─┼──────────┼─► UDP 组播发现  │
│  TCP 9527 ◄─────┼──────────┼──── TCP 9527   │
│                 │          │                 │
│  [核心模块]      │          │  [核心模块]      │
│  ├─ Scanner     │          │  ├─ Scanner     │
│  ├─ Planner     │          │  ├─ Planner     │
│  ├─ Executor    │          │  ├─ Executor    │
│  ├─ Transport   │          │  ├─ Transport   │
│  ├─ State/DB    │          │  ├─ State/DB    │
│  └─ History     │          │  └─ History     │
└─────────────────┘          └─────────────────┘
```

### 技术栈

| 层 | 技术 | 用途 |
|---|------|------|
| 桌面框架 | Tauri v1 | 跨平台窗口 + 原生功能调用 |
| 后端语言 | Rust (edition 2021) | 性能、安全、并发 |
| 前端 | React 18 + TypeScript | 用户界面 |
| 数据库 | SQLite (rusqlite + bundled) | 持久化状态 |
| 加密 | Ed25519 (ed25519-dalek) | 身份认证、消息签名 |
| 哈希 | blake3 | 文件完整性验证 |
| 文件监听 | notify 6 | 文件变化监听 |
| 序列化 | serde_json | V1 协议消息 |
| 异步运行时 | tokio (full) | 网络 I/O、并发任务 |

---

## 2. 设备发现 (UDP 组播)

### 协议

```
组播组：  239.10.10.10
端口：    53530
间隔：    每 5 秒发送一次 announce
超时：    15 秒（3 次未收到标记离线）
```

### Announce 消息 (JSON, UDP)

```json
{
  "device_id":   "a1b2c3d4e5f6...64hex",
  "display_name": "MacBook Pro",
  "public_key":   [32 bytes],
  "port":         9527
}
```

### 流程

1. 设备启动后，同步服务器监听 TCP 端口
2. 发现服务获取实际端口，通过 UDP 组播广播 announce
3. 每接口绑定一个 UDP socket，加入组播组
4. 收到其他设备 announce → 记录 peer + 地址
5. 15 秒未收到更新 → 移出在线列表

### 地址评分

多地址场景下，设备按优先级排序：

| IP 类型 | 加分 |
|---------|:--:|
| 私有 IP (10/172.16/192.168) | +100 |
| 公网 IP | +10 |
| 环回/link-local | -100 |
| VPN/虚拟网卡 | 额外 -40 惩罚 |

### 手动连接

自动发现失败时，用户可直接输入 IP 和端口：

1. `ping_peer_address(ip, port)` → 连 TCP 发 Ping 收 Pong
2. `request_peer_identity(ip, port)` → 获取对端 `{ device_id, public_key }`
3. 存储连接记录

---

## 3. 身份与认证

### 设备身份

每个设备启动时自动生成或加载 Ed25519 密钥对：

```
存储路径：
  macOS:  ~/Library/Application Support/LanBridge/identity.key
  Windows:  %APPDATA%/LanBridge/identity.key

格式：   32 字节签名密钥（raw binary）
device_id： 验证公钥的 64 字符 hex 编码
```

### 配对流程

```
设备 A                          设备 B
  │                               │
  │─── PairRequest ──────────────►│
  │   { device_id, nonce }       │
  │◄── AuthChallenge ────────────│
  │   { nonce }                  │
  │─── AuthProof ────────────────►│
  │   { signature }              │
  │◄── AuthOk ───────────────────│
  │                               │
```

配对验证码：
```
SHA256("lanbridge-pairing-v1" || nonce || min_pubkey || max_pubkey)
              → 取前 4 字节 → 模 1,000,000 → 6 位验证码
```

两端的公钥排序后输入，保证验证码一致。

### 认证握手

每次 TCP 连接的认证流程：

```
客户端                         服务端
  │                              │
  │─── AuthHello ──────────────►│  (声明身份)
  │   { device_id }             │
  │◄── AuthChallenge ──────────│  (32 字节随机 nonce)
  │   { nonce }                 │
  │─── AuthProof ──────────────►│  (签名 nonce + device_id)
  │   { signature }             │
  │◄── AuthOk / AuthReject ────│
  │   { reason? }               │
```

认证 payload：
```
"lanbridge-auth-v1:" || device_id || ":" || nonce
          ↓
device.sign(payload) → signature
```

### 连接意图同步

手动断开是按可信设备保存的持久化状态，不等同于网络不可达。双方分别保存本机意图和对端意图；只有两者均允许连接时，任务扫描、传输、删除和冲突操作才可执行。

认证后可交换以下控制消息：

```text
PeerDisconnect { device_id, state_revision? }
PeerDisconnectAck { device_id, state_revision? }
PeerReconnect { device_id, state_revision? }
PeerReconnectAck { device_id, state_revision? }
```

- `state_revision` 仅在本机意图变化时递增；重复消息幂等，较小 revision 被忽略。
- 旧版消息缺少 revision 时只按当前会话处理，不覆盖已持久化的新状态。
- 启动后发布当前本机状态；失败按 1、5、15、60 秒退避重试，收到 ACK 后停止。
- 手动断开期间仍允许 Ping、身份认证和以上控制消息，以便原发起端恢复；其他任务消息返回 `PeerDisconnected`。

---

## 4. 传输协议

### 4.1 协议协商

每次文件传输开始时，通过 `TransferHello` / `TransferReady` 协商协议版本：

```
发送端                         接收端
  │                              │
  │─── TransferHello ──────────►│
  │   { versions: [2,1],        │
  │     preferred: 2 }          │
  │◄── TransferReady ──────────│
  │   { selected_version: 2 }   │
```

- 超时：**1 秒**（原为 5 秒）
- 缓存：首次协商结果按 `device_id` 缓存，后续传输跳过协商
- 协商失败时：关闭当前认证流，重新打开认证流后按 legacy V1 合约传输

### 4.2 V1 协议（JSON 逐块）

#### 消息格式

所有消息使用 `4 字节长度前缀 + JSON 正文`：

```
┌──────────┬──────────────────────────┐
│ 长度 4B  │ JSON 消息体              │
│ (大端)   │ (serde_json)             │
└──────────┴──────────────────────────┘
```

#### 上传流程（新版 V1）

```
发送端                         接收端
  │                              │
  │─── FileChunkStart ─────────►│
  │   { file_hash: "", total_bytes }
  │◄── FileAck ────────────────│  (准备就绪)
  │─── FileChunk ──────────────►│  (逐块，每块 1MB)
  │   { offset, data: [bytes] }│  ⚠️ data 为 JSON 数组
  │           ...               │
  │◄── FileChunkAck ───────────│  (每 16MB 检查点确认)
  │           ...               │
  │─── FileChunkEnd ──────────►│
  │   { file_hash }            │
  │◄── FileAck ────────────────│  (完成)
```

Legacy V1 fallback 会保留旧合约：`FileChunkStart` 携带完整 hash，并且每个 `FileChunk` 等待一次 `FileAck`。这样旧安装包仍可接收新版发送端的文件。

#### 性能特征

| 指标 | 值 |
|------|------|
| 块大小 | 1 MB |
| ACK | 新版 V1 每 16MB checkpoint；legacy fallback 每块 ACK |
| JSON 膨胀 | 3-4 倍（二进制 → JSON 整型数组） |
| 发送前哈希 | 新版 V1 上传流式 hash；legacy fallback 和兼容下载路径仍可能预读取 |
| 大文件回退 | >100MB 时跳过哈希，用 size+mtime |

#### 瓶颈

V1 的主要剩余瓶颈是 JSON 承载二进制数据造成的序列化和体积膨胀。以下比例来自优化前判断，不能直接代表当前新版 V1/V2 实测：

| 因素 | 贡献 | 说明 |
|------|:--:|------|
| JSON 膨胀 3-4x | ~40% | 1MB → 3-4MB 线上传输 |
| 逐块 ACK RTT | legacy only | 新版 V1/V2 已改为 checkpoint ACK |
| JSON 序列化/反序列化 | ~15% | serde_json CPU 开销 |
| 发送前哈希 | legacy/兼容路径 | 新版上传和 V2 走流式 hash |
| 文件 I/O | ~15% | 同步读写 |

### 4.3 V2 协议（二进制混合）

#### 消息格式

V2 消息分为 **JSON 控制帧**（与 V1 相同格式）和 **二进制数据帧**：

```
JSON 控制帧：
┌──────────┬──────────────────────────┐
│ 长度 4B  │ JSON 消息体              │
│ (大端)   │ (serde_json，通常 <200B) │
└──────────┴──────────────────────────┘

二进制数据帧（FileChunkBinaryV2）：
┌──────────┬──────────────┬─────────────────┐
│ 长度 4B  │ JSON 头      │ 原始二进制载荷  │
│ (大端)   │ (~150B)      │ (4MB = 原始bytes)│
└──────────┴──────────────┴─────────────────┘
```

#### 上传流程 (V2)

```
发送端                             接收端
  │                                  │
  │─── FileStreamStartV2 ──────────►│
  │   { total_bytes }               │  (JSON 控制帧)
  │─── FileChunkBinaryV2 ──────────►│  (二进制帧)
  │   { offset, bytes, ack:false }  │  纯原始字节跟在 JSON 头后
  │           ...                    │  流水线发送，不等 ACK
  │─── FileChunkBinaryV2 ──────────►│
  │   { offset, bytes, ack:true }   │  每 16MB 发一次 ACK 请求
  │◄── FileStreamAckV2 ────────────│  (检查点确认)
  │           ...                    │
  │─── FileStreamEndV2 ───────────►│
  │   { file_hash }                 │  (JSON 控制帧)
  │◄── FileStreamAckV2 ────────────│  (确认)
```

#### 关键差异点

| 特性 | V1 | V2 |
|------|:--:|:--:|
| 数据编码 | JSON 整型数组（3-4x 膨胀） | 原始字节（零膨胀） |
| 流控 | 新版 checkpoint；legacy 逐块 ACK | 流水线 + 检查点（每 16MB） |
| 哈希 | 新版上传流式；legacy/兼容路径可预读 | 流式 blake3（边发边算） |
| 每块开销 | 高（JSON 序列化/反序列化） | 低（字节直接复制） |
| 源文件不变性 | 发送前和发送后各检查一次 | 同左 |
| 协商 | 无 | TransferHello/TransferReady |

#### 性能优势

V2 的理论 Gigabit LAN 吞吐可达 **80-110 MB/s**（接近线速），因为：

- 原始字节传输（无膨胀）
- 流水线发送（无逐块 RTT）
- 流式哈希（无预读）
- JSON 头极小（每块 ~150B vs V1 的 ~3-4MB）

#### V2 当前限制

- ACK 检查点间隔 16MB（可调）
- 流水线发送无中间 ACK（尾端哈希验证）
- 暂无持久断点续传；网络中断后仍从文件开头重试

### 4.4 文件下载双向对称性

LanBridge 的文件传输是对称的——同一段代码处理两个方向：

| 场景 | 实际发送端 | 发送函数 | 接收处理位置 |
|------|-----------|---------|-------------|
| 主→副同步 | 主设备 | `send_authenticated_file_to_peer` → V1/V2 | `server.rs handle_connection` |
| 副→主回传 | 副设备 | `send_file_with_retry` → 同上 | 同上 |
| 副端拉取 | 主设备 | `send_file_download` / `send_file_download_v2` | `connection.rs request_*` |
| 主端响应 | 副设备 | 同上 | 同上 |

---

## 5. 同步执行流程

### 完整链路

```
用户点击"同步"
  │
  ├─ 1. Scanner: 扫描本地文件
  │     ├─ walkdir 遍历 sync_root
  │     ├─ 跳过忽略条目（.git, node_modules, .DS_Store...）
  │     ├─ 记录 symlink（跳过不跟踪）
  │     ├─ 小文件 (<100MB) → blake3 哈希
  │     └─ 大文件 (≥100MB) → UnverifiedLargeFile 标记
  │
  ├─ 2. 远程扫描（主端角色）
  │     ├─ ScanRequest → TCP 发送到副端
  │     ├─ 副端扫描自身文件
  │     └─ ScanResponse 返回文件列表
  │
  ├─ 3. Planner: 对比 baseline 生成动作
  │     ├─ 主端新文件 → ApplyToSecondary
  │     ├─ 主端修改 → ApplyToSecondary
  │     ├─ 主端删除 → MoveSecondaryToHistory
  │     ├─ 副端新增/修改 → MarkPendingReturn
  │     └─ 副端删除 → MarkPendingReturn（显式 delete request）
  │
  ├─ 4. Executor: 执行动作
  │     ├─ 排序：先删除/目录/小文件 → 后大文件
  │     ├─ 主端→副端：网络发送
  │     │    ├─ 目录 → DirectoryCreate 消息
  │     │    ├─ 文件 → V2/V1 + 重试（3 次，150/300ms）
  │     │    └─ 删除 → FileDelete 消息（副端移入回收站）
  │     ├─ 副端标记 pending：DB 写入 PendingReturnChange
  │     └─ 副端拉取：从主端下载文件
  │
  └─ 5. Baseline 更新
        ├─ 成功 → 写入 SyncBaseline（记录两端 hash/mtime）
        └─ 失败 → 记录日志、返回 UI 错误列表
```

### 重试策略

| 错误类型 | 重试 | 说明 |
|---------|:--:|------|
| 网络超时/中断 | 3 次 | 分类后重试 |
| 文件被锁定 | 3 次 | 等待后重试 |
| 权限不足 | 不重试 | 立即失败 |
| 路径无效 | 不重试 | 立即失败 |
| 哈希不匹配 | 不重试 | 数据已损坏 |
| 用户取消 | 不重试 | 进入 deferred/cancelled 交互 |

---

## 6. 回传同步（Return Sync）

### 触发条件

副端文件创建/修改/删除后，同步扫描会检测到并从 baseline 比对发现变化，
生成 `MarkPendingReturn` 动作写入 `pending_return_changes` 表。

### 手动执行

```
用户选择文件 → 点击"回传同步"
  │
  ├─ 1. 冲突检测: 比对当前主端 snapshot vs baseline
  │     ├─ 主端 hash == baseline hash → 无冲突
  │     ├─ 主端 hash != baseline hash → 冲突
  │     └─ 无 baseline + 主端文件存在 → 冲突
  │
  ├─ 2. 无冲突 + 创建/修改 → 直接发送（副端→主端）
  │
  ├─ 3. 无冲突 + 删除请求 → 发送 FileDelete，主端移入 history
  │
  └─ 4. 有冲突 → 用户选择：
        ├─ 使用副端版本：旧主端文件备份到 .lanbridge-history/overwritten/
        └─ 保留两者：副端文件以冲突名写入
```

### 冲突文件名格式

```
<stem> (conflict from <device-name> <YYYY-MM-DD HHmmss>)<extension>
冲突不覆盖已有文件，自动追加 `-2`, `-3`...
```

---

## 7. 历史与回收站

### 存储结构

```
<sync_root>/.lanbridge-history/
  ├── trash/               ← 主端删除产生
  │   └── <unix-ms>/
  │       └── <relative_path>
  └── overwritten/         ← 冲突覆盖产生
      └── <unix-ms>/
          └── <relative_path>
```

### 保留策略

| 限制 | 默认值 | 说明 |
|------|:--:|------|
| 大小 | 1 GB | 超过时阻止新删除操作 |
| 时间 | 30 天 | 超过时建议清理但阻塞（等用户主动清理） |

### 恢复

- 恢复文件到原始路径
- 路径被占用 → 使用 `(restored <YYYY-MM-DD HHmmss>)` 后缀
- 支持通过 UI 查看和恢复

---

## 8. 数据传输安全

### 完整性保证

```
发送端                          接收端
  │                              │
  │  1. 发送前检查源文件不变     │
  │  2. 流式 blake3 累计哈希     │
  │  3. 结束时发送累计哈希       │
  │                              │
  │                              │ 4. 写入临时文件 .lanbridge-partial
  │                              │ 5. 计算接收数据 blake3
  │                              │ 6. 比对哈希
  │                              │ 7. 匹配 → 原子重命名为目标文件
  │                              │ 8. 不匹配 → 删除临时文件
```

### 路径安全

所有接收路径经过 `safe_join` 检查：

- 拒绝 `..` 路径遍历
- 拒绝空组件、尾随空格/点
- 拒绝 Windows 保留名（CON, PRN, COM1-9, LPT1-9）
- 拒绝控制字符和非法字符（`< > : " | ? * \0`）
- 拒绝组件长度超过 255 字符（Windows）

### 防篡改

- `ensure_source_file_unchanged` — 发送前后验证源文件大小 + mtime 不变
- `transfer progress` 日志包含即时 mbps、已发送字节、运行时间

---

## 9. 错误模型

### 错误枚举

```rust
enum AppError {
    PeerOffline,           // 对端不在线
    FolderMissing,         // 同步目录不存在
    PermissionDenied,      // 权限拒绝
    DiskFull,              // 磁盘空间不足
    FileLocked,            // 文件被锁定
    HashMismatch,          // 哈希不匹配
    InvalidPath,           // 路径无效
    CaseCollision,         // 大小写冲突（Windows/双系统）
    NetworkInterrupted,    // 网络中断
    ConflictRequired,      // 冲突需用户决策
    HistoryLimitReached,   // 历史存储已满
}
```

### 错误处理原则

- 所有文件级操作原子化（每个文件成功/失败独立上报）
- 失败文件不清除已成功的文件
- 网络错误标记 `retryable: true`
- 权限/路径错误标记 `retryable: false`
- 错误在 UI 中明确可见，不静默跳过

---

## 10. 性能数据

### 目标与待实测数据

| 场景 | 历史 V1 基线 | 新版 V1/V2 目标 |
|------|:------:|:----------:|
| 1GB 单文件 | ~13-15 MB/s | 80-110 MB/s |
| 100MB 单文件 | ~15 MB/s | 80-110 MB/s |
| 1000 个 1KB 小文件 | ~200 files/s | ~200 files/s |
| WiFi 5GHz (802.11ac) | ~8-10 MB/s | ~30-40 MB/s |

这些数字用于验收和对比，当前仍需要按 `docs/testing/transfer-performance-e2e.md` 填写真实实机结果。

### 瓶颈排名

| 排名 | 因素 | 严重程度 |
|:--:|------|:--------:|
| 1 | V1 JSON 数据膨胀 3-4x | 🔴 |
| 2 | 小文件公平调度 | 🟡 P3 |
| 3 | legacy V1 逐块 ACK | 🟡 仅旧版本兼容路径 |
| 4 | 文件 I/O 同步阻塞 | 🟢 |
| 5 | 服务端连接管理 | 🟢 |

---

## 11. 配置与常量

| 参数 | 值 | 说明 |
|------|:--:|------|
| 默认端口 | 9527 | SyncServer TCP 端口 |
| 发现组播 | 239.10.10.10:53530 | UDP 组播地址 |
| 发现间隔 | 5s | announce 发送间隔 |
| peer 超时 | 15s | 3 次未收到标记离线 |
| watcher 消抖 | 500ms | 文件变更累积间隔 |
| 大文件阈值 | 100 MB | ≥此值跳过急切哈希 |
| V1 传输块大小 | 1 MB | JSON 兼容路径单次读/写块 |
| V2 传输块大小 | 4 MB | 二进制路径单次读/写块，减少每块分配和系统调用 |
| ACK 间隔 | 16 MB | 新版 V1/V2 检查点频率 |
| 进度记录间隔 | 64 MB | 日志频率 |
| V2 协商超时 | 1s | TransferHello 等待时间 |
| 历史大小限制 | 1 GB | 每个同步任务 |
| 历史保留天数 | 30 天 | 自动清理阈值 |
| 日志保留 | 10000 条/7天 | 取更少值 |

---

## 12. 接收提交、兼容与恢复

### 目标前置条件

- `expected_target_hash = Some("")`：目标必须不存在。
- `Some(hash)`：提交前目标必须仍为该 hash。
- `None`：legacy 请求；仅允许创建缺失目标，已有文件更新/删除返回 `TargetPreconditionFailed`。
- 前置条件变化返回 `TargetChanged`，发送方重新扫描并进入冲突流程，禁止盲覆盖。

V1/V2 身份均可在认证后完成能力协商。V1 缺失能力使用安全默认值；V2 才使用二进制流和增强 ACK。`FileAck` 的 `resolution`、`conflict_path`、`primary_hash`、`secondary_hash` 及 `ConflictApply.resolution_id` 均为可选 serde 字段，旧 peer 可忽略。

### Incoming 隔离

接收状态以 `(connection_id, task_id, relative_path)` 定位，并以 `(task_id, relative_path)` lease 排他。每次传输在 `.lanbridge-temp/incoming/` 使用 UUID partial。错误连接不能 append、finish 或 cancel；断线、注销、超量 chunk、hash/flush/replace 失败统一清理句柄、进度、lease 和 partial。

### Durable commit

`transfer_commit_journal` 状态为 `Prepared → FilesystemCommitted → MetadataCommitted`：先写日志，再备份旧目标并原子替换，最后在 SQLite 事务中更新 snapshot、baseline、history 和 pending。启动时若最终文件 hash 等于 incoming hash，则补齐 metadata；否则清理废弃 partial 与日志。相同内容重试复用恢复记录，不返回伪 `TargetChanged`。

Windows 已实现 `ReplaceFileW` / `MoveFileExW`，sharing/lock violation 以 50/100/200/400/800ms 重试，失败返回带 `retryable` 与 `os_code` 的 `AtomicReplaceFailed`。真实 NTFS 行为仍以原生 CI 和双机验收为准。

### Keep Both

Primary 以 `resolution_id` 幂等应用 Secondary staging，并在 ACK 返回唯一冲突路径和双方 hash。Secondary 先下载并验证 Primary 临时文件，再创建/校验 Secondary 冲突副本，最后原子提交 Primary 到原路径；`conflict_resolution_journal` 支持从 `RemoteApplied` 继续，避免重试上传已被替换的原路径。
