# LanBridge Transfer Performance E2E Test Document

> **Purpose:** 每次修改协议或传输代码后，在 Windows/macOS 实机上运行这些用例，记录结果作为 before/after 对比。

## Test Environment

| Field | Value |
| --- | --- |
| Windows device name | |
| macOS device name | |
| Network type | wired / Wi-Fi 5 / Wi-Fi 6 |
| Router model | |
| Any VPN active? | |
| Any firewall/antivirus notes | |
| LanBridge version/commit | |
| Date | |

---

## Test Cases

### Test 1: Windows primary → macOS secondary, 1GB single file

| Field | Value |
| --- | --- |
| OS pair | Windows → macOS |
| Network type | |
| File count | 1 |
| Total bytes | |
| Hash time (ms) | |
| Transfer time (ms) | |
| Final ACK time (ms) | |
| Observed MB/s | |
| Protocol version | v1 / v2 |
| Chunk size | 1MB |
| ACK interval | 16MB |
| Any firewall/VPN/antivirus notes | |

### Test 2: macOS primary → Windows secondary, 1GB single file

| Field | Value |
| --- | --- |
| OS pair | macOS → Windows |
| Network type | |
| File count | 1 |
| Total bytes | |
| Hash time (ms) | |
| Transfer time (ms) | |
| Final ACK time (ms) | |
| Observed MB/s | |
| Protocol version | v1 / v2 |
| Chunk size | 1MB |
| ACK interval | 16MB |
| Any firewall/VPN/antivirus notes | |

### Test 3: Windows primary → macOS secondary, 2000 small files (total ~1GB)

| Field | Value |
| --- | --- |
| OS pair | Windows → macOS |
| Network type | |
| File count | 2000 |
| Total bytes | |
| Hash time (ms) | |
| Transfer time (ms) | |
| Final ACK time (ms) | |
| Observed MB/s | |
| Protocol version | v1 / v2 |
| Any firewall/VPN/antivirus notes | |

### Test 4: macOS primary → Windows secondary, 2000 small files (total ~1GB)

| Field | Value |
| --- | --- |
| OS pair | macOS → Windows |
| Network type | |
| File count | 2000 |
| Total bytes | |
| Hash time (ms) | |
| Transfer time (ms) | |
| Final ACK time (ms) | |
| Observed MB/s | |
| Protocol version | v1 / v2 |
| Any firewall/VPN/antivirus notes | |

### Test 5: Secondary → primary return sync, 1GB single file

| Field | Value |
| --- | --- |
| OS pair | |
| Network type | |
| File count | 1 |
| Total bytes | |
| Hash time (ms) | |
| Transfer time (ms) | |
| Final ACK time (ms) | |
| Observed MB/s | |
| Protocol version | v1 / v2 |
| Any firewall/VPN/antivirus notes | |

### Test 6: Large file transfer + concurrent small file added to same task

| Field | Value |
| --- | --- |
| OS pair | |
| Network type | |
| Large file size | |
| Small file count | |
| Small file total bytes | |
| Large file transfer time (ms) | |
| Small file wait time before transfer starts (ms) | |
| Large file Observed MB/s | |
| Protocol version | |
| Notes | |

---

## Acceptance Criteria

Each real-device run is considered recorded when every applicable test case includes:

- OS pair
- Network type
- File count
- Total bytes
- Hash time
- Transfer time
- Final ACK time
- Observed MB/s
- Any firewall/VPN/antivirus notes

---

## How To Generate A Test File

### 1GB single file (Windows PowerShell):

```powershell
$path = "$env:USERPROFILE\Desktop\test_1gb.bin"
$size = 1GB
$stream = [System.IO.File]::OpenWrite($path)
$stream.SetLength($size)
$stream.Close()
```

### 2000 small files (Windows PowerShell):

```powershell
$dir = "$env:USERPROFILE\Desktop\test_2000_small"
New-Item -ItemType Directory -Force $dir
1..2000 | ForEach-Object {
    $content = "file $_ content " + ("x" * 500)
    $content | Out-File -FilePath "$dir\file_$_.txt" -Encoding UTF8
}
```

### 1GB single file (macOS terminal):

```bash
dd if=/dev/urandom of=~/Desktop/test_1gb.bin bs=1m count=1024
```

### 2000 small files (macOS terminal):

```bash
mkdir -p ~/Desktop/test_2000_small
for i in $(seq 1 2000); do
  echo "file $i content $(head -c 500 /dev/urandom | base64)" > ~/Desktop/test_2000_small/file_$i.txt
done
```

---

## Before/After Comparison Template

| Metric | Before (V1 64KB/ACK-per-chunk) | After V1 optimize (1MB/16MB ACK/stream-hash) | After V2 (binary protocol) |
| --- | --- | --- | --- |
| 1GB upload MB/s | | | |
| 1GB download MB/s | | | |
| 2000 files time | | | |
| Hash pre-delay (s) | | | |
| ACK count per 1GB | | | |
