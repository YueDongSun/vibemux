# Protocol boundaries

- Internal Event：SQLite append-only canonical model，独立于终端输出。
- Local control IPC：`vibemuxd::control` 已实现 private pre-alpha v1 的 authenticated Windows named-pipe/POSIX UDS transport，仅开放 `health`/`shutdown`。Windows descriptor/lock位于protected per-user hashed runtime且pipe拒绝remote clients；POSIX descriptor为`0600`。没有TCP fallback。`vibemuxctl`只负责standalone daemon lifecycle与显式stale metadata recovery，不直接访问SQLite或暴露token/endpoint path。
- PTY：只提供交互输入输出，不推断 completion/approval。
- ACP/MCP：未来协议边界，当前不创建 fake gateway。
- Plugin stdio：`vibemux_plugin_protocol` 已实现private pre-alpha Protobuf v1 schema、固定上限framing、manifest/negotiation与session lifecycle；该wire crate本身保持无process依赖，SDK与真实plugin仍未实现。
- Plugin supervisor：`vibemux_plugin_supervisor` 已用真实Windows/Linux mock child验证stdout-only framing、独立bounded stderr metadata、bounded queues、deadline/heartbeat/cancel/drain/shutdown/crash；尚无daemon registry、restart policy、SDK或真实plugin。
- A2A：`vibemux_a2a` 已验证一个隔离的 A2A v1 HTTP+JSON 本地 loopback information-share slice：官方 Agent Card discovery、官方 client/server transport、bounded Message/DataPart 和 correlation-preserving acknowledgement。该 slice 不读写 canonical store，不创建 VibeMux Task/Run，不支持 remote bind、JSON-RPC、gRPC、streaming、artifact、authentication 或 TCK；这些能力不得从本地 smoke 推断。
- Git artifact：worktree、diff、commit 是可审计产物，不自动 merge。
