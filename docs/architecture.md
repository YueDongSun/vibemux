# Architecture

```text
PowerShell / CLI / Future TUI
             |
      Application Services
             |
   +---------+----------+
   |         |          |
 Domain    Storage    Policy
   |         |          |
   +---- Canonical Event Model ----+
   |                                |
   v                                v
WorkspaceManager              TerminalBackend
Git worktree                  WezTerm / tmux / mock
   |                                |
   +---------- Run Orchestrator ----+
                    |
              ExecutionBackend
          Native Windows / POSIX
                    |
              HarnessAdapter
       Generic PTY / mock / future ACP
```

“同一个 folder”指同一逻辑 repo；每个 Run 使用不同物理 worktree。worktree 只隔离并发修改，不是 sandbox。TerminalBackend 管 pane 生命周期，不判断 Task 成功；ExecutionBackend 描述 runtime；HarnessAdapter 只构造受控启动规格。ACP/A2A/MCP 是未来 gateway，当前不伪造实现。

