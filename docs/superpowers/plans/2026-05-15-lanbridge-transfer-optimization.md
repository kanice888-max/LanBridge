# LanBridge Transfer Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将当前 4-5 MB/s 的文件传输性能逐步优化到可验证、可回退、可跨 Windows/macOS 实机稳定运行的水平。

**Architecture:** 先保留现有 JSON length-prefixed 协议并优化 chunk、ACK、文件句柄和哈希路径，降低实现风险；再引入 V2 裸二进制协议作为显式协商能力，旧客户端继续走 V1。每个阶段都要求 Windows 与 macOS worktree 同步修改，并通过双机实测验证。

**Tech Stack:** Tauri、Rust/Tokio TCP、Serde JSON 控制消息、Blake3、React/Vite、Windows/macOS 实机测试。

---

## 2026-05-18 Implementation Status

- P1/P2 backend protocol changes have been implemented in both Windows and macOS worktrees.
- V1 upload now uses 1MB chunks, persistent sender buffers, streaming hash, and checkpoint ACKs after successful protocol negotiation or cached capability detection.
- V1 fallback remains conservative for older peers: it pre-hashes before transfer, includes the hash in `FileChunkStart`, and keeps per-chunk `FileAck` behavior.
- V1 download/serve still preserves the legacy start-hash contract for compatibility; V2 is the optimized download path for upgraded peers.
- V2 upload/download use explicit negotiation, JSON control frames, raw binary payload chunks, checkpoint ACKs, streaming hash finalization, and V1 fallback on negotiation failure.
- Added regression coverage for negotiated V2 upload, V2 download, V2 fallback to legacy V1, V1 checkpoint ACK behavior, and V2 control frame round trips.

---

## 历史瓶颈判断与当前剩余问题

以下是优化前的瓶颈判断，保留用于解释 P1/P2 的设计背景。P1/P2 已经处理了 chunk、ACK、发送端流式 hash、V2 binary payload 和兼容 fallback；当前剩余重点是实机测速、小文件公平调度、断点续传和可选限速/压缩。

- 历史 V1 每个 64KB chunk 都封装进 JSON `Vec<u8>`，二进制数据被 JSON 数组序列化，CPU 和体积开销都很大。
- 历史 V1 每个 chunk 都等待一次 `FileAck`，导致 1GB 文件约需要 16384 次往返确认。
- 历史接收端每个 chunk 重新打开 partial 文件追加写入，增加系统调用和文件系统开销。
- 历史发送前完整 hash 一遍文件，发送时再读一遍文件，用户感知上会出现“传输前等待”。
- 当前同任务仍偏串行，处理大文件时小文件公平插队属于 P3。

正常局域网 TCP 传输在千兆有线下通常应接近 60-110 MB/s，Wi-Fi 5/6 稳定环境常见 20-80 MB/s。LanBridge 优化后的阶段性目标是：

- V1 优化后：有线 20-60 MB/s，Wi-Fi 10-40 MB/s。
- V2 裸二进制协议后：有线 60-100 MB/s，Wi-Fi 20-80 MB/s。

这些数值是产品验收目标，不是硬保证；实际速度仍受磁盘、Wi-Fi 信号、VPN、杀毒软件扫描和系统防火墙影响。

---

## File Structure

**Backend protocol and transfer path**

- Modify: `worktrees/windows/src-tauri/src/transport/protocol.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/protocol.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/server.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/server.rs`

**Sync scheduling and fairness**

- Modify: `worktrees/windows/src-tauri/src/commands.rs`
- Modify: `worktrees/macos/src-tauri/src/commands.rs`
- Modify: `worktrees/windows/src-tauri/src/app_state.rs`
- Modify: `worktrees/macos/src-tauri/src/app_state.rs`

**Tests and real-device validation**

- Modify: `worktrees/windows/src-tauri/tests/pairing_transfer.rs`
- Modify: `worktrees/macos/src-tauri/tests/pairing_transfer.rs`
- Modify: `worktrees/windows/src-tauri/tests/e2e_full_flow.rs`
- Modify: `worktrees/macos/src-tauri/tests/e2e_full_flow.rs`
- Create: `docs/testing/transfer-performance-e2e.md`

---

