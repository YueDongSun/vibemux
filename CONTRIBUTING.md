# Contributing

1. 创建本地 `.venv` 并执行 `python -m pip install -e ".[dev]"`。
2. 运行 `python -m pytest` 和 `python -m ruff check .`。
3. 使用 Conventional Commits；每个 commit 保持单一阶段。
4. 不提交 `.env`、凭据、`.vibemux/` 或用户机器绝对路径。
5. backend 行为应同时覆盖 Mock 合约，并在可用平台运行 tmux/WezTerm contract tests。

