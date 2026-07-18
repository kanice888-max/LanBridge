# LanBridge GitHub 开源发布文件清单

详细、长期维护的上传文件名单以 [GitHub 上传文件名单](github-upload-file-manifest.md) 为准；本文保留为开源发布流程和历史整理说明。

本文用于整理 LanBridge 开源发布到 GitHub 时应该上传、可以上传、不要上传的文件。目标是公开可构建、可审查的源码，同时避免把本地临时文件、打包产物、日志、数据库、身份密钥和大体积无关素材传上去。

## 推荐发布方式

优先发布一个已通过验证的 integration 候选源码快照，而不是直接把当前协调仓库或任一开发 worktree 的全部内容原样公开。

推荐来源：
- `worktrees/integration`：两个平台分支完成必要验证并合并后，在干净环境完成发布检查的唯一正式候选来源。
- `worktrees/macos`、`worktrees/windows`：仅用于各自平台的开发和本地验证；未完成 integration 合并与验证前，不能直接作为正式发布来源。

根目录 `main` 是产品文档、架构、计划和工作流的协调分支；其内容不能仅因位于 `main` 就视为发布就绪。只有 integration 候选经过有意提升和验证后，才可成为公开 GitHub 的默认分支内容。无论如何都不要把顶层 `worktrees/` 整个目录作为 GitHub 仓库内容上传。

## 应该上传

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
- 基础文档：`README.md` 及其语言版本、`LICENSE`、`SECURITY.md`、`docs/architecture/`、`docs/rules/`、`docs/security/`、`docs/validation/`、`docs/workflows/`
- 当前给 agent/协作者用的必要说明：`AGENTS.md`

## 可以上传，但发布前建议整理

- `plans/active/`：如果希望公开开发路线，可以上传；如果包含内部草稿，建议删减。
- `docs/quality/debt-log.md`：可以公开技术债，但发布前建议确认措辞。
- `docs/testing/`：如果是通用测试说明，可以上传。
- `redesign/design.md`：可以作为设计规范公开。
- Figma 更新说明、交互文档：如果不包含私密链接、账号信息或未授权素材，可以上传。

## 不应该上传

- 依赖目录：`node_modules/`
- 前端构建产物：`dist/`
- Rust 构建产物：`target/`、`src-tauri/target/`
- Tauri 打包产物：`.app`、`.dmg`、`.msi`、`.exe`、`.zip`、`src-tauri/target/release/bundle/`
- 本地数据库和运行状态：`*.sqlite`、`*.db`、LanBridge app data 目录中的数据库文件。
- 本地身份密钥：`identity.key`、任何设备私钥或配对密钥。
- 运行日志和崩溃日志：`lanbridge.log`、`startup-crash.log`、`crash-diagnostics.log`
- 系统垃圾文件：`.DS_Store`、`Thumbs.db`
- 编辑器和本机配置：`.idea/`、`.vscode/`、`.claude/`
- 本地测试文件夹、用户同步样本文件、真实个人文件。
- 临时视频工程：`remotion-promo/`
- Remotion 生成的视频、截图、缓存和导出素材，除非明确要作为宣传素材单独发布。
- 下载来的第三方源码整包或大型参考项目，除非许可证允许且确实是项目依赖的一部分。

## 发布前检查清单

```bash
cd worktrees/integration
npm ci
git status --short
git diff --check
npm run lint:names
npm run build
npm test
cargo test --manifest-path src-tauri/Cargo.toml
```

除上述通用检查外，正式发布仍须满足 `docs/validation/checks.md` 规定的原生 Windows 和 macOS 发布门禁。

还需要人工确认：
- `git status --short` 中没有 `node_modules/`、`dist/`、`target/`、`remotion-promo/`。
- 没有 `.dmg/.app/.msi/.exe` 等打包产物。
- 没有 `lanbridge.log`、`startup-crash.log`、`crash-diagnostics.log`。
- 没有 `identity.key`、数据库、真实同步文件夹内容。
- 没有私人路径、内网 IP、账号、token、`.env` 环境变量文件、证书或签名密钥。
- README 中清楚说明 LanBridge 是 Primary/Secondary + 显式回传模型，不描述为“完全双向同步”。

## 建议的 .gitignore 补充

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
.env
.env.*
!.env.example
*.pem
*.p12
*.pfx
*.keystore
*.key
```

## 最小推荐开源包结构

```text
AGENTS.md
README.md
README.en.md
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
