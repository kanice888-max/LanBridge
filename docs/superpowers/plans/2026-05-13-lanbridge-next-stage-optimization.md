# LanBridge 下一阶段优化开发计划

> **适用范围：** 本文档记录当前 Windows worktree 在完成基础发现、连接、配对身份、任务注册、网络传输、ACK 与基础状态收尾后的下一阶段开发内容。后续实现时应继续遵守测试优先：先写能失败的测试，再补实现。

## 1. 当前基线

当前实现已经具备以下基础闭环：

- TCP 同步服务器随应用启动。
- UDP discovery 可发现同局域网设备，并保留同设备多地址候选。
- 手动 IP 连接可通过 TCP 拉取对端公开身份，不再使用临时假身份。
- 已配对设备通过 `AuthHello/AuthChallenge/AuthProof/AuthOk` 鉴权。
- 创建任务支持 `TaskRegister`，也支持不填写对端路径时通过 `TaskInvite` 让对端自动分配 `incoming_tasks` 接收目录。
- 文件传输使用分块协议 `FileChunkStart/FileChunk/FileChunkEnd`，每步返回 `FileAck`。
- 接收端校验 task root、路径、大小与 blake3 hash 后原子落盘。
- Primary 同步前会请求远端 scan，用于发现潜在覆盖冲突。
- 现有回归测试覆盖真实 TCP 端到端流：发现、连接、身份、任务注册、计划、传输、ACK、落盘、远端扫描和 baseline 记录。

## 2. 产品优先级判断

下一阶段不追 Resilio Sync 的全部复杂度。当前最有收益的是把“用户真的能放心用”的链路补完整：

1. 对端确认任务邀请与选择目录。
2. 自动同步、队列、状态和进度。
3. 可视化冲突处理。
4. 持久重试与失败恢复。
5. 之后再考虑并发传输和 Delta Sync。

暂不建议投入：

- uTP 或自研拥塞控制。
- 多设备网状 P2P。
- WAN/NAT 穿透。
- 默认启用全自动双向同步。

这些能力开发成本高，且会显著扩大安全、冲突和一致性风险。

## 3. P0：对端确认任务邀请

### 2026-05-13 实施记录

已完成最小可用闭环：

- A 端创建任务改为发送 `TaskInvite`，无需填写 B 端路径。
- B 端 TCP server 收到邀请后进入 pending，不再在真实应用中自动接受。
- pending invite 已持久化到应用数据目录的 `pending_task_invites.json`，B 端重启后仍可看到未处理邀请。
- B 端 Dashboard 显示待处理邀请，可输入本机目录并接受或拒绝。
- B 端接受邀请前会校验本机目录必须已存在、是目录、且没有用户文件，避免误选非空目录造成覆盖风险。
- B 接受后会在 B 端持久化 `SyncTask` 并注册 task root。
- A 端通过 `TaskInviteStatusRequest` 轮询，收到 accepted 后持久化本机 `SyncTask`。
- A 端轮询可看到 B 端拒绝状态和拒绝原因。
- 新增集成测试覆盖 pending invite、B 端接受、A 端轮询、拒绝轮询、重启恢复、无效目录拒绝、随后文件传输落盘。

剩余收尾：

- 重复邀请、邀请超时/过期清理还需要继续补测试与持久化策略。
- 目录校验目前只做“已存在目录 + 空目录”最小保护，后续还应复用平台层危险路径、权限、大小写冲突和 ignore 规则。

### 目标

解决“发起方不知道对端路径”的体验问题，同时避免自动把对端目录藏在用户不知道的位置。

### 用户流程

1. A 选择 B 设备。
2. A 填写任务名、本机目录和同步方式。
3. A 发送 `TaskInvite`。
4. B 收到待处理邀请，在 UI 中显示：
   - 发起设备名。
   - 任务名。
   - 建议同步方向。
   - 选择本机接收目录按钮。
   - 接受、拒绝按钮。
5. B 接受后返回 `TaskInviteAck`，包含 B 选择的目录。
6. A 和 B 都持久化任务信息。

### 技术方案

- 在 TCP server 收到 `TaskInvite` 时，不再直接自动接受为最终行为，而是写入本地 pending invite store。
- 新增 Tauri command：
  - `list_task_invites`
  - `accept_task_invite(invite_id, local_path)`
  - `reject_task_invite(invite_id, reason)`
- 新增前端入口：
  - Dashboard 显示“待确认邀请”。
  - Pairing/Task 创建页显示“等待对端确认”状态。
