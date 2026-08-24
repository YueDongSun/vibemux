# VibeMux

VibeMux is a Windows-first, local-first, terminal-native collaboration environment for multiple coding harnesses. The official target is Windows 10/11 + PowerShell; WSL2/Linux is for development and the optional tmux backend. Windows Terminal is only the entry point; WezTerm is the first programmable terminal backend.

> **Current status: pre-alpha.** The Python CLI remains the behavior reference for the complete prototype workflow. Rust implements the verified M3 daemon/store/local-IPC scope plus isolated M4.0 protocol and M4.1 process-supervisor foundations. VibeMux is not yet a functional multi-harness alpha.

## Current status

| Path | Status | Implemented scope | Not implemented or integrated |
|---|---|---|---|
| Python `vibemux` | `PARTIAL — BEHAVIOR REFERENCE` | Mock workflow, Git worktrees, safe diff/stop/cleanup, SQLite event log, resource ownership/reconciliation, and compatibility fixtures | Not the future authoritative core; new orchestration, A2A, and public plugin behavior must target Rust |
| Rust core / `vibemuxd` | `VERIFIED` within M3 | Typed IDs and state machines, canonical events, SQLite store, single-writer worker, authenticated local IPC, and start/health/inspect/recover/stop | Complete Task/Run CLI parity and Python-to-Rust state migration/default cutover |
| Plugin protocol / supervisor | `PARTIAL` overall M4 | M4.0 wire/manifest/negotiation/lifecycle and M4.1 shell-free child supervision, bounded queues, heartbeat/cancel/drain/shutdown, and crash containment | Daemon-owned registry/control, restart budget/quarantine, real vendor plugins, and SDKs |
| A2A | `PARTIAL — LOOPBACK ONLY` | Local-loopback HTTP+JSON information-share validation slice | Stateful/remote A2A, authentication/TLS, streaming, JSON-RPC/gRPC, and TCK/ITK conformance |
| Probe / frontend | `PARTIAL` | Read-only launcher/gateway/A2A probes and a Ratatui shell | Real PTY/ConPTY attach and native-TUI ownership/input/focus/resize/teardown |

