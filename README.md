# VibeMux

VibeMux 是 Windows-first、本地优先、terminal-native 的多 coding-harness 协作环境。正式目标是 Windows 10/11 + PowerShell；WSL2/Linux 是开发和可选 tmux backend。Windows Terminal 只是入口，WezTerm 才是首个可编程 terminal backend。

## 快速开始（Windows PowerShell）

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install -e ".[dev]"
vibemux --version
vibemux init --terminal-backend mock
vibemux doctor
```

目标 repo 必须已有 initial commit。每个 Run 使用独立 `.vibemux/worktrees/<run>`，这是并发修改隔离，不是安全 sandbox；不提供网络隔离、secret broker 或容器边界。

## 核心边界

`Task/Run/Event` 是与终端无关的领域模型。`TerminalBackend` 只管理 pane；`ExecutionBackend` 描述命令在哪个 runtime 执行；`HarnessAdapter` 生成受控 argv。用户消息永远不进入 shell 字符串，事件只记录内容 hash/大小。

当前实现包含 Mock、WezTerm command adapter、tmux adapter、Native/POSIX 兼容建模、Git worktree、SQLite append-only event log、safe diff/stop/cleanup 基础和离线 mock harness。ACP/A2A/MCP、ConPTY、scheduler、daemon、自动 merge/push 均明确不在 MVP。

## Mock workflow

```powershell
$task = vibemux task "验证隔离"
$run = vibemux spawn $task --harness mock --terminal-backend mock
vibemux send $run "PING" --submit
vibemux trace
vibemux stop $run
```

## 开发命令

```powershell
python -m pytest
python -m ruff check .
python scripts/smoke_test.py
```

详见 [docs/architecture.md](docs/architecture.md)、[docs/platform_support.md](docs/platform_support.md)、[docs/protocol_boundaries.md](docs/protocol_boundaries.md) 与 `docs/adr/`。

