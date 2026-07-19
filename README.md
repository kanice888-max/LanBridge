<div align="center">

# LanBridge

**让 Mac 和 Windows 在局域网内安全地同步文件夹。**

不经云端，不需要第三方存储；同步过程始终由你确认和掌控。

[中文（默认）](README.md) · [English](README.en.md)

</div>

> [!IMPORTANT]
> LanBridge 采用 **Primary / Secondary（主端 / 从端）+ 显式回传** 模型，不是“完全自动双向同步”。主端变更会自动同步到从端；从端变更必须由用户明确发起回传，才会影响主端。

LanBridge 是一款面向 macOS 和 Windows 的开源桌面应用，用于在同一受信任局域网内同步指定文件夹。它适合同时使用 Mac 与 Windows、重视隐私和文件可控性的人：文件不需要上传到云端，也不会因为从端的一次修改而悄悄覆盖主端内容。

## 为什么选择 LanBridge

- **文件留在本地网络**：设备直接在局域网内发现和连接，支持手动 IP 连接；不依赖云盘或第三方中转服务。
- **同步方向清晰**：每个任务都有明确主端。主端的新建、修改和删除会自动同步到从端，避免“哪一台才是最终版本”的不确定性。
- **从端改动由你决定**：从端的新增、修改和删除会显示为待回传项；只有你确认回传后，才会对主端生效。
- **冲突不静默覆盖**：如果主端与从端都修改了同一路径，LanBridge 会要求选择处理方式；确认覆盖前会先备份主端旧文件。
- **删除可恢复**：同步删除和覆盖前的文件会先进入任务历史记录，而不是立即永久删除；可从历史记录恢复。
- **配对而非盲连**：设备配对需要双方确认验证码；任务邀请也必须由接收端接受后才能注册本地文件夹。
- **为桌面文件夹而生**：提供文件树、同步状态、传输进度、待回传项、冲突处理、历史恢复和系统托盘入口。

## 工作方式

```text
主端文件夹 ── 自动同步 ──▶ 从端文件夹
    ▲                            │
    └──── 用户确认“回传” ◀───────┘
```

1. 在同一受信任局域网内配对两台设备，可使用设备发现或手动 IP。
2. 创建同步任务时选择哪一端为主端，并由接收端选择自己的本地文件夹。
3. 主端的变更自动发送到从端；主端删除会先将从端对应内容移入 LanBridge 历史记录。
4. 从端变更进入“待回传”列表。回传前会检查主端自上次同步后是否也发生变更。
5. 有冲突时由用户选择保留方式；恢复或覆盖均不会静默丢弃已有文件。

## 适用场景

- 在 Mac 与 Windows PC 之间同步工作文件夹、照片素材或项目文档。
- 希望避免云端存储、账号体系和持续上传的局域网工作流。
- 希望保留自动同步的便利，同时要求从端修改必须经过明确确认。

## 不是它要解决的问题

- 不是云盘，也不支持广域网 / NAT 穿透同步。
- 不是完全自动的双向同步；请不要把它当作 Dropbox、iCloud Drive 或 Syncthing 的双向替代品。
- 不保证数据库、虚拟机镜像、浏览器配置、邮件存储、依赖缓存或被其他应用持续写入的文件可安全实时同步。
- 不同步符号链接，也不会把删除操作直接永久应用到用户文件。

## 开始使用

LanBridge 当前为 pre-1.0 项目。使用前请确保两台设备处于同一受信任局域网，并分别在 macOS 与 Windows 上运行应用。

## 下载与安装

从 [GitHub Releases](https://github.com/kanice888-max/LanBridge/releases/latest) 下载与自己设备匹配的安装包和 `SHA256SUMS.txt`：

| 系统 | 下载内容 | 说明 |
| --- | --- | --- |
| macOS Intel | `LanBridge_0.1.4_x64.dmg` | 适用于 Intel Mac。 |
| macOS Apple Silicon | `LanBridge_0.1.4_aarch64.dmg` | 适用于 M 系列芯片 Mac。 |
| Windows x64 | `.exe` 或 `.msi` | `.exe` 适合个人安装，`.msi` 适合受管理部署。 |

请在下载后核对 SHA-256。macOS 安装包使用 ad-hoc 签名，尚未经过 Apple 公证：首次启动可能需要
右键点按 **打开**，或在“系统设置 → 隐私与安全性”中选择“仍要打开”；请不要关闭 Gatekeeper。详细步骤见
[macOS 安装说明](docs/release/macos-installation.md)与[Windows 安装说明](docs/release/windows-installation.md)。

1. 在两台设备上打开 LanBridge。
2. 通过局域网发现或手动输入 IP 发起配对，并在两端核对验证码。
3. 创建任务，选择主端文件夹；接收端接受邀请并选择自己的目标文件夹。
4. 在主端创建或修改一个测试文件，确认它出现在从端。
5. 在从端修改该文件，在“待回传”中检查变更后再明确执行回传。

## 从源码运行

### 前置条件

- Node.js 18 或更高版本
- Rust stable 工具链
- 对应平台的 Tauri 1.x 开发依赖
  - macOS：Xcode Command Line Tools
  - Windows：Microsoft C++ Build Tools 和 WebView2 Runtime

```bash
npm install
npm run tauri dev
```

常用检查与构建命令：

```bash
npm run lint:names
npm run build
npm test
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

完整的平台验证与发布要求见[验证检查](docs/validation/checks.md)。

## 安全与数据保护

LanBridge 面向**受信任的局域网**。设备发现不代表信任；请只在确认对方身份后完成配对。应用会保存本地设备身份，任务邀请需经接收端确认，且所有冲突都需要显式决定。

- [同步与数据安全不变量](docs/rules/invariants.md)
- [安全策略](SECURITY.md)
- [安全加固计划](docs/security/security-hardening-plan.md)

发现安全问题时，请不要公开披露，按 [SECURITY.md](SECURITY.md) 的方式私下报告。

## 项目结构与贡献

```text
src/              React 前端
src-tauri/        Rust / Tauri 后端与集成测试
docs/             产品、架构、安全与工作流文档
scripts/          项目工具脚本
```

提交涉及同步、传输、配对、删除、冲突或历史记录的变更前，请先阅读[架构概览](docs/architecture/index.md)、[数据安全不变量](docs/rules/invariants.md)和[任务工作流](docs/workflows/task-flow.md)，并在对应 worktree 中完成验证。

欢迎阅读[贡献指南](CONTRIBUTING.md)、[更新日志](CHANGELOG.md)和[行为准则](CODE_OF_CONDUCT.md)，再提交 issue 或 PR。

## 开源协议

本项目采用 [MIT License](LICENSE)。
