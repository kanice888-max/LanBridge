<div align="center">

# LanBridge

**在受信任局域网内，让 Mac 与 Windows 文件夹保持同步。**

不经云端，不需要第三方存储；重要变更始终由你确认。

[中文（默认）](README.md) · [English](README.en.md)

</div>

> [!IMPORTANT]
> LanBridge 采用 **主端 / 从端 + 显式回传** 模型。主端变更会自动同步到从端；从端变更会先进入待回传列表，只有你确认后才会写回主端。因此它不是完全自动的双向同步工具。

LanBridge 是一款面向 macOS 和 Windows 的开源桌面应用，适合希望在本地网络中同步文件、又不想把文件交给云端的人。它让同步方向保持清晰，并尽量避免误操作或冲突悄悄覆盖文件。

## 核心特点

- **文件留在本地网络**：设备可在局域网内发现，也支持手动输入 IP；不依赖云盘或第三方中转。
- **同步方向清晰**：每个任务都有明确主端。主端的新建、修改和删除会自动同步到从端。
- **从端改动由你决定**：从端的新增、修改和删除会先显示为待回传项，确认后才影响主端。
- **冲突不会静默覆盖**：同一路径在两端都被修改时，LanBridge 会要求你选择处理方式；覆盖前会保留主端旧文件。
- **删除可以恢复**：同步删除和覆盖前的文件会进入任务历史记录，可按需恢复。
- **为桌面文件夹而生**：提供文件树、同步状态、传输进度、待回传项、冲突处理、历史恢复和系统托盘入口。

## 工作方式

```text
主端文件夹 ── 自动同步 ──▶ 从端文件夹
    ▲                            │
    └──── 用户确认“回传” ◀───────┘
```

1. 将两台设备放在同一可信局域网内，通过设备发现或手动输入 IP 连接。
2. 创建任务时指定主端；接收端接受任务后选择自己的本地文件夹。
3. 主端变更自动传到从端；从端变更先进入待回传列表。
4. 需要时确认回传从端变更；发生冲突时由你选择保留方式。

## 适合什么场景

- 在 Mac 与 Windows PC 之间同步工作文件夹、照片素材或项目文档。
- 希望避开云端存储、账号体系和持续上传的局域网工作流。
- 希望保留自动同步的便利，同时不让从端修改直接影响主端。

## 使用前了解

- LanBridge 不支持广域网或 NAT 穿透同步。
- 它不是 Dropbox、iCloud Drive 或 Syncthing 那类完全自动的双向同步替代品。
- 不建议同步数据库、虚拟机镜像、浏览器配置、邮件存储、依赖缓存，或其他应用持续写入的文件。
- 不同步符号链接；同步删除也不会直接永久删除你的文件。

## 下载与安装

从 [GitHub Releases](https://github.com/kanice888-max/LanBridge/releases/latest) 下载与你设备匹配的安装包：

| 系统 | 下载内容 | 说明 |
| --- | --- | --- |
| macOS Intel | `LanBridge_0.2.0_x64.dmg` | 适用于 Intel Mac。 |
| macOS Apple Silicon | `LanBridge_0.2.0_aarch64.dmg` | 适用于 M 系列芯片 Mac。 |
| Windows x64 | `.exe` 或 `.msi` | `.exe` 适合个人安装，`.msi` 适合受管理部署。 |

macOS 安装包尚未经过 Apple 公证。首次打开若被拦截，请在 LanBridge.app 上右键选择“打开”，或在“系统设置 → 隐私与安全性”中点击“仍要打开”；无需关闭 Mac 的安全保护。详细步骤见 [macOS 安装说明](docs/release/macos-installation.md) 与 [Windows 安装说明](docs/release/windows-installation.md)。

## 快速开始

1. 在两台设备上打开 LanBridge，并确认它们位于同一可信网络。
2. 通过设备发现或手动输入 IP 连接，并确认正在连接自己的设备。
3. 创建同步任务，选择主端文件夹；接收端接受任务并选择目标文件夹。
4. 在主端创建或修改一个测试文件，确认它出现在从端。
5. 在从端修改文件；在“待回传”中查看变更，再按需要确认回传。

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

完整的平台验证与发布要求见 [验证检查](docs/validation/checks.md)。

## 安全边界

请只在可信的家庭或办公网络中使用 LanBridge，并确认正在连接的是自己的设备。应用会在本机保存设备身份；接收端需要接受任务后才能使用对应文件夹；冲突和回传均需要用户明确确认。

- [同步与数据安全不变量](docs/rules/invariants.md)
- [安全策略](SECURITY.md)
- [安全加固计划](docs/security/security-hardening-plan.md)

发现安全问题时，请不要公开披露，按 [SECURITY.md](SECURITY.md) 的方式私下报告。

## 项目结构

```text
src/              React 前端
src-tauri/        Rust / Tauri 后端与集成测试
docs/             产品、架构、安全与工作流文档
scripts/          项目工具脚本
```

## 开源协议

本项目采用 [MIT License](LICENSE)。