## Priority Overview

| Priority | Stage | Expected Gain | Risk | Ship Separately |
| --- | --- | --- | --- | --- |
| P0 | 建立测速基线和可复现实机脚本 | 不直接提速 | 低 | 是 |
| P1 | V1 协议低风险优化：大 chunk、少 ACK、持久文件句柄、流式 hash | 中高 | 中 | 是 |
| P2 | V2 裸二进制协议：JSON 控制帧 + raw payload | 高 | 高 | 是 |
| P3 | 同任务传输队列和小文件公平调度 | 中 | 中 | 是 |
| P4 | 断点续传和块级校验 | 中 | 高 | 是 |
| P5 | 可选压缩/限速/性能 UI | 场景化 | 中 | 是 |

---

## P0: Transfer Baseline And Instrumentation

**Purpose:** 先确定慢在哪里，避免盲目改协议后无法证明收益。

**Status:** Implemented. Timing/progress logs and the manual E2E measurement document exist; real-device result rows still need to be filled after packaged-device testing.

**Files:**

- Modify: `worktrees/windows/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/server.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/server.rs`
- Create: `docs/testing/transfer-performance-e2e.md`

- [x] **Step 1: Add transfer timing logs**

Add structured logs around:

- file hash start/end
- TCP authenticated stream open time
- first byte sent/received
- every 64MB transferred
- final ACK time

Log fields:

```text
task_id
relative_path
direction
bytes_total
bytes_done
elapsed_ms
mbps
protocol_version
chunk_size
ack_interval_bytes
```

- [x] **Step 2: Add a manual performance test document**

Create `docs/testing/transfer-performance-e2e.md` with these exact test cases:

```text
1. Windows primary -> macOS secondary, 1GB single file, wired/wireless noted.
2. macOS primary -> Windows secondary, 1GB single file, wired/wireless noted.
3. Windows primary -> macOS secondary, 2000 small files, total 1GB.
4. macOS primary -> Windows secondary, 2000 small files, total 1GB.
5. Secondary -> primary return sync, 1GB single file.
6. Repeat one large transfer while another small file is added to the same task.
```

- [x] **Step 3: Define acceptance criteria**

P0 passes when the document records:

```text
OS pair:
Network type:
File count:
Total bytes:
Hash time:
Transfer time:
Final ACK time:
Observed MB/s:
Any firewall/VPN/antivirus notes:
```

- [x] **Step 4: Verify no behavior change**

Run:

```powershell
cargo test --manifest-path worktrees/windows/src-tauri/Cargo.toml
cargo test --manifest-path worktrees/macos/src-tauri/Cargo.toml
```

Expected: all tests pass; only additional logs are introduced.

---

## P1: V1 Safe Throughput Optimization

**Purpose:** 不改协议版本，先把现有协议能安全提升的部分做完。这一阶段适合先发给实机测试。

**Status:** Implemented with compatibility constraints. Newer V1 uploads use 1MB chunks, streaming hash, persistent buffers, and checkpoint ACKs. Legacy fallback and compatibility download paths preserve the old start-hash/per-chunk ACK behavior where needed.

### P1.1 Increase Chunk Size

**Files:**

- Modify: `worktrees/windows/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/server.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/server.rs`

- [x] **Step 1: Extract shared transfer constants**

Create constants near the transfer code:

```rust
const TRANSFER_V1_CHUNK_SIZE: usize = 1024 * 1024;
const TRANSFER_V1_ACK_INTERVAL_BYTES: u64 = 16 * 1024 * 1024;
```

- [x] **Step 2: Change upload and download chunk size from 64KB to 1MB**

Replace local `const CHUNK_SIZE: usize = 64 * 1024;` in upload/download paths with `TRANSFER_V1_CHUNK_SIZE`.

- [x] **Step 3: Add regression test for files above message limit**

Keep or extend the existing large-file test:

```text
test_authenticated_chunked_transfer_supports_files_over_message_limit
```

Expected: a file larger than 10MB still transfers through chunking.

### P1.2 Reduce ACK Frequency

**Files:**

- Modify: `worktrees/windows/src-tauri/src/transport/protocol.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/protocol.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/server.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/server.rs`

