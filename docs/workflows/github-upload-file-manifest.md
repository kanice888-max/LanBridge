# LanBridge GitHub 上传文件名单

本文是 LanBridge 开源发布时的长期文件清单，用来防止上下文压缩或换人接手后丢失上传规则。

目标：GitHub 仓库只包含可构建、可审查、可维护的应用源码和公开文档；不要上传本地 worktree、构建产物、日志、数据库、身份密钥、真实测试文件或临时宣传工程。

## 基准来源

- 正式发布源码基准：使用已完成干净发布检查的 `worktrees/integration` 提交。
- macOS 或 Windows 分支仍在单独开发时，可以从对应 worktree 同步实现；在两个平台分支完成必要验证前，不把任一平台 worktree 直接当作正式发布源码。
- 根目录 `main` 是产品文档、架构、计划和工作流的协调分支，不自动等同于可发布的应用源码快照。只有 integration 候选内容经过有意提升并完成验证后，才可以发布到公开 GitHub 的默认分支。
- 无论从哪个 worktree 取发布候选，都不要上传整个 `worktrees/` 目录或开发机根目录快照。

## 必须上传

- 应用源码：`src/`
- Tauri / Rust 源码：`src-tauri/src/`
- Rust 测试：`src-tauri/tests/`
- Rust 配置和锁文件：`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/build.rs`
- Tauri 配置和必要资源：`src-tauri/tauri.conf.json`、`src-tauri/icons/`、`src-tauri/Info.plist`
- 本地 Rust patch：`src-tauri/patches/tao-0.16.11/`
- 前端入口和配置：`index.html`、`package.json`、`package-lock.json`、`vite.config.ts`、`tsconfig.json`、`tsconfig.node.json`
- 项目脚本：`scripts/`
- 应用实际引用的静态资源：`src/assets/`、`public/`，如果存在且被源码引用。
- 公开基础文档：`README.md` 及其语言版本、`LICENSE`、`SECURITY.md`、`AGENTS.md`
- 必要 docs：`docs/architecture/`、`docs/product/`、`docs/release/`、`docs/rules/`、`docs/security/`、`docs/validation/`、`docs/workflows/`、`docs/testing/`
- 公开质量记录：`docs/quality/debt-log.md`
- 设计规范：`redesign/design.md`

## 可以上传，但发布前需要审查

- 公开开发路线或执行计划。
- 通用测试说明和复现文档。
- 技术债、已知限制、后续优化计划。
- Figma 或交互说明的脱敏版本。
- 宣传用最终成品，例如压缩后的视频或截图；优先放到 GitHub Release assets 或单独宣传仓库。

## 禁止上传

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

上述检查应在干净的 integration 候选环境执行；正式发布还必须满足 `docs/validation/checks.md` 中要求的原生 Windows 与 macOS 发布门禁。

检查是否误跟踪禁止上传内容：

```bash
git ls-files \
  | rg '(^|/)(worktrees|node_modules|dist|target|remotion-promo)(/|$)|(^|/)\.env(\.[^/]+)?$|\.DS_Store|Thumbs\.db|\.dmg$|\.app$|\.msi$|\.exe$|\.(pem|p12|pfx|keystore|key)$|lanbridge\.log|startup-crash\.log|crash-diagnostics.*\.log|\.sqlite$|\.db$' \
  | rg -v '(^|/)\.env\.example$'
```

期望没有命中。若命中的是文档中的示例路径，需要人工确认；若命中真实文件，应从 Git 跟踪中移除。

## 人工检查清单

- GitHub 文件列表没有 `worktrees/`、`remotion-promo/`、安装包、日志、数据库、密钥。
- README 说明 LanBridge 是 Primary/Secondary + 显式回传模型，不描述为完全双向同步。
- `SECURITY.md` 可从 README 进入。
- docs 中没有本机绝对路径、私人账号、token、证书、签名密钥或真实同步文件。
- `.gitignore` 覆盖本文禁止上传的主要类别，以及常见的环境变量和证书/签名凭证文件。

## 最小推荐仓库结构

```text
AGENTS.md
README.md
README.en.md
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