- TCP 层需要支持邀请请求等待结果，或先返回 `TaskInvitePending`，再由发起方轮询/订阅最终结果。

### 风险

- 如果 TCP 请求一直等待 B 端操作，会阻塞连接并增加超时复杂度。
- 推荐先采用 pending 模型：请求先入库并返回 pending，发起方轮询状态。
- 需要处理 B 离线、拒绝、超时、重复邀请、同名任务目录冲突。

### 验收标准

- A 创建任务时不需要输入 B 的路径。
- B 能看到邀请并选择自己的目录。
- B 拒绝时 A 显示明确失败原因。
- B 接受后两端重启，任务和 task root 仍然可用。
- 有集成测试覆盖邀请、接受、拒绝、超时和重启后恢复。

## 4. P1：自动同步、任务队列与进度

### 目标

让同步从“手动测试按钮”变成“文件变更后自动执行”，同时避免事件风暴和同步自循环。

### 功能范围

- 每个任务新增 `auto_sync_enabled`。
- 使用 `notify` 监听本机同步目录。
- 对同一任务的文件事件做 debounce，建议 800ms 到 2s。
- 同一任务串行执行，避免并发 scan/sync 互相踩。
- 同步写入产生的 watcher 事件必须能被识别或被后续扫描去重，避免无限循环。
- UI 显示任务状态：
  - 空闲
  - 等待防抖
  - 扫描中
  - 正在请求远端状态
  - 正在传输
  - 已完成
  - 失败，等待重试

### 技术方案

- 新增 sync job queue：
  - key：`task_id`
  - 每个 task 同一时间最多一个 active job。
  - 新事件到来时合并为同一个 pending job。
- watcher 只负责发信号，不直接执行同步。
- 同步执行仍使用现有 `sync_now` 内部逻辑，但拆出可复用服务函数，避免 Tauri command 与后台 worker 重复代码。
- 进度事件通过 Tauri event 发给前端，例如：
  - `sync:status`
  - `sync:file-progress`
  - `sync:error`

### 风险

- 大量文件写入会造成事件风暴。
- 文件仍在写入时读取可能失败或 hash 不稳定。
- 电脑睡眠/唤醒、VPN 切换后 watcher 事件可能丢失。
- 必须保留周期性/手动扫描作为兜底。

### 验收标准

- 修改文件后无需点击 `Sync Now`，在 debounce 后自动同步。
- 连续保存同一文件不会触发多次重复传输。
- 同步写入不会造成自循环。
- UI 能看到扫描、传输、失败与完成状态。
- 断网时任务进入失败/待重试状态，而不是静默消失。

## 5. P1：可视化冲突处理

### 目标

把“有冲突但用户不知道怎么处理”的黑盒状态变成可理解、可操作的决策界面。

### UI 内容

冲突面板至少展示：

- 相对路径。
- 本机版本修改时间、大小、hash 状态。
- 对端版本修改时间、大小、hash 状态。
- 冲突原因。
- 操作：
  - 保留本机。
  - 保留对端。
  - 保留两者。
  - 跳过。

默认推荐：保留两者。不要默认静默覆盖。

### 技术方案

- 扩展现有 conflict model，补充 size、hash status 和设备侧标签。
- `detect_conflicts` 返回可直接渲染的 `ConflictInfo`。
- `resolve_conflict_keep_both` 应生成稳定安全的文件名，例如 `name (conflict 2026-05-13 143000).ext`。
- 覆盖操作必须先进入 history，再写新文件。

### 风险

- 对大文件 hash 不可用时，只能使用 size/mtime，必须在 UI 显示“未完全校验”。
- 用户选择“以本机为准”可能导致对端新改动被覆盖，因此不能作为默认策略。

### 验收标准

- 冲突不会被静默覆盖。
- 用户能看到两边版本信息。
- “保留两者”不会覆盖任何现有文件。
- 覆盖前一定产生 history 备份。

## 6. P1：持久重试队列

### 目标

解决 VPN 切换、WiFi 抖动、睡眠唤醒等情况下同步失败只能靠用户手动再点的问题。

### 技术方案

- 新增 `sync_jobs` 或 `retry_queue` 表。
- 记录：
  - task id
  - 操作类型
  - relative path
  - attempt count
  - next retry time
  - last error
  - created/updated time
- 可重试错误：
  - peer offline
  - network interrupted
  - timeout
  - temporary file lock
  - hash source temporarily unavailable
