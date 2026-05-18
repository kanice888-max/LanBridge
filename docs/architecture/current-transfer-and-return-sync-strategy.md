# 当前传输策略与待回传/冲突技术方案

更新日期：2026-05-18

## 目的

本文整理 LanBridge 当前已经落地的传输策略，以及待回传、冲突、历史恢复、重复同步防护和调度优先级的技术方案。它用于后续实现和 review 时统一判断，不替代更底层的协议说明 `docs/architecture/transfer-protocol.md`。

LanBridge 仍然是 **Primary-Secondary + 显式回传** 模型，不是自动双向同步：

- Primary 的新增、修改、删除自动同步到 Secondary。
- Secondary 的新增、修改、删除只进入待回传列表。
- Secondary 到 Primary 必须由用户显式点击回传。
- 冲突只在回传前判断。
- 不静默覆盖，覆盖前必须备份 Primary 旧文件。

## 当前传输策略

### 1. 连接与认证

每次传输使用已配对设备之间的认证 TCP 流：

1. 通过 UDP multicast 或手动 IP 找到对端地址。
2. TCP 连接后执行 `AuthHello`、`AuthChallenge`、`AuthProof`、`AuthOk`。
3. 服务端只接受已信任的 peer identity。
4. 文件传输、扫描、任务邀请、删除、下载请求都走认证消息。

失败策略：

- 未配对、身份不可信、认证失败：不重试。
- 网络中断、连接拒绝、timeout、unexpected EOF：按分类重试。
- 用户取消、冲突、路径非法、hash mismatch：不重试。

### 2. 协议协商

新版传输先尝试 V2：

```text
Sender -> Receiver: TransferHello { supported_versions: [2, 1], preferred_version: 2 }
Receiver -> Sender: TransferReady { selected_version, max_chunk_size, ack_interval_bytes }
```

当前策略：

- 首选 V2。
- 协商成功后按 `device_id` 缓存协议版本。
- 协商失败、对端断开、返回旧消息或 timeout 时，关闭当前认证流，重新打开认证流走 legacy V1。
- V2 不能没有 V1 fallback。

### 3. V2 binary 传输

V2 用 JSON 控制帧 + 原始二进制 payload：

- 控制帧仍是 `4-byte length + JSON`。
- 文件块使用 `FileChunkBinaryV2` 描述 offset、bytes、ack。
- JSON header 后紧跟原始 payload bytes。
- chunk size 当前为 4MB。
- 每 16MB checkpoint ACK 一次。
- 发送端边读文件边更新 Blake3 hasher。
- `FileStreamEndV2` 携带最终 hash。
- 接收端写入 `.lanbridge-partial`，校验大小和 hash 后原子 rename。

适用路径：

- Primary -> Secondary 自动同步。
- Secondary -> Primary 显式回传。
- Secondary pull / download from Primary。

### 4. 新版 V1 JSON 传输

新版 V1 仍使用 JSON `FileChunk`，但已经做低风险优化：

- chunk size 为 1MB。
- 新版 V1 上传不再预读全文件 hash。
- `FileChunkStart.file_hash` 为空字符串表示最终 hash 会在 `FileChunkEnd.file_hash` 给出。
- 发送端边读边 hash。
- checkpoint ACK 间隔为 16MB。
- 接收端写入 partial，完成时校验 hash 后 rename。

### 5. Legacy V1 fallback

为了兼容旧安装包，fallback 会保守使用旧合约：

- 发送前预先计算完整 hash。
- `FileChunkStart.file_hash` 携带完整 hash。
- 每个 `FileChunk` 等待一次 `FileAck`。
- `FileChunkEnd.file_hash` 为空。

这是刻意保留的兼容路径，不应拿它代表新版 V1 性能。

### 6. 取消与 deferred

用户取消传输后：

1. 当前发送端/下载端停止继续发送。
2. 对端接收到取消或连接失败后清理 `.lanbridge-partial`。
3. 该 task/path 被标记为 deferred。
4. 后续同路径传输会被拒绝，避免取消后立刻被重试覆盖。
5. UI 顶部弹出提示：
   - 继续：清除 deferred，重新触发同步。
   - 不继续：本轮应用会话中保持搁置。

这不是断点续传。真正 pause/resume 需要持久 chunk manifest、offset 协商、块级 hash 和 stale partial GC，属于后续 P4。

## 待回传与冲突模型

### 1. 概念定义

待回传：

> 副机有新内容或删除请求，主机还没有应用。需要用户明确发回主机。

冲突：

> 两边都改了同一个路径。回传前发现主机版本也相对 baseline 变了，需要用户选择保留哪边。

安全策略：

> 不静默覆盖。若用户选择用副机版本覆盖主机，必须先备份主机旧文件到 `.lanbridge-history/overwritten/`。

### 2. 待回传生成规则

Secondary 扫描本机文件后，与本地 baseline 比较：

