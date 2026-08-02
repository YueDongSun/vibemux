# VibeMux 工程约定

- Windows 10/11 是正式平台；不要把 WSL 当作运行前置条件。
- 核心模型不得写死 tmux 或 WezTerm 字段。
- 不执行 user message；使用 argv/list 和 stdin，禁止 shell 拼接。
- 不自动 stash/reset/clean/commit/merge/push；cleanup 默认 dry-run。
- `.vibemux/`、本地数据库、日志和虚拟环境不得提交。
- 重大架构决定写入 ADR，包含 Context、Decision、Consequences、Alternatives。

