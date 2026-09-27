# CLAUDE.md

> Claude Code entry point for the VibeMux repository. Read it together with [AGENTS.md](AGENTS.md) (engineering rules) and [PROGRESS.md](PROGRESS.md) (status and evidence) before any non-trivial change.

## What VibeMux is

VibeMux is a **Windows-first, local-first, terminal-native collaboration environment for multiple coding harnesses** (Claude Code, Codex, OpenCode, Copilot, Grok, future Gemini/Pi). The target platform is Windows 10/11 + PowerShell; WSL2/Linux is for development and the optional tmux backend. WezTerm is the first programmable terminal backend; Windows Terminal is only an entry point.

Two implementations coexist:

- A **Python 3.12 prototype** (`src/vibemux/`, installable as the `vibemux` CLI) is the current full entry point and the behavior reference for Rust parity.
- A **Rust 2024 workspace** (`crates/`) is the target authoritative core. M3 (persistence + single-writer daemon + authenticated local IPC) is `VERIFIED`; M4 plugin protocol/supervisor is `PARTIAL`; M5.0 read-only probe and unified frontend shell is `PARTIAL`; A2A is `PARTIAL — LOCAL STATEFUL`, with authenticated HTTP+JSON, JSON-RPC, and gRPC plus canonical bindings and local supervisor verification. Daemon-owned harness dispatch (ADR 029) is `PARTIAL`: fixture-verified, with no live vendor CLI evidence yet. See [PROGRESS.md](PROGRESS.md) for the authoritative status table and remaining gates.

`AGENTS.md` is the engineering-rules source of truth; `PROGRESS.md` is the implementation-status source of truth. Read both before any non-trivial change.

## Core architecture (read this once)

The canonical domain model is `Task / Run / Event` — terminal-independent. Five orthogonal abstractions own distinct concerns:

| Abstraction | Owns | Does NOT do |
|---|---|---|
| `TerminalBackend` | pane lifecycle (WezTerm / tmux / mock) | decide Task success |
| `ExecutionBackend` | runtime that executes commands | own canonical state |
| `HarnessAdapter` | controlled argv / launch spec | execute shell strings |
| `WorkspaceManager` | Git worktree + branch per Run | merge, push, or clean automatically |
| `Run orchestrator` | spawn → terminal → harness | infer completion from PTY text |

Key invariants from `AGENTS.md` §2: user messages are data (never interpolated into shell strings), events are append-only, the canonical DB has exactly one authoritative writer (`vibemuxd` after cutover), worktrees are concurrency isolation (not a sandbox), and `RunStatus.SUCCEEDED` requires structured-adapter, verifier, or explicit user authority.

Rust crate dependency direction (from `AGENTS.md` §4.2):

```text
types → events → harness → store / workspace / platform / a2a / probe → plugin-api → plugin-host → vibemuxd → vibemux-cli
```

Domain crates must not depend on platform, database, terminal, or network implementations. `vibemux_harness` is pure logic (no I/O); detection inputs are injected by the daemon, which reads the trusted `vibemux_probe` cache. The agent/launcher/probe-state vocabulary (`AgentKind`/`LauncherKind`/`ProbeState`) lives in `vibemux_harness` and is re-exported by `vibemux_probe`, which depends on it — the harness crate pulls no probe I/O. Full crate responsibilities and data flow are in [docs/architecture.md](docs/architecture.md).

## Layout quick map

```
src/vibemux/           # Python prototype (CLI: typer + rich; `vibemux theme` snippets)
config/theme_palettes.json         # exported GUI palettes (pinned by the palette_parity test)
crates/vibemux_types/              # IDs, state machines, frontend query types
crates/vibemux_events/             # canonical event envelopes, idempotency
crates/vibemux_harness/            # AgentKind/LauncherKind/ProbeState + harness registry/profiles/rows + canonical event drafts + dispatch protocol state machines (pure logic, no I/O)
crates/vibemux_store/              # SQLite migrations (schema 4: + harness projections, harness dispatch records) + repositories (bundled)
crates/vibemux_platform/           # Windows/POSIX process, IPC, and process-tree containment primitives + launch trampoline logic
crates/vibemux_a2a/                # official A2A Rust SDK adapter (loopback only)
crates/vibemux_plugin_protocol/    # M4.0 Protobuf v1 wire + manifest (+ optional terminal observation payload)
crates/vibemux_plugin_supervisor/  # M4.1 child process supervision
crates/vibemux_terminal_observer/  # out-of-process WezTerm observation plugin (list/focus native panes)
crates/vibemux_probe/              # M5.0 read-only launcher/gateway probe + trusted probe cache (depends on vibemux_harness)
crates/vibemux_frontend/           # egui Supervisor Chat GUI (ADR 028) + Ratatui diagnostic TUI + ASCII dump
crates/vibemuxd/                   # M3 daemon (writer worker + control IPC v5: health/plugin/harness/frontend/terminal/dispatch ops + harness dispatch service)
crates/vibemux_cli/                # pre-alpha vibemuxctl lifecycle + harnesses/switch/dispatch client
tests/                              # Python pytest suite
scripts/                            # smoke + benchmark scripts
docs/adr/                           # architecture decision records
```