| Secondary 状态 | Baseline 状态 | Planner 动作 |
| --- | --- | --- |
| 新文件 | 无 baseline | `MarkPendingReturn(Created)` |
| 文件内容变化 | 有 baseline 且 secondary side 改变 | `MarkPendingReturn(Modified)` |
| 文件缺失 | 有 baseline | `MarkPendingReturn(Deleted)` |
| 与 baseline 一致 | 有 baseline | `Noop` |

实现位置：

- Planner 负责生成 `MarkPendingReturn`。
- Executor 将其写入 `pending_return_changes`。
- UI 通过待回传列表展示。

### 3. 冲突判断规则

冲突不在扫描阶段直接覆盖待回传概念，而是在用户执行回传前判断。

回传前读取：

- 待回传项 `PendingReturnChange`
- 当前 Primary snapshot
- 上一次同步 baseline

判断：

| 情况 | 结果 |
| --- | --- |
| 无 baseline + Secondary 新建 + Primary 当前不存在 | 无冲突 |
| 无 baseline + Primary 同路径存在 | 冲突 |
| 有 baseline + Secondary 修改 + Primary 当前不存在 | 冲突：主机删除，副机修改 |
| 有 baseline + Secondary 删除 + Primary 当前不存在 | 无冲突，清理 pending/baseline |
| 有 baseline + Secondary 新建 + Primary 当前不存在 | 无冲突 |
| 有 baseline + Primary hash 与 baseline primary hash 一致 | 无冲突 |
| 有 baseline + Primary hash 与 baseline primary hash 不一致 | 冲突 |
| hash unavailable | fallback 到 size + modified time，并标记 hash-unverified |

修改时间不是唯一依据；有 verified hash 时 hash 优先。

实现要求：本地回传和 Secondary 通过网络显式回传都必须复用同一套冲突判断规则，不能各自维护一份简化版 hash 比较逻辑。

### 4. 回传执行规则

用户可单独回传某个文件，也可批量回传安全项。

创建/修改：

1. 回传前检测冲突。
2. 无冲突：Secondary 发送文件到 Primary。
3. Primary 写入 partial，校验后落盘。
4. 双端更新 baseline。
5. 移除对应 pending return。

删除请求：

1. 回传前检测 Primary 是否相对 baseline 改变。
2. 无冲突：Secondary 向 Primary 发送 `FileDelete`。
3. Primary 将目标移入 `.lanbridge-history/trash/`。
4. 双端移除对应 baseline / pending 状态。
5. Primary 已经不存在时，视为成功清理本地 pending 状态。

冲突：

- 不自动回传。
- UI 显示冲突提示。
- 用户可选择：
  - 保留两份：Secondary 文件以 conflict-safe 名称写入 Primary。
  - 使用副机版本：先备份 Primary 当前文件，再覆盖。
  - 取消：保留 pending return。

## 重复同步防护

目标：任一方向成功同步后的文件，不应被对端马上重复同步回来。

核心机制是 baseline：

- 成功写入目标端后，写入 `sync_baselines`。
- baseline 同时记录 primary side 和 secondary side 的 hash / hash_status / size / modified time。
- 下一轮 planner 看到当前文件与 baseline 一致，应生成 `Noop`。

### Primary -> Secondary

流程：

1. Primary 扫描并计划 `ApplyToSecondary`。
2. Secondary 接收文件并校验。
3. Secondary 更新本地 snapshot/baseline。
4. Primary 收到 ACK 后更新本地 baseline。
5. 如果 Secondary 之前有同路径 pending return，成功 pull/receive 后应清掉同路径 pending，避免 bounce back。

### Secondary -> Primary

流程：

1. Secondary pending return 被用户选择。
2. 回传成功后，Secondary 移除 pending row。
3. Primary 写入文件后更新 baseline。
4. Secondary 本地也更新 baseline，使该版本被视为双方一致。
5. 下一轮 Primary 自动同步或 Secondary 扫描不应重新计划同文件。

### 删除请求

Secondary delete request 成功后：

- Primary 文件进入 history/trash。
- Secondary 删除 pending row。
- 双端移除该 path baseline。
- 下一轮 Primary 不应重新把同文件推回 Secondary，除非 Primary 上又重新生成了同路径文件。

## 历史恢复策略

历史项展示的是“仍可恢复”的文件。

恢复成功后：

1. 文件从 `.lanbridge-history/...` 移回原路径。
2. 如果原路径已占用，恢复到 conflict-safe restored 名称。
3. DB 中对应 history entry 被移除。
4. UI 重新加载 history list。
5. 该卡片应消失。

恢复结果后续应在日志或已恢复记录中可查，而不是继续留在“可恢复卡片”列表里。

## 调度优先级方案

当前代码已有不同入口：

- 用户点击回传：`execute_return_sync`
- 用户继续取消文件：清除 deferred 后调用 `syncNow`
- 用户手动同步：`syncNow`
- 主机自动同步：前端 3 秒 polling 调用 `syncNow`
- 状态检查：peer status / discovery / progress polling

