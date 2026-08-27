# VibeMux

VibeMux is a Windows-first local coordination environment for coding-agent
harnesses. The Rust core separates tasks, process ownership, terminal surfaces,
harness protocols, and A2A. Windows does not require WSL.

Current development version: **0.2.0-alpha.1**. This is a pre-alpha engineering
slice, not a complete autonomous multi-agent product.

## Start With the Unified CLI

```powershell
cargo build --locked --workspace --bins
.\target\debug\vibemuxctl.exe --help
.\target\debug\vibemuxctl.exe --format text status
.\target\debug\vibemuxctl.exe --format text workspace status
.\target\debug\vibemuxctl.exe harness catalog --config examples/harness/harness_config.json
```

The checked-in Rust toolchain and lockfile define the build. This does not
install a global command, start a daemon, or invoke a model.

Use `--format text`, `--format json`, or `--json` before the command. Auto mode
uses readable text in a terminal and JSON when redirected. Help/version remain
text. Existing daemon JSON contracts and standalone binaries remain available.

## Read-only Dashboard

```powershell
.\target\debug\vibemuxctl.exe ui --report crates/vibemux_probe/tests/fixtures/probe_report_v1.json
.\target\debug\vibemuxctl.exe --format text ui --once --report crates/vibemux_probe/tests/fixtures/probe_report_v1.json --width 80 --height 24
```

These commands use a synthetic saved report and do not invoke a harness.
Navigate with Tab or 1/2/3, select with j/k or arrows, open harness details with
Enter, show help with ?, and exit with q/Esc. Native CLI TUI slots remain
reserved or unavailable; there is no ConPTY attachment or tool approval action.

Without `--report`, the fixed read-only probe service runs once. Launcher,
authentication, inference, and task completion are separate evidence levels.
Saved reports are validated, labeled, and projected to safe metadata.

## Native Harness Core

Codex app-server, Claude stream-json, and ACP adapters for OpenCode, Copilot,
and Grok share bounded request handling and private JSON capture. Unknown
vendor fields are retained; oversized, malformed, uncorrelated, and incomplete
streams fail explicitly. Public output omits prompts, vendor bodies, endpoint
details, and local configuration.

```powershell
.\target\debug\vibemuxctl.exe harness replay --protocol codex_app_server --input crates/vibemux_harness/tests/fixtures/codex_app_server.jsonl
.\target\debug\vibemuxctl.exe help harness
```

Public configurations disable execution. Real model requests need explicit
local configuration and authorization and may consume account usage.
The last qualified native initialization checks passed for Codex, Claude,
OpenCode, and Copilot. Grok's shutdown check did not pass. Do not interpret
synthetic tests or these initialization checks as model-generation evidence.

## Explicit Daemon Operations

```powershell
.\target\debug\vibemuxctl.exe daemon start --project-root .
.\target\debug\vibemuxctl.exe daemon health --project-root .
.\target\debug\vibemuxctl.exe daemon inspect --project-root .
.\target\debug\vibemuxctl.exe daemon stop --project-root .
```

Start is explicit and readiness requires authenticated local IPC. Stale state
is not automatically deleted. Recovery requires a fresh matching confirmation.
The Rust daemon keeps its separate database and is the intended canonical
single writer. The standalone harness dispatcher is not yet bound to canonical
daemon Task/Run admission or executable remote A2A tasks.

Worktrees isolate concurrent file changes, not filesystem or network authority.
Windows Job Objects control owned process lifetime, not a security sandbox.

## Repository Layout

```text
crates/                 Rust domain, storage, transport, adapters, CLI, and UI
src/vibemux/            frozen Python behavior reference
tests/                  Python/reference and public-artifact checks
examples/harness/       disabled, machine-independent harness configuration
docs/guides/            user guides and implementation learning material
docs/workflow/          branch and worktree workflow
docs/reports/           dated evidence and verification boundaries
docs/adr/               architecture decisions
.vibemux/               ignored runtime, private configuration, and local worktrees
target/                 ignored Rust build output
```

Historical branch refs are retained. The primary checkout and an isolated
development worktree are distinct; no command automatically pulls, merges,
pushes, or removes them. See the [branch workflow](docs/workflow/branch_workflow.md).

## Python Reference

The existing Python CLI is retained for compatibility and migration tests.
Use its project virtual environment; it is not a second new orchestration core.
The [mock lab](docs/labs/01_mock_workflow.md) documents the reference workflow.

## Development Checks

```powershell
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
.\.venv\Scripts\python.exe -m pytest
.\.venv\Scripts\python.exe -m ruff check .
.\.venv\Scripts\python.exe scripts/check_public_artifacts.py
```

Run Windows process/ACL tests in isolated temporary runtime directories.
When reusing a virtual environment from another checkout, set `PYTHONPATH` to
this worktree's `src` for the validation process. An editable installation can
otherwise import the old checkout; do not change a shared installation just
to run worktree tests.
Additional declared security/conformance/release gates are not implied by
these commands. Do not publish private configuration, test logs, raw capture,
databases, virtual environments, or build output.

## Documentation

- [Unified CLI guide](docs/guides/unified_cli_guide.md)
- [Harness core guide](docs/guides/harness_core_guide.md)
- [A2A implementation and learning path](docs/guides/a2a_implementation_learning_path.md)
- [Architecture](docs/architecture.md) and [ADR 024](docs/adr/024_unified_cli_and_workspace_layout.md)
- [Implementation status](PROGRESS.md) and [documentation map](docs/index.md)
- [Security](SECURITY.md) and [contributing](CONTRIBUTING.md)

No local commit implies a remote push, CI result, release, or deployment.