The checked-in plugin schema is `crates/vibemux_plugin_protocol/proto/vibemux_plugin_v1.proto`.

## Common commands

All commands run from the repo root. PowerShell is the documented shell on Windows.

### Python prototype

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install -e ".[dev]"
vibemux --version
vibemux init --terminal-backend mock
vibemux doctor
```

- Run all tests: `python -m pytest`
- Lint: `python -m ruff check .`
- Format check: `python -m ruff format --check .`
- Type check: `python -m mypy .`
- Mock-workflow smoke: `python scripts/smoke_test.py`

### Rust workspace

MSRV is **Rust 1.85.0** (pinned in `rust-toolchain.toml`). Build with the pinned toolchain to avoid surprises.

- Format check: `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Full test suite: `cargo test --workspace --all-features`
- Build the pre-alpha daemon + CLI: `cargo build -p vibemuxd --bin vibemuxd -p vibemux_cli --bin vibemuxctl` (add `--bin vibemux_launch_trampoline` for harness dispatch; it must sit next to `vibemuxd`)
- Release build for benchmarking: `cargo build --release -p vibemuxd --bin vibemuxd -p vibemux_cli --bin vibemuxctl`
- Run the read-only probe: `cargo run -p vibemux_probe --bin vibemux_probe`
- Render the frontend dashboard once (non-interactive smoke): `cargo run -p vibemux_frontend --bin vibemux_frontend_dump`
- Interactive terminal dashboard: `cargo run -p vibemux_frontend --bin vibemux_frontend_tui` (`--json`, `--theme <name>`; `t` cycles themes, `c` snapshots, `q`/`Esc` exit)
- GUI (egui/eframe, opens a window): `cargo run -p vibemux_frontend --bin vibemux_frontend`

### Daemon + harness surface (Windows, pre-alpha)

```powershell
.\target\debug\vibemuxctl.exe daemon start  --project-root .
.\target\debug\vibemuxctl.exe daemon health  --project-root .
.\target\debug\vibemuxctl.exe daemon inspect --project-root .
.\target\debug\vibemuxctl.exe daemon stop   --project-root .

# Harness orchestration (needs control protocol v3+; the daemon speaks v5 and accepts v1-v4 for older ops; daemon must be running):
.\target\debug\vibemux_probe.exe --write-cache --project-root .  # atomically writes the trusted detection cache to .vibemux\probe_cache.json (stdout report unchanged)
.\target\debug\vibemuxctl.exe harnesses --project-root .          # live refresh from the probe cache
.\target\debug\vibemuxctl.exe harnesses --cached --project-root . # persisted snapshot (no re-probe)
.\target\debug\vibemuxctl.exe switch grok --project-root .        # set project default (detection-gated)
```

`harnesses`/`switch` emit machine-readable JSON rows (`name/command/protocol/provider/available/path/roles/default`, plus `launcher`/`version`). `path` is the **resolved executable path** the probe's PATH scan actually found and used; cached (`--cached`) rows include the persisted `launcher`/`version` exactly like live rows. Only the probe's verified `--version` state counts as available; `switch` rejects undetected (`harness_not_detected`) or unknown (`store_unknown_harness`) harnesses before any state change, and a missing cache surfaces `harness_probe_cache_missing` with actionable remediation that names `vibemux_probe --write-cache --project-root <root>` and the real cache path. All harness state is written solely by `vibemuxd`'s single `WriterWorker` (schema 4).

Harness dispatch (ADR 029, pre-alpha, needs control protocol v5) is off until the operator creates `.vibemux\harness_dispatch.json` (schema 1; example and rules in README "Harness dispatch") and restarts the daemon:

```powershell
.\target\debug\vibemuxctl.exe dispatch catalog --project-root .                                  # configured routes, no paths
.\target\debug\vibemuxctl.exe dispatch probe claude --project-root .                             # initialize-only; sends no prompt
.\target\debug\vibemuxctl.exe dispatch submit codex --prompt-file prompt.txt --project-root .    # real prompt; may use paid inference
.\target\debug\vibemuxctl.exe dispatch status <request_id> --project-root .                      # content-free record
.\target\debug\vibemuxctl.exe dispatch output <request_id> --project-root .                      # the only command that prints vendor content
.\target\debug\vibemuxctl.exe dispatch cancel <request_id> --project-root .
```

Codex and Claude routes execute; ACP routes (OpenCode/Copilot/Grok) are probe-only. At most one dispatch is active per project, the read-only protocol profiles are not an OS sandbox, and transcripts live in daemon memory only. Do not run `probe` or `submit` against real vendor CLIs without the user's explicit authorization (`submit` sends a real prompt); tests use the `vibemux_native_fixture` binary.

