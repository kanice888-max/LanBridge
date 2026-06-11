# LanBridge GitHub 上传文件名单

本文是 LanBridge 开源发布时的长期文件清单，用来防止上下文压缩或换人接手后丢失上传规则。

目标：GitHub 仓库只包含可构建、可审查、可维护的应用源码和公开文档；不要上传本地 worktree、构建产物、日志、数据库、身份密钥、真实测试文件或临时宣传工程。

## 基准来源

- 正式发布源码基准：优先使用 `worktrees/integration`。
- macOS 或 Windows 分支仍在单独开发时，可以从对应 worktree 同步实现，但不要上传整个 `worktrees/` 目录。
- 根目录 `main` 应作为公开应用源码仓库维护，而不是开发机上的协调工作区快照。

## 必须上传

这些内容是开源仓库可构建、可审查所需内容。

- 应用源码：`src/`
- Tauri / Rust 源码：`src-tauri/src/`
- Rust 测试：`src-tauri/tests/`
- Rust 配置和锁文件：`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/build.rs`
- Tauri 配置和必要资源：`src-tauri/tauri.conf.json`、`src-tauri/icons/`、`src-tauri/Info.plist`
- 本地 Rust patch：`src-tauri/patches/tao-0.16.11/`
- 前端入口和配置：`index.html`、`package.json`、`package-lock.json`、`vite.config.ts`、`tsconfig.json`、`tsconfig.node.json`
- 项目脚本：`scripts/`
- 应用实际引用的静态资源：`src/assets/`、`public/`，如果存在且被源码引用。
- 公开基础文档：`README.md`、`LICENSE`、`SECURITY.md`、`AGENTS.md`
- 必要 docs：`docs/architecture/`、`docs/rules/`、`docs/validation/`、`docs/workflows/`、`docs/testing/`
- 公开质量记录：`docs/quality/debt-log.md`
- 设计规范：`redesign/design.md`

## 可以上传，但发布前需要审查

这些内容可以公开，但必须确认不包含内部草稿、私人路径、账号、token、证书、签名密钥、真实同步文件或未授权素材。

- 公开开发路线或执行计划。
- 通用测试说明和复现文档。
- 技术债、已知限制、后续优化计划。
- Figma 或交互说明的脱敏版本。
- 宣传用最终成品，例如压缩后的视频或截图；优先放到 GitHub Release assets 或单独宣传仓库。

## 禁止上传

这些内容不应进入 GitHub 主仓库。

- 本地 worktree：`worktrees/`、`.worktrees/`
- 依赖目录：`node_modules/`
- 前端构建产物：`dist/`
- Rust/Tauri 构建产物：`target/`、`src-tauri/target/`
- Tauri 打包产物：`.app`、`.dmg`、`.msi`、`.exe`、`.zip`、`src-tauri/target/release/bundle/`
- 运行日志和崩溃日志：`lanbridge.log`、`startup-crash.log`、`crash-diagnostics.log`、`crash-diagnostics-*.log`
- 本地数据库和运行状态：`*.sqlite`、`*.db`
- 本地身份密钥：`identity.key`、任何设备私钥、配对密钥或签名密钥。
- 系统和编辑器文件：`.DS_Store`、`Thumbs.db`、`.claude/`、`.codex/`、`.vscode/`、`.idea/`
- 临时视频工程：`remotion-promo/`
- Remotion 缓存、导出中间文件、未压缩素材工程。
- `redesign/folder-animation-preview/` 预览工程。
- 私有 Word 文档：`docs/superpowers/*.docx`
- 本地测试文件夹、用户同步样本、真实个人文件。
- 下载来的第三方源码整包或大型参考项目，除非许可证允许且它确实是构建依赖。

## 特殊保留规则

- `src-tauri/patches/tao-0.16.11/` 必须保留，因为 `Cargo.toml` 通过本地 patch 依赖它。
- `package-lock.json` 必须保留，用于复现 npm 依赖。
- `src-tauri/Cargo.lock` 必须保留，用于复现 Rust/Tauri 构建。
- `redesign/design.md` 可以公开；完整预览工程和临时动画实验工程不上传。
- `remotion-promo/` 永远不进入主仓库；如果要公开宣传视频，只发布最终成品。
- `SECURITY.md` 和 `docs/security/security-hardening-plan.md` 应保留，开源发布后用于说明安全边界和后续加固计划。

## 发布前检查命令

上传或推送前先运行：

```bash
git status --short
git diff --check
npm run lint:names
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

检查是否误跟踪禁止上传内容：

```bash
git ls-files | rg '(^|/)(worktrees|node_modules|dist|target|remotion-promo)(/|$)|\.DS_Store|Thumbs\.db|\.dmg$|\.app$|\.msi$|\.exe$|lanbridge\.log|startup-crash\.log|crash-diagnostics.*\.log|identity\.key|\.sqlite$|\.db$'
```

期望没有命中。若命中的是文档中的示例路径，需要人工确认；若命中真实文件，应从 Git 跟踪中移除。

## 人工检查清单

- GitHub 文件列表没有 `worktrees/`、`remotion-promo/`、安装包、日志、数据库、密钥。
- README 说明 LanBridge 是 Primary/Secondary + 显式回传模型，不描述为完全双向同步。
- `SECURITY.md` 可从 README 进入。
- docs 中没有本机绝对路径、私人账号、token、证书、签名密钥或真实同步文件。
- `.gitignore` 覆盖本文禁止上传的主要类别。

## 最小推荐仓库结构

```text
AGENTS.md
README.md
LICENSE
SECURITY.md
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
redesign/design.md
```

`src-tauri/` 内应包含源码、配置、图标、测试和必要 patch，但不要包含 `src-tauri/target/`。
