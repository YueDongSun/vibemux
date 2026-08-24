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

当前实现包含 Mock、WezTerm command adapter、tmux adapter、Native/POSIX 兼容建模、Git worktree、SQLite append-only event log、safe diff/stop/cleanup 基础、离线 mock harness，以及pre-alpha Rust daemon生命周期。ACP/MCP、ConPTY、scheduler、自动 merge/push与完整远程A2A仍未实现。

## Rust 核心迁移

Python `99d1f8e` 是当前行为参考，不再承接新的 orchestration 或 A2A 功能。Rust 2024 workspace 已实现与平台无关的 typed IDs、Task/Run 状态机、canonical event envelope、SQLite store、dedicated single-writer worker、authenticated Windows named-pipe/POSIX UDS control transport、standalone `vibemuxd`与pre-alpha `vibemuxctl`生命周期，以及一个仅限本地 loopback 的 A2A v1 HTTP+JSON information-share验证切片；Python CLI仍是当前完整入口，Rust command parity/database migration、plugin host、terminal/workspace parity、stateful/remote A2A与 conformance尚未实现。权威里程碑与验证边界见 [PROGRESS.md](PROGRESS.md)。

Rust workspace也已加入isolated `vibemux_plugin_protocol` M4.0基础：checked-in Protobuf v1、bounded framing、TOML manifest、permission/capability negotiation与handshake lifecycle。该wire crate本身不spawn进程，也不能修改Task/Run/SQLite。

M4.1 `vibemux_plugin_supervisor`已在Windows/Linux用真实mock child验证shell-free spawn、clean environment、bounded queues、stderr cap、heartbeat/cancel/drain/shutdown与crash containment；它尚未接入daemon registry、自动restart、真实vendor plugin或Task/Run mutation。

## Rust daemon 生命周期预览

迁移期间 Rust 只写 `.vibemux/vibemux_rust.sqlite3`，不会打开 Python 的 `.vibemux/vibemux.sqlite3`。`vibemuxctl`不会覆盖已安装的 Python `vibemux`命令：

```powershell
cargo build -p vibemuxd --bin vibemuxd -p vibemux_cli --bin vibemuxctl
.\target\debug\vibemuxctl.exe daemon start --project-root .
.\target\debug\vibemuxctl.exe daemon health --project-root .
.\target\debug\vibemuxctl.exe daemon inspect --project-root .
.\target\debug\vibemuxctl.exe daemon stop --project-root .
```

`start`以authenticated IPC health而非PID作为ready依据。stale descriptor/lock会fail closed，不会自动删除文件或终止未知进程。

daemon异常退出后，先运行`inspect`。仅当状态为`recoverable`时，才可把返回的64位confirmation用于单独恢复命令：

```powershell
.\target\debug\vibemuxctl.exe daemon recover --confirmation <inspect返回值> --project-root .
```

恢复不会kill PID，也不会打开或修改Python/Rust数据库；artifact、PID或confirmation发生变化时会拒绝。

Windows将bearer descriptor和cooperative writer lock存放在`%LOCALAPPDATA%\VibeMux\runtime\<project_hash>`；protected根及其继承规则只允许当前登录用户、`SYSTEM`和Administrators。项目内仍只保存数据库；同一登录SID下的agents属于协作信任域。POSIX继续使用项目`.vibemux`下的`0600` descriptor与随机UDS。

release启动性能可用固定脚本复测；脚本会拒绝已有daemon descriptor的项目：

```powershell
cargo build --release -p vibemuxd --bin vibemuxd -p vibemux_cli --bin vibemuxctl
.\scripts\benchmark_daemon_start.ps1 -project_root C:\path\to\test_project -sample_count 20
```

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
cargo test --workspace --all-features
```

## 本机探测与统一前端预览

以下命令只执行 launcher版本、显式 endpoint、CC Switch只读 telemetry和本地 A2A self-test，不发送模型 prompt：

```powershell
cargo run -p vibemux_probe --bin vibemux_probe
cargo run -p vibemux_frontend --bin vibemux_frontend -- --once
```

交互式 Ratatui shell可通过不带 `--once` 启动并使用 `q`/`Esc`退出。Claude、Codex、OpenCode、Copilot、Grok native TUI目前只显示 reserved slot；真实 PTY/ConPTY attach尚未实现，不应理解为可用。

详见 [docs/architecture.md](docs/architecture.md)、[docs/platform_support.md](docs/platform_support.md)、[docs/protocol_boundaries.md](docs/protocol_boundaries.md) 与 `docs/adr/`。
