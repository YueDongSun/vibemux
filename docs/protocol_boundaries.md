# Protocol boundaries

- Internal Event：SQLite append-only canonical model，独立于终端输出。
- PTY：只提供交互输入输出，不推断 completion/approval。
- ACP/MCP：未来协议边界，当前不创建 fake gateway。
- A2A：`vibemux_a2a` 已验证一个隔离的 A2A v1 HTTP+JSON 本地 loopback information-share slice：官方 Agent Card discovery、官方 client/server transport、bounded Message/DataPart 和 correlation-preserving acknowledgement。该 slice 不读写 canonical store，不创建 VibeMux Task/Run，不支持 remote bind、JSON-RPC、gRPC、streaming、artifact、authentication 或 TCK；这些能力不得从本地 smoke 推断。
- Git artifact：worktree、diff、commit 是可审计产物，不自动 merge。
