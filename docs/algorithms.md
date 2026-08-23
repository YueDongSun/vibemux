# Algorithms and invariants

状态转换由 `models.py` 的显式 transition table 驱动。状态写入和事件追加在同一 SQLite transaction 中完成。每个 Run 持久化创建时的 `base_commit`；diff 将 working tree 与该 commit 比较，并补充 untracked file patch，因此覆盖 committed、staged、unstaged 和 untracked 改动。Git/terminal 命令统一通过 shell-free CommandRunner；tmux 将 agent_host 与目标保持为独立 argv。外部 Git/terminal side effect 失败时 Run 进入 failed，并保留失败事件。路径使用 `commonpath`/`normcase`，不使用 startswith；cleanup 先 plan、默认 dry-run，拒绝 dirty、unknown、main 或 root 外 worktree。
