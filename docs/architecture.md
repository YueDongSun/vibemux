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
                    |
           controlled agent_host
                    |
        injected shell-free CommandRunner
```

“同一个 folder”指同一逻辑 repo；每个 Run 使用不同物理 worktree。worktree 只隔离并发修改，不是 sandbox。TerminalBackend 管 pane 生命周期，不判断 Task 成功；ExecutionBackend 描述 runtime；HarnessAdapter 只构造受控启动规格。Git、WezTerm 和 tmux 共用 injected CommandRunner；agent_host 只接收受控 argv/environment，不执行 shell command string。ACP/A2A/MCP 是未来 gateway，当前不伪造实现。

## Probe 与统一前端边界

`vibemux_probe` 只产生版本化、非内容、只读诊断：launcher/version、显式 provider endpoint、CC Switch health/aggregate telemetry 和 A2A self-test。launcher验证不等于 authentication/inference验证。

`vibemux_frontend` 消费 probe report 和未来 daemon control API；不直接读写 core SQLite。统一 dashboard 为 Claude、Codex、OpenCode、Copilot 和 Grok分别保留 `NativeTuiSlot`。这些 slot最终由 terminal plugin/ConPTY承载 CLI自身 TUI；dashboard不解析 terminal text、不重绘 vendor TUI，也不从 pane exit推断完成。当前 slot仅为 `reserved`，没有启动或输入转发行为。

## Daemon writer ownership

`vibemuxd::WriterWorker` 在 dedicated blocking thread内构造并独占 `SqliteStore`。调用方只持有 bounded queue handle；queue满时返回显式 backpressure。数据库旁的 nonce lock通过原子 `create_new`建立，第二 writer fail closed，shutdown后由持有者核验 nonce再删除。

`vibemuxd::control` 在 Windows 使用 per-instance named pipe，在 POSIX 使用随机化的 per-instance Unix-domain socket。listener bind成功后才发布含 protocol version、endpoint和OS CSPRNG bearer token的 ignored runtime descriptor；四字节大端长度前缀在分配JSON payload前执行64 KiB上限，连接读写与peer-close均有deadline。descriptor owner token同时保护descriptor/socket cleanup，旧实例不得删除替代实例的runtime artifact。当前只开放 versioned `health`和`shutdown`，blocking writer调用通过 `spawn_blocking`隔离，无TCP fallback。standalone user-mode daemon进程、CLI bootstrap/start/query/stop、stale-instance recovery和Windows hostile same-user ACL hardening仍未实现。