- 不重试错误：
  - invalid path
  - permission denied
  - untrusted peer
  - case collision

### 风险

- 队列里的旧操作可能与新扫描结果冲突。
- 每次重试前必须重新确认 baseline/remote scan，不能盲目重放旧传输。

### 验收标准

- 网络断开导致失败后，恢复网络可以自动重试。
- 重启应用后未完成的可重试任务仍存在。
- 重试前会重新检查远端状态，避免覆盖离线修改。

## 7. P2：接收端状态收尾

### 目标

目前接收端落盘和 ACK 已闭合，但接收端 UI/DB 仍主要依赖后续扫描感知状态。下一阶段应让接收端在成功接收后更新本地状态和日志。

### 技术方案

- 接收端 `FileChunkEnd` 成功后：
  - 更新对应 task 的 snapshot。
  - 写 event log。
  - 可选更新 baseline，前提是能明确 primary/secondary 角色和远端 hash。
- 接收端 `FileDelete` 成功后：
  - 写 history entry。
  - 更新 snapshot 为 deleted 或等待下一次 scan 清理。

### 风险

- 如果接收端没有完整任务记录，只知道 task root，baseline 更新会缺少角色语义。
- 因此 P0 邀请确认必须让接收端也持久化 `SyncTask`，不能只持久化 task root。

### 验收标准

- 文件接收成功后，接收端 UI 不需要手动扫描也能看到最新状态。
- 接收端日志能显示收到哪些文件、失败哪些文件。
- 删除进入 history 后能在历史页面恢复。

## 8. P2：并发传输

### 目标

提升大量小文件和中等文件的吞吐。先不做复杂 P2P，只在已配对两端之间做有限并发。

### 技术方案

- 每个 task 默认并发度 2。
- 每个文件仍保持单文件 ACK 和原子落盘。
- 任务队列按 relative path 稳定排序，避免 UI 状态跳动。
- UI 显示总进度和当前活跃文件。

### 风险

- 并发过高会拖慢 WiFi、增加磁盘争用。
- 错误恢复更复杂，必须保留 per-file result。

### 验收标准

- 并发传输不会破坏 baseline 和 history。
- 单文件失败不影响其他文件完成。
- 用户能配置或至少看到并发限制。

## 9. P3：Delta Sync

### 目标

减少大文件小改动时的传输量。

### 建议范围

第一阶段只做固定分块，不做滚动哈希：

- 默认块大小 1 MB 或 4 MB。
- 双方交换块 hash manifest。
- 只发送缺失或 hash 不同的块。
- 在临时文件中重组，最终 blake3 全文件校验通过后 rename。

### 风险

- 对插入字节导致整体偏移的文件，固定分块收益有限。
- 块索引会增加 DB 和协议复杂度。
- 对大量小文件没有明显收益。

### 验收标准

- 大文件局部覆盖写只传变化块。
- 中途失败不会损坏原文件。
- 全文件 hash 不一致时拒绝替换并保留临时文件用于诊断或清理。

## 10. 不做项

下一阶段明确不做：

- uTP。
- NAT 穿透。
- 多设备网状 P2P。
- 自动合并 Office、数据库、项目文件等结构化内容。
- 默认自动覆盖冲突。
- 无权限的免配对同步任务创建。

## 11. 建议实施顺序

- [ ] Task A：任务邀请 inbox、B 端确认 UI、B 端选择目录、两端任务持久化。（最小闭环、pending 持久化、拒绝轮询和基础空目录校验已实现，剩余超时/重复邀请、平台级危险路径和权限校验）
- [ ] Task B：同步状态模型和 Tauri event，前端展示等待、扫描、传输、失败、完成。
- [ ] Task C：watcher + debounce + per-task 串行队列。
- [ ] Task D：冲突面板，支持保留本机、保留对端、保留两者、跳过。
- [ ] Task E：持久重试队列和恢复后重放。
- [ ] Task F：接收端 DB/log/history 状态收尾。
- [ ] Task G：有限并发传输。
- [ ] Task H：固定分块 Delta Sync 原型。

## 12. 每阶段通用验收

每个 Task 完成前必须满足：

- 新增或修改行为有回归测试。
- `cargo test --manifest-path src-tauri/Cargo.toml` 通过。
- `npm run build` 通过。
- 不引入静默覆盖。
- 不引入不可恢复删除。
- UI 必须显示失败原因，不能只显示空列表或无响应。