If `inspect` reports `recoverable`, the output contains a 64-character hexadecimal SHA-256 confirmation that must be passed to a separate `daemon recover` invocation. Recovery never kills a PID and never opens or modifies the database — it only removes unchanged stale descriptor/socket/writer lock artifacts.

Release-start benchmark (script refuses a project that already has a daemon descriptor):

```powershell
.\scripts\benchmark_daemon_start.ps1 -project_root C:\path\to\test_project -sample_count 20
```

## Working conventions that bite

- **One task per branch/worktree.** Use `feat/<issue>-<short-name>`, `fix/<issue>-<short-name>`, `refactor/<issue>-<short-name>`, or `docs/<issue>-<short-name>`.
- **Never claim "tests pass" unless they were executed on the change you're handing off.** Record the exact commands and outcomes.
- **Rust library crates should use `#![forbid(unsafe_code)]`.** `unsafe` is only allowed in a narrow platform module and requires an accepted ADR plus a `// SAFETY:` comment.
- **No unbounded channels, no async mutex across `.await`, no `unwrap`/`expect` on untrusted runtime input.** Blocking SQLite/Git/filesystem/process work goes through `spawn_blocking` or a dedicated worker.
- **No public Rust ABI plugin.** Plugins are out-of-process executables speaking length-delimited Protobuf over stdin/stdout (`vibemux_plugin_protocol`). WASM may be considered later for deterministic policy/transform plugins, but never as a substitute for OS-accessing harness or terminal plugins.
- **Do not describe planned behavior as implemented.** Keep plans and accepted ADRs explicit about unimplemented work. If implementation status changes, update `PROGRESS.md` in the same PR. Commands in docs must match the current CLI.
- **Secrets, prompts, full environments, and database paths are never emitted by lifecycle, recovery, or probe JSON.** Probe and frontend treat that as a hard contract. Harness dispatch events, status, and errors are content-free too; only `harness_dispatch_output` returns raw vendor records.

## Targeted cross-references

- Engineering rules (must/must-not, lifecycle gates, security, testing layers): [AGENTS.md](AGENTS.md).
- Milestone status, evidence, and dated progress entries: [PROGRESS.md](PROGRESS.md).
- Modules, data flow, contracts: [docs/architecture.md](docs/architecture.md).
- Architecture diagram + crate boundaries: [PROGRESS.md §3](PROGRESS.md) and [docs/architecture.md](docs/architecture.md).
- Platform claims + live-validation environments: [docs/platform_support.md](docs/platform_support.md).
- Wire contracts and what is/isn't supported: [docs/protocol_boundaries.md](docs/protocol_boundaries.md).
- ADRs (architecture decisions): [docs/adr/](docs/adr/).
- CLI smoke recipe: [README.md](README.md) "Mock workflow".

## Forbidden Git actions

Unless explicitly authorized for a known safe resource, do not run: `git reset --hard`, `git clean -fd`/`-fdx`, `git checkout -- .`, `git restore .`, force push, deleting unknown branches, rewriting published history, deleting worktrees outside a validated cleanup plan.

## Where to be careful

- `99d1f8e` is only the **original Python audit baseline**; the current Python package remains the behavior reference while new orchestration or A2A work targets Rust. Do not let the Python prototype grow a second authoritative state writer after cutover.
- During migration, Rust only writes `.vibemux/vibemux_rust.sqlite3`; it never opens `.vibemux/vibemux.sqlite3`. Path code refuses symlinks/hardlinks that alias the Python database.
- The GUI is the Supervisor Chat workspace (ADR 028): it reads real task/run/event/artifact summaries over Control v4 and never fabricates a transcript or reply; Send stays unavailable until a continuous coordinator-chat adapter exists. The ten harnesses (Claude/Codex/OpenCode/Copilot/Grok/Qwen/iFlow/TRAE/CodeBuddy/Kimi) appear as diagnostics and TUI dashboard rows. Nothing attaches to PTYs/ConPTYs or renders a native TUI; the optional WezTerm observer only lists and focuses an explicitly linked native pane. Treat any reference to live native-TUI attach, embedded terminals, or harness launch/input forwarding as premature.
- `vibemuxctl dispatch` (ADR 029) runs one-shot structured turns through the daemon; it is not a continuous chat adapter, does not enable the GUI Send action, and has no live vendor CLI evidence yet. Do not describe ACP execution, concurrent dispatch, or writable dispatch as implemented.
- The current A2A gateway remains **loopback-only and local-only** even though HTTP+JSON, JSON-RPC, gRPC, authentication, bounded streaming, cancellation, and artifacts are implemented within that local contract. Do not infer remote/TLS deployment, complete official ITK coverage, message history, automatic restart recovery, or unrestricted coding-harness integration.
- `vibemuxctl` is pre-alpha; it does not replace the installed Python `vibemux` command and it does not override a public database path.
