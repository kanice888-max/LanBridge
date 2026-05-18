# Task Flow

Use this flow for non-trivial code changes.

1. Identify the owning worktree: Windows, macOS, or integration.
2. Read `docs/architecture/index.md`, `docs/architecture/monorepo-map.md`, and `docs/rules/invariants.md`.
3. If the task changes behavior across devices, add or update a Rust integration test first.
4. Make the smallest implementation change.
5. Apply the same shared fix to both platform worktrees when relevant.
6. Run targeted checks first, then broader checks from `docs/validation/checks.md`.
7. Update active plans and debt log when scope, risk, or follow-up work changes.
8. Report what changed, what passed, and what remains unvalidated.

## Sync Feature Review Path

For sync behavior, review every segment:

```text
发现设备 → 连接 → 配对/身份 → 建任务 → 计划同步 → 传输协议 → 接收落盘 → ACK/重试/状态收尾
```

Do not call the flow fixed unless the relevant test or manual check covers the full path.
