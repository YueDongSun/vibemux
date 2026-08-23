# Changelog

## Unreleased

- 持久化 Run base commit，并修复 committed/staged/unstaged/untracked diff 语义。
- 为 Git、terminal 和 harness host 引入 injected shell-free CommandRunner。
- 增加 SQLite forward migration 与受控 Windows PowerShell companion launcher。
- 增加 immutable cleanup plan、spawn compensation、持久化 mock inventory 与资源 reconciliation。
- Run 成功状态要求 structured adapter、verifier 或显式用户授权。
- 冻结 Python domain/event/cleanup/terminal contract fixture，供 Rust parity 测试复用。
- 增加 Rust `vibemux_store`：SQLite migration、WAL、atomic state/event、idempotency 与 replay 基础。
- 增加 Rust `vibemux_a2a`：官方 A2A v1 SDK Agent Card discovery 与本地 HTTP+JSON 基础信息分享。
- 增加 Rust `vibemux_probe` 与 `vibemux_frontend`：固定化 agent/gateway/A2A只读探测、统一 Ratatui dashboard和五个 native-TUI reserved slots。

## 0.1.0a0

- 建立 Windows-first Python package、领域模型、SQLite event log、Git worktree 和 terminal/harness adapters。
- 添加 CLI、跨平台 smoke、文档、ADR 与 GitHub Actions 矩阵。
