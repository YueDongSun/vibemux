# Platform support

| 平台/组件 | 定位 | 状态 |
|---|---|---|
| Windows native | 正式产品目标 | 核心、Mock与pre-alpha daemon lifecycle可运行 |
| WSL2 | 开发与可选 POSIX backend | 兼容 |
| Linux | 开发与 CI | 兼容 |
| macOS | best effort | 未承诺 |
| Windows Terminal | shell/launcher 入口 | 非 pane backend |
| WezTerm | Windows primary backend | command contract 已实现，live 需本机依赖 |
| tmux | POSIX backend | command contract 已实现 |
| ConPTY | 原生未来 backend | 未实现 |
| `vibemuxd` / `vibemuxctl` | Windows named pipe / POSIX UDS | Windows实机与Linux容器process smoke通过 |