但当前还没有一个中心化任务队列严格表达所有优先级。目标调度顺序应为：

1. 用户显式点击的回传
2. 用户选择继续的取消文件
3. 用户手动点击的主机同步
4. 主机自动同步
5. 后台状态检查

### 建议实现

新增 per-task scheduler，所有会触发同步的入口都提交 `SyncIntent`：

```rust
enum SyncIntentKind {
    ExplicitReturnSync,
    ResumeDeferredTransfer,
    ManualPrimarySync,
    AutoPrimarySync,
    BackgroundStatusCheck,
}

struct SyncIntent {
    task_id: Uuid,
    kind: SyncIntentKind,
    selected_paths: Vec<String>,
    created_unix_ms: i64,
}
```

优先级：

| Intent | Priority |
| --- | ---: |
| `ExplicitReturnSync` | 100 |
| `ResumeDeferredTransfer` | 80 |
| `ManualPrimarySync` | 60 |
| `AutoPrimarySync` | 40 |
| `BackgroundStatusCheck` | 10 |

执行规则：

- 同一 task 同一时间只跑一个文件级执行器。
- 高优先级 intent 可排到低优先级 intent 前面。
- 同一路径操作不能交错。
- 已在运行的文件传输不抢占，除非用户取消。
- return-sync 的 selected paths 只执行用户选择的路径。
- auto-sync 不应吞掉 pending return；Secondary 普通 `sync_now` 只负责发现 pending，不自动回传。

### 与现有 `SyncRunCoordinator` 的关系

当前 `SyncRunCoordinator` 只能做到：

- 同一 task 已有 sync 运行时，第二次 sync 标记为 queued。
- 当前 run 结束后再跑一次。

它不能表达：

- 回传比自动同步优先。
- 用户继续取消文件比自动同步优先。
- 状态检查最低优先级。
- selected paths 的队列隔离。

因此建议保留 `SyncRunCoordinator` 的互斥能力，但在其前面加 priority queue。

## UI 文案方案

待回传列表：

- 标题：`待回传`
- 空状态：`暂无待回传内容`
- 说明：`副机有新内容，主机还没有。`
- 删除请求：`副机已删除，是否同步删除请求到主机？`

冲突提示：

- 标题：`两边都改了这个文件`
- 说明：`需要选择保留哪边。`
- 安全提示：`不会静默覆盖。覆盖主机前会先备份主机旧文件。`
- 操作：
  - `保留两份`
  - `用副机版本覆盖主机（先备份）`
  - `取消`

结果状态：

- 回传成功：`已回传`
- 删除请求成功：`主机文件已移入历史`
- 冲突未处理：`需要先处理冲突`
- 已搁置：`已搁置，稍后可继续`

## 当前完成度

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| V2 binary 传输 | 已实现 | 上传、下载、协商、fallback 已覆盖测试 |
| 新版 V1 checkpoint ACK | 已实现 | 新版上传路径生效，legacy fallback 保守兼容 |
| 取消/deferred | 已实现 | 不是断点续传 |
| 副机新增/修改进入待回传 | 已实现 | 支持单个和批量回传 |
| 副机删除进入 delete request | 已实现 | 不自动删除 Primary |
| 回传前冲突判断 | 已实现 | hash 优先，fallback size/mtime |
| 历史恢复后卡片消失 | 已实现 | restore 后移除 history DB entry 并刷新列表 |
| 成功同步后 baseline 防重复 | 已实现核心逻辑 | 仍需继续用实机流程验证边缘情况 |
| UI 文案完全按新简化稿 | 部分完成 | 需要再按本文文案做 UI copy 收口 |
| 严格调度优先级 | 未完全实现 | 需要新增中心 priority queue |

## 验证建议

自动测试：

- `cargo test --manifest-path src-tauri/Cargo.toml --test scanner_planner_history`
- `cargo test --manifest-path src-tauri/Cargo.toml --test e2e_full_flow`
- `cargo test --manifest-path src-tauri/Cargo.toml --test pairing_transfer`

实机验证：

1. Secondary 新增文件，只出现在待回传，不自动到 Primary。
2. Secondary 修改文件，待回传列表显示 modified。
3. Secondary 删除已同步文件，待回传列表显示 delete request，Primary 不自动删除。
4. Primary 同路径修改后，Secondary 回传同路径显示冲突。
5. 冲突选择保留两份，Primary 目录出现 conflict-safe 文件名。
6. 冲突选择用副机覆盖，Primary 旧文件进入 `.lanbridge-history/overwritten/`。
7. 任一方向成功同步后，下一轮 scan/sync 不重复生成同路径待回传或自动同步。
8. 历史恢复成功后，可恢复卡片消失。
9. 用户取消传输后，对端 partial 清理；继续后重新同步，不继续则保持 deferred。
10. 自动同步运行期间点击回传，后续 priority queue 实现后应保证回传优先。