- [x] **Step 1: Add checkpoint ACK fields without breaking existing messages**

Extend `FileAck` only if needed with optional fields using serde defaults, or add a new V1-compatible `FileChunkAck` message. Do not remove existing `FileAck`.

Recommended message:

```rust
FileChunkAck {
    task_id: String,
    relative_path: String,
    received_bytes: u64,
    success: bool,
    error: Option<String>,
}
```

- [x] **Step 2: ACK every 16MB and at final end**

Sender behavior:

```text
send chunks continuously
wait for FileChunkAck only when bytes_sent >= next_ack_at
always wait for FileAck after FileChunkEnd
```

Receiver behavior:

```text
append chunk
return FileChunkAck only when sender requested checkpoint or error occurs
return FileAck after FileChunkEnd
```

- [x] **Step 3: Test ACK reduction**

Add a test that transfers a 40MB file and asserts checkpoint ACK count is small through an injectable test hook or debug counter.

Expected:

```text
40MB file -> about 2 checkpoint ACKs + 1 final ACK
```

### P1.3 Keep Receiver File Handle Open

**Files:**

- Modify: `worktrees/windows/src-tauri/src/transport/server.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/server.rs`

- [x] **Step 1: Store `std::fs::File` in `IncomingTransfer`**

Expected struct shape:

```rust
struct IncomingTransfer {
    partial_path: PathBuf,
    final_path: PathBuf,
    file_hash: String,
    total_bytes: u64,
    written_bytes: u64,
    hasher: blake3::Hasher,
    file: std::fs::File,
}
```

- [x] **Step 2: Open the partial file once at transfer start**

Use `std::fs::File::create(&partial_path)?` in `start_incoming_chunked_file`.

- [x] **Step 3: Append through the stored handle**

`append_incoming_chunk` should call `transfer.file.write_all(data)?` instead of reopening the path for every chunk.

- [x] **Step 4: Flush and close before rename**

`finish_incoming_chunked_file` must flush and drop the file before `std::fs::rename`.

Expected: Windows rename does not fail due to an open file handle.

### P1.4 Stream Hash On Send

**Files:**

- Modify: `worktrees/windows/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/server.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/server.rs`

- [x] **Step 1: Stop pre-reading the entire file only to calculate hash**

Sender records `source_file_state` before transfer, updates a `blake3::Hasher` while reading chunks, and sends the final hash at `FileChunkEnd`.

- [x] **Step 2: Extend `FileChunkEnd` with hash while preserving compatibility**

Recommended compatibility shape:

```rust
FileChunkEnd {
    task_id: String,
    relative_path: String,
    #[serde(default)]
    file_hash: Option<String>,
}
```

Receiver uses the hash from `FileChunkStart` if present, otherwise the hash from `FileChunkEnd`.

- [x] **Step 3: Check file stability before and after streaming**

Keep:

```rust
source_file_state(file_path)
ensure_source_file_unchanged(file_path, &before)
```

Expected: generating files are rejected instead of syncing partial content.

---

## P2: V2 Bare Binary Protocol

**Purpose:** 去掉 JSON `Vec<u8>` 承载二进制数据的根本开销。V2 必须协商启用，旧版本继续使用 V1。

**Status:** Implemented. V2 upload/download use negotiated JSON control frames plus raw binary payloads, streaming hash finalization, checkpoint ACKs, and V1 fallback on negotiation failure.

### P2.1 Version Negotiation

**Files:**

- Modify: `worktrees/windows/src-tauri/src/transport/protocol.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/protocol.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/server.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/server.rs`

- [x] **Step 1: Add explicit transfer capability messages**

Add:

```rust
TransferHello {
    supported_versions: Vec<u16>,
    preferred_version: u16,
}

TransferReady {
    selected_version: u16,
    max_chunk_size: u32,
    ack_interval_bytes: u64,
}
```

- [x] **Step 2: Fallback to V1 on negotiation failure**

Sender flow:

```text
open authenticated stream
send TransferHello
if TransferReady selected_version == 2 -> use V2
if peer replies unexpected message, closes, or times out -> reopen stream and use V1
```

Timeout target: 2-5 seconds.

### P2.2 V2 Frame Format

**Files:**