## Python behavior-reference quick start (Windows PowerShell)

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install -e ".[dev]"
vibemux --version
vibemux init --terminal-backend mock
vibemux doctor
```

The target repo must already have an initial commit. Each Run uses an independent `.vibemux/worktrees/<run>`; this is concurrent-modification isolation, **not** a security sandbox. There is no network isolation, secret broker, or container boundary.

## Core boundaries

`Task/Run/Event` is the terminal-independent domain model. `TerminalBackend` only manages panes; `ExecutionBackend` describes the runtime that executes commands; `HarnessAdapter` produces a controlled argv. User messages never enter a shell string; events only record content hash/size.

The Python behavior reference includes Mock, a WezTerm command adapter, a tmux adapter, Native/POSIX-compatible execution modeling, Git worktree management, an append-only SQLite event log, safe diff/stop/cleanup primitives, an offline mock harness, resource ownership, and reconciliation. ACP/MCP vendor adapters, ConPTY, the scheduler, and automatic merge/push are not implemented.

## Rust core migration

`99d1f8e` is only the original Python audit baseline; it is not the current implementation baseline. The Python package remains the behavior reference and complete prototype workflow entry point, but new orchestration, A2A, event-broker, scheduler, and public plugin behavior must target Rust.

The Rust 2024 workspace (`0.2.0-alpha.0`, MSRV 1.85) implements platform-independent typed IDs and Task/Run state machines, a canonical event envelope, SQLite migrations/WAL and atomic state-plus-event foundations, a dedicated single-writer worker, authenticated Windows named-pipe/POSIX UDS control, standalone `vibemuxd`, the pre-alpha `vibemuxctl` daemon lifecycle, explicit stale-runtime recovery, the declared Windows per-user control-runtime boundary, a loopback-only A2A v1 HTTP+JSON information-share slice, read-only probe/frontend shells, and the isolated M4.0/M4.1 foundations.

Python `vibemux` remains the complete prototype entry point; Rust `vibemuxctl` provides daemon lifecycle commands only. Complete Rust Task/Run command parity, Python-to-Rust state migration/default cutover, daemon-owned plugin registry/control integration, restart budget/quarantine, Rust workspace/terminal parity, real vendor plugins, plugin SDKs, and stateful/remote A2A conformance are not implemented. Authoritative milestones and verification boundaries live in [PROGRESS.md](PROGRESS.md).

The Rust workspace also includes the isolated `vibemux_plugin_protocol` M4.0 foundation: a checked-in Protobuf v1 schema, bounded framing, a TOML manifest, permission/capability negotiation, and a handshake lifecycle. The wire crate itself does not spawn processes and cannot mutate Task/Run state or SQLite.

M4.1 `vibemux_plugin_supervisor` has been validated on Windows/Linux against a real mock child for shell-free spawn, clean environment, bounded queues, stderr cap, heartbeat/cancel/drain/shutdown, and crash containment. The isolated process-supervisor foundation is implemented. Daemon-owned plugin registry/control integration, restart budget/quarantine, real vendor plugins, and Task/Run mutation integration are not implemented; `vibemuxd` does not currently depend on the supervisor crate.

## Rust daemon lifecycle (M3 verified scope)

During the migration, Rust only writes `.vibemux/vibemux_rust.sqlite3` and never opens the Python `.vibemux/vibemux.sqlite3`. `vibemuxctl` will not overwrite an installed Python `vibemux` command:

```powershell
cargo build -p vibemuxd --bin vibemuxd -p vibemux_cli --bin vibemuxctl
.\target\debug\vibemuxctl.exe daemon start --project-root .
.\target\debug\vibemuxctl.exe daemon health --project-root .
.\target\debug\vibemuxctl.exe daemon inspect --project-root .
.\target\debug\vibemuxctl.exe daemon stop --project-root .
```

`start` uses authenticated IPC health (not PID) as the ready signal. Stale descriptor/lock artifacts fail closed; the lifecycle client never deletes files or terminates unknown processes automatically.

After a daemon crashes, run `inspect` first. Only when status is `recoverable` may the returned 64-bit confirmation be fed into a separate recovery command:

```powershell
.\target\debug\vibemuxctl.exe daemon recover --confirmation <inspect_output> --project-root .
```

Recovery never kills a PID and never opens or modifies the Python/Rust database. It refuses if any artifact, PID, or confirmation changes.

Windows stores the bearer descriptor and cooperative writer lock under `%LOCALAPPDATA%\VibeMux\runtime\<project_hash>`; the protected root and its inheritance rules allow only the current logon user, `SYSTEM`, and Administrators. The project itself still holds only the database; agents under the same logon SID are the collaboration trust domain. POSIX continues to use the project-local `.vibemux/` descriptor at mode `0600` and a randomized UDS path.

The release-start performance can be re-measured with a fixed script; the script refuses a project that already has a daemon descriptor:

```powershell
cargo build --release -p vibemuxd --bin vibemuxd -p vibemux_cli --bin vibemuxctl
.\scripts\benchmark_daemon_start.ps1 -project_root C:\path\to\test_project -sample_count 20
```

## Mock workflow

```powershell
$task = vibemux task "validate isolation"
$run = vibemux spawn $task --harness mock --terminal-backend mock
vibemux send $run "PING" --submit
vibemux trace
vibemux stop $run
```

## Development commands

```powershell
python -m ruff format --check .
python -m ruff check .
python -m mypy .
python -m pytest
python scripts/smoke_test.py

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

The current GitHub Actions workflow runs Python pytest/Ruff/smoke and Rust fmt/Clippy/workspace tests on Windows and Ubuntu. Ruff format, mypy, package build, coverage, `cargo nextest`, `cargo deny`, `cargo audit`, fuzzing, and live terminal-backend validation are not all enforced in CI yet.

## Local probe and unified frontend preview

The following commands only execute launcher version checks, explicit endpoint parsing, CC Switch read-only telemetry, and a local A2A self-test; they do not send model prompts:

```powershell
cargo run -p vibemux_probe --bin vibemux_probe
cargo run -p vibemux_frontend --bin vibemux_frontend -- --once
```

The interactive Ratatui shell can be started without `--once`; press `q` / `Esc` to exit. Claude, Codex, OpenCode, Copilot, and Grok native TUIs currently display a reserved slot only; real PTY/ConPTY attach is not implemented and must not be treated as usable.

See [docs/architecture.md](docs/architecture.md), [docs/platform_support.md](docs/platform_support.md), [docs/protocol_boundaries.md](docs/protocol_boundaries.md), and `docs/adr/`.
