# LanBridge GitHub 开源发布文件清单

本文用于整理 LanBridge 开源发布到 GitHub 时应该上传、可以上传、不要上传的文件。目标是公开可构建、可审查的源码，同时避免把本地临时文件、打包产物、日志、数据库、身份密钥和大体积无关素材传上去。

## 推荐发布方式

优先发布一个干净的应用源码根目录，而不是直接把当前协调仓库的全部内容原样公开。

推荐来源：
- `worktrees/macos`：当前 macOS-first 共享前端和 Tauri 实现源。
- `worktrees/windows`：Windows 平台对应实现源。
- `worktrees/integration`：如果后续已经完成平台合并，优先从 integration 发布。

不建议把顶层 `worktrees/` 整个目录作为 GitHub 仓库内容上传。顶层仓库更像开发协调仓库，里面会混有多平台 worktree、本地构建产物和临时文件。

## 应该上传

这些文件是开源仓库可构建、可审查所必需的内容。

- 应用源码：`src/`
- Tauri / Rust 源码：`src-tauri/src/`
- Rust 测试：`src-tauri/tests/`
- Rust 配置与锁文件：`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/build.rs`
- Tauri 配置：`src-tauri/tauri.conf.json`
- 应用图标资源：`src-tauri/icons/`
- macOS plist 等必要平台配置：`src-tauri/Info.plist`
- 前端配置：`package.json`、`package-lock.json`、`vite.config.ts`、`tsconfig.json`、`tsconfig.node.json`、`index.html`
- 项目脚本：`scripts/`
- 样式和静态资源：项目实际引用的 `src/assets/`、`public/` 等目录，如果存在。
- 基础文档：`README.md`、`LICENSE`、`docs/architecture/`、`docs/rules/`、`docs/validation/`、`docs/workflows/`
- 当前给 agent/协作者用的必要说明：`AGENTS.md`

## 可以上传，但发布前建议整理

这些内容可以公开，但建议确认是否仍准确、是否包含内部讨论或过期计划。

- `plans/active/`：如果希望公开开发路线，可以上传；如果包含内部草稿，建议删减。
- `docs/quality/debt-log.md`：可以公开技术债，但发布前建议确认措辞。
- `docs/testing/`：如果是通用测试说明，可以上传。
- `redesign/design.md`：可以作为设计规范公开。
- Figma 更新说明、交互文档：如果不包含私密链接、账号信息或未授权素材，可以上传。

## 不应该上传

这些文件不应进入 GitHub。它们要么体积大、可再生成，要么包含本地状态、日志、密钥或临时内容。

- 依赖目录：`node_modules/`
- 前端构建产物：`dist/`
- Rust 构建产物：`target/`、`src-tauri/target/`
- Tauri 打包产物：`.app`、`.dmg`、`.msi`、`.exe`、`.zip`、`src-tauri/target/release/bundle/`
- 本地数据库和运行状态：`*.sqlite`、`*.db`、LanBridge app data 目录中的数据库文件。
- 本地身份密钥：`identity.key`、任何设备私钥或配对密钥。
- 运行日志和崩溃日志：`lanbridge.log`、`startup-crash.log`、`crash-diagnostics.log`
- 系统垃圾文件：`.DS_Store`、`Thumbs.db`
- 编辑器和本机配置：`.idea/`、`.vscode/`（除非只保留通用推荐配置）、`.claude/`
- 本地测试文件夹、用户同步样本文件、真实个人文件。
- 临时视频工程：`remotion-promo/`
- Remotion 生成的视频、截图、缓存和导出素材，除非明确要作为宣传素材单独发布。
- 下载来的第三方源码整包或大型参考项目，除非许可证允许且确实是项目依赖的一部分。

## remotion-promo 处理

`worktrees/macos/remotion-promo/` 是本地制作 Remotion 宣传视频用的临时工程，不属于 LanBridge 应用源码。

处理规则：
- 不上传到 GitHub 主仓库。
- 不复制到 integration / release 分支。
- 如果需要公开宣传视频，只上传最终压缩后的视频成品到 release assets 或单独的宣传仓库，不上传整个 Remotion 工程。

## 发布前检查清单

上传前建议执行以下检查。

```bash
git status --short
git diff --check
npm run lint:names
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

还需要人工确认：
- `git status --short` 中没有 `node_modules/`、`dist/`、`target/`、`remotion-promo/`。
- 没有 `.dmg/.app/.msi/.exe` 等打包产物。
- 没有 `lanbridge.log`、`startup-crash.log`、`crash-diagnostics.log`。
- 没有 `identity.key`、数据库、真实同步文件夹内容。
- 没有私人路径、内网 IP、账号、token、证书、签名密钥。
- README 中清楚说明 LanBridge 是 Primary/Secondary + 显式回传模型，不描述为“完全双向同步”。

## 建议的 .gitignore 补充

如果发布根目录还没有忽略这些规则，建议补充：

```gitignore
node_modules/
dist/
target/
src-tauri/target/
src-tauri/target/release/bundle/
*.app
*.dmg
*.msi
*.exe
*.zip
.DS_Store
Thumbs.db
.claude/
.vscode/
.idea/
remotion-promo/
lanbridge.log
startup-crash.log
crash-diagnostics.log
identity.key
*.sqlite
*.db
```

## 最小推荐开源包结构

如果要整理一个干净的 GitHub 仓库，建议至少包含：

```text
AGENTS.md
README.md
LICENSE
package.json
package-lock.json
vite.config.ts
tsconfig.json
tsconfig.node.json
index.html
scripts/
src/
src-tauri/
docs/
```

其中 `src-tauri/` 内应保留源码、配置、图标和测试，但不要包含 `src-tauri/target/`。