- Modify: `worktrees/windows/src-tauri/src/transport/protocol.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/protocol.rs`

- [x] **Step 1: Keep JSON for control frames only**

V2 control messages remain length-prefixed JSON:

```rust
FileStreamStartV2 {
    task_id: String,
    relative_path: String,
    total_bytes: u64,
}

FileChunkBinaryV2 {
    task_id: String,
    relative_path: String,
    offset: u64,
    bytes: u32,
    ack: bool,
}

FileStreamEndV2 {
    task_id: String,
    relative_path: String,
    file_hash: String,
}
```

- [x] **Step 2: Send raw bytes immediately after `FileChunkBinaryV2`**

Wire format:

```text
4-byte JSON length
JSON FileChunkBinaryV2 header
raw payload bytes, exactly header.bytes
```

Receiver must call `read_exact(header.bytes)` after decoding `FileChunkBinaryV2`.

- [x] **Step 3: ACK by byte checkpoint**

Add:

```rust
FileStreamAckV2 {
    task_id: String,
    relative_path: String,
    received_bytes: u64,
    success: bool,
    error: Option<String>,
}
```

ACK every 16MB and final completion.

### P2.3 V2 Upload

**Files:**

- Modify: `worktrees/windows/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/server.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/server.rs`

- [x] **Step 1: Implement V2 sender**

Sender reads file with a 1MB or 4MB buffer:

```text
read file chunk
update blake3 hasher
write FileChunkBinaryV2 header
write raw bytes
wait only when ack checkpoint is requested
```

- [x] **Step 2: Implement V2 receiver**

Receiver:

```text
decode FileChunkBinaryV2
read raw bytes
validate offset
write raw bytes into open partial file handle
update blake3 hasher
send FileStreamAckV2 only on checkpoint or error
```

- [x] **Step 3: Finalize with hash and atomic rename**

Sender sends `FileStreamEndV2` with streaming hash.

Receiver:

```text
flush file
compare written_bytes with total_bytes
compare blake3 hash
rename partial file to final path
update DB snapshot/baseline/log
return final FileAck or FileStreamAckV2
```

### P2.4 V2 Download / Secondary Pull

**Files:**

- Modify: `worktrees/windows/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/server.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/server.rs`

- [x] **Step 1: Add V2 download request**

Recommended message:

```rust
FileDownloadRequestV2 {
    task_id: String,
    relative_path: String,
}
```

- [x] **Step 2: Server streams source file as raw V2 chunks**

The server must reuse the same binary chunk writer used by upload.

- [x] **Step 3: Client receives into `.lanbridge-partial` and renames after hash validation**

Expected behavior matches upload receiver.

### P2.5 V2 Tests

**Files:**

- Modify: `worktrees/windows/src-tauri/tests/pairing_transfer.rs`
- Modify: `worktrees/macos/src-tauri/tests/pairing_transfer.rs`
- Modify: `worktrees/windows/src-tauri/tests/e2e_full_flow.rs`
- Modify: `worktrees/macos/src-tauri/tests/e2e_full_flow.rs`

- [x] **Step 1: Unit test protocol encode/decode for V2 control frames**

Test `FileChunkBinaryV2` header encode/decode without raw bytes.

- [x] **Step 2: Integration test authenticated V2 upload**

Expected:

```text
paired peer
registered task root
send 128MB file through V2
receiver file exists
receiver hash matches source
receiver DB snapshot is updated
```

- [x] **Step 3: Integration test V2 fallback**

Expected:

```text
new sender attempts V2
old receiver does not support V2
sender falls back to V1
file still transfers
```

- [x] **Step 4: Integration test V2 download**

Expected:

```text
secondary requests primary file
primary streams through V2
secondary writes validated file
```

---

## P3: Queue Fairness For Large Tasks

**Purpose:** 大文件传输时，同任务下新增小文件不应长期等待。

**Files:**

- Modify: `worktrees/windows/src-tauri/src/app_state.rs`
- Modify: `worktrees/macos/src-tauri/src/app_state.rs`
- Modify: `worktrees/windows/src-tauri/src/commands.rs`
- Modify: `worktrees/macos/src-tauri/src/commands.rs`

- [ ] **Step 1: Introduce per-task transfer queue**

