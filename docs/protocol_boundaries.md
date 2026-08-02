# Protocol boundaries

- Internal Event：SQLite append-only canonical model，独立于终端输出。
- PTY：只提供交互输入输出，不推断 completion/approval。
- ACP/A2A/MCP：未来协议边界，本轮不创建 fake gateway。
- Git artifact：worktree、diff、commit 是可审计产物，不自动 merge。