Queue item fields:

```text
task_id
relative_path
kind
direction
size
priority
created_unix_ms
retry_count
```

- [ ] **Step 2: Schedule small files before large file continuation when safe**

Rule:

```text
files <= 8MB get high priority
large files run one at a time per peer
deletes and directory creates execute before file payloads
```

- [ ] **Step 3: Preserve ordering for same relative path**

Never interleave two operations for the same `relative_path`.

---

## P4: Resume And Block-Level Recovery

**Purpose:** VPN/Wi-Fi 切换或临时断网后，不需要从 0 重传超大文件。

**Files:**

- Modify: `worktrees/windows/src-tauri/src/transport/protocol.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/protocol.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/connection.rs`
- Modify: `worktrees/windows/src-tauri/src/transport/server.rs`
- Modify: `worktrees/macos/src-tauri/src/transport/server.rs`

- [ ] **Step 1: Persist partial transfer metadata**

Metadata path:

```text
.lanbridge-temp/transfers/<task_id>/<relative_path_hash>.json
```

Fields:

```text
relative_path
total_bytes
written_bytes
file_state_before
chunk_size
block_hashes_completed
created_unix_ms
updated_unix_ms
```

- [ ] **Step 2: Add resume request**

```rust
FileResumeRequest {
    task_id: String,
    relative_path: String,
    known_bytes: u64,
}
```

- [ ] **Step 3: Validate partial before resume**

Receiver must hash the last completed block before accepting resume.

---

## P5: Optional Compression, Limits, And UI

**Purpose:** 提供用户可理解的性能控制，而不是让后台“黑盒变慢”。

**Files:**

- Modify: `worktrees/windows/src/features/dashboard/Dashboard.tsx`
- Modify: `worktrees/macos/src/features/dashboard/Dashboard.tsx`
- Modify: `worktrees/windows/src-tauri/src/commands.rs`
- Modify: `worktrees/macos/src-tauri/src/commands.rs`

- [ ] **Step 1: Add transfer progress state**

UI should show:

```text
current file
bytes transferred
speed MB/s
remaining estimate
protocol V1/V2
waiting/retrying/error state
```

- [ ] **Step 2: Add optional speed limit**

Use a simple token bucket per peer.

- [ ] **Step 3: Add optional compression only for compressible files**

Do not compress:

```text
zip, 7z, rar, jpg, png, mp4, mov, mp3, pdf, already compressed archives
```

Compression is disabled by default until benchmark proves benefit.

---

## Cross-Platform Requirements

- Windows and macOS protocol enums must stay byte-compatible.
- Both worktrees must be updated in the same PR/commit set.
- Windows rename requires all file handles closed before final rename.
- macOS may expose case-sensitive or case-insensitive volumes; tests must include same-name case variants.
- `.lanbridge-partial`, `.lanbridge-temp`, and `.lanbridge-history` must never be scanned as user content.
- V2 must never be enabled without fallback to V1 until both packaged apps are upgraded.

---

## Real-Device Acceptance Checklist

- [ ] Windows primary -> macOS secondary, 1GB single file reaches at least 20 MB/s on a healthy wired LAN.
- [ ] macOS primary -> Windows secondary, 1GB single file reaches at least 20 MB/s on a healthy wired LAN.
- [ ] Windows secondary -> macOS primary return sync works after V2 is enabled.
- [ ] macOS secondary -> Windows primary return sync works after V2 is enabled.
- [ ] 2000 small files do not stall behind one large file forever after P3.
- [ ] Disconnect network mid-transfer; app returns a clear retryable state.
- [ ] Restart receiver after failed transfer; partial files are ignored by scanner.
- [ ] Old V1 build can still receive from new build through fallback.
- [ ] New build can still receive from old V1 build through fallback.

---

## Recommended Execution Order

1. P0-P2 are complete in code and covered by automated tests.
2. Fill real-device rows in `docs/testing/transfer-performance-e2e.md` before judging actual throughput.
3. P3 should be next if users still see small files waiting behind large transfers.
4. P4/P5 should wait until V2 real-device behavior is stable.

The old “do not start P2” gate is superseded because P2 has already shipped in the current code path with V1 fallback.
