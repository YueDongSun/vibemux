# CLAUDE.md

> Claude Code entry point for the VibeMux repository. Read it together with [AGENTS.md](AGENTS.md) (engineering rules) and [PROGRESS.md](PROGRESS.md) (status and evidence) before any non-trivial change.

## What VibeMux is

VibeMux is a **Windows-first, local-first, terminal-native collaboration environment for multiple coding harnesses** (Claude Code, Codex, OpenCode, Copilot, Grok, future Gemini/Pi). The target platform is Windows 10/11 + PowerShell; WSL2/Linux is for development and the optional tmux backend. WezTerm is the first programmable terminal backend; Windows Terminal is only an entry point.

Two implementations coexist:

- A **Python 3.12 prototype** (`src/vibemux/`, installable as the `vibemux` CLI) is the current full entry point and the behavior reference for Rust parity.
- A **Rust 2024 workspace** (`crates/`) is the target authoritative core. M3 (persistence + single-writer daemon + authenticated local IPC) is `VERIFIED`; M4 plugin protocol/supervisor is `PARTIAL`; M5.0 read-only probe and unified frontend shell is `PARTIAL`; A2A is `PARTIAL — LOCAL STATEFUL`, with authenticated HTTP+JSON, JSON-RPC, and gRPC plus canonical bindings and local supervisor verification. Daemon-owned harness dispatch (ADR 029) is `PARTIAL`: fixture-verified, with live evidence on native Windows for initialize-only probes and one Claude submit that the vendor refused as not logged in, because the route's `--bare` accepted only an API key; the read-only route has since dropped `--bare` (ADR 029 amendment of 2026-10-23), and no authenticated execution turn has run yet. Dual-track coding workflows (ADR 031) are `PARTIAL — OFFLINE ONLY`: verified with fixture workers; there is no AAG gateway client, native TUI session, or live run. The decision port with advisory per-task harness routing (ADR 034) is `PARTIAL — FIXTURE ONLY`: local rules and a TypeSafe Jev peer, verified against fixture peers and a loopback test server; no live Jev call has run. Native harness request dispatch in the CLI process (ADR 040, `vibemux_harness_native`) is `PARTIAL — FIXTURE ONLY`; the unified CLI and read-only multi-pane console (ADR 041, ADR 042) are `PARTIAL`; the native MCP read host (ADR 035, `vibemux_mcp`) is `PARTIAL — PRE-ALPHA`. The read-only mobile web surface (ADR 033, `vibemux_web`) is `PARTIAL`. See [PROGRESS.md](PROGRESS.md) for the authoritative status table and remaining gates.

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
types → events → harness → workflow / decision → store / workspace / platform / a2a / probe → plugin-api → plugin-host → vibemuxd → vibemux-cli
```

Domain crates must not depend on platform, database, terminal, or network implementations. `vibemux_harness` is pure logic (no I/O); detection inputs are injected by the daemon, which reads the trusted `vibemux_probe` cache. The agent/launcher/probe-state vocabulary (`AgentKind`/`LauncherKind`/`ProbeState`) lives in `vibemux_harness` and is re-exported by `vibemux_probe`, which depends on it — the harness crate pulls no probe I/O. Full crate responsibilities and data flow are in [docs/architecture.md](docs/architecture.md).

## Layout quick map

```
src/vibemux/           # Python prototype (CLI: typer + rich; `vibemux theme` snippets)
config/theme_palettes.json         # exported GUI palettes (pinned by the palette_parity test)
crates/vibemux_types/              # IDs, state machines, frontend query types
crates/vibemux_events/             # canonical event envelopes, idempotency
crates/vibemux_harness/            # AgentKind/LauncherKind/ProbeState + harness registry/profiles/rows + canonical event drafts + dispatch protocol state machines + DSH session substrate (pure logic, no I/O)
crates/vibemux_workflow/           # dual-track workflow contracts: TaskSpec validation, renderer, gates, broker rules, leases, optimizer (pure logic, no I/O; ADR 031)
crates/vibemux_decision/           # decision port: questions, facts, answer validation, local rules, fallback resolver, config, records, peer wire (pure logic, no I/O; ADR 034)
crates/vibemux_decision_peer/      # one-shot out-of-process decision peer: the only decision-API network client (TypeSafe Jev; ADR 034)
crates/vibemux_store/              # SQLite migrations (schema 9: + harness projections, harness dispatch records, decision records, and the supervised-workflow records of ADRs 031/036-039) + repositories (bundled)
crates/vibemux_platform/           # Windows/POSIX process, IPC, and process-tree containment primitives + launch trampoline logic
crates/vibemux_a2a/                # official A2A Rust SDK adapter (loopback only)
crates/vibemux_plugin_protocol/    # M4.0 Protobuf v1 wire + manifest (+ optional terminal observation payload)
crates/vibemux_plugin_supervisor/  # M4.1 child process supervision
crates/vibemux_terminal_observer/  # out-of-process WezTerm observation plugin (list/focus native panes)
crates/vibemux_probe/              # M5.0 read-only launcher/gateway probe + trusted probe cache (depends on vibemux_harness)
crates/vibemux_dsh/                # ADR 032 DSH session driver: config trust, Node launcher pair, containment, framing, deadlines, bounded records
crates/vibemux_harness_native/     # ADR 040 bounded vendor stream adapters and CLI-process request dispatch (Codex/Claude/ACP); never canonical state or execution permission
crates/vibemux_model_peer/         # optional out-of-process model peer; never receives a core writer handle
crates/vibemux_mcp/                # ADR 035 first-party read-only MCP frontend over the daemon client (stdio)
crates/vibemux_web/                # ADR 033 operator-started read-only mobile web surface (+ the ADR 042 console's daemon views, opt-in loopback local page mode); build.rs needs crates/vibemux_web/web/dist (npm ci && npm run build)
crates/vibemux_frontend/           # egui Supervisor Chat GUI (ADR 028) + Ratatui diagnostic TUI + DSH terminal surface + ASCII dump + read-only multi-pane console with content-free daemon views (ADR 041/042)
crates/vibemux_workspace/          # Git worktree lifecycle, owner receipts, and cleanup
crates/vibemuxd/                   # M3 daemon (writer worker + control IPC v10: health/plugin/harness/frontend/terminal/dispatch/workflow/dsh/decision ops, model-supervisor and contract-edit ops + harness dispatch, workflow, DSH, and decision services)
crates/vibemux_cli/                # pre-alpha vibemuxctl lifecycle + harnesses/switch/dispatch/harness/workflow/dsh/decide/tasks/plugins/terminal/status/workspace/probe/ui client
tests/                              # Python pytest suite
scripts/                            # smoke + benchmark scripts + dual-track acceptance runner (verify_dual_track.py)
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

MSRV is **Rust 1.92.0** (pinned in `rust-toolchain.toml`). Build with the pinned toolchain to avoid surprises.

- Format check: `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Full test suite: `cargo test --workspace --all-features`
- Build the pre-alpha daemon + CLI: `cargo build -p vibemuxd --bin vibemuxd -p vibemux_cli --bin vibemuxctl` (add `--bin vibemux_launch_trampoline` for harness dispatch; it must sit next to `vibemuxd`)
- Release build for benchmarking: `cargo build --release -p vibemuxd --bin vibemuxd -p vibemux_cli --bin vibemuxctl`
- Run the read-only probe: `cargo run -p vibemux_probe --bin vibemux_probe`
- Render the frontend dashboard once (non-interactive smoke): `cargo run -p vibemux_frontend --bin vibemux_frontend_dump`
- Interactive terminal dashboard: `cargo run -p vibemux_frontend --bin vibemux_frontend_tui` (`--json`, `--theme <name>`; `j`/`k` select, `r` re-runs the read-only probe, `t` cycles themes, `c` snapshots, `q`/`Esc` exit)
- Read-only multi-pane console (ADR 041/042, also `vibemuxctl ui`): `cargo run -p vibemux_frontend --bin vibemux_dashboard -- --project-root .` (`1`-`7` views incl. daemon Tasks/Daemon/Sessions/Decisions, `L` single/split/grid, `Tab` next pane, `z` zoom, `/` filter, `r` refresh, `t` theme; reads only content-free daemon state, never starts a daemon)
- DSH terminal surface (ADR 032; needs an operator `.vibemux/dsh_session.json`): `cargo run -p vibemux_frontend --bin vibemux_dsh_tui -- --project-root .` (`--config`, `--theme`, `--print-config`; `Enter` sends, `Esc` cancels the turn, `Ctrl-F` searches the in-memory transcript, `Home`/`End` jump, `Ctrl-C` quits)
- GUI (egui/eframe, opens a window): `cargo run -p vibemux_frontend --bin vibemux_frontend`

### Daemon + harness surface (Windows, pre-alpha)

```powershell
.\target\debug\vibemuxctl.exe daemon start  --project-root .
.\target\debug\vibemuxctl.exe daemon health  --project-root .
.\target\debug\vibemuxctl.exe daemon inspect --project-root .
.\target\debug\vibemuxctl.exe daemon stop   --project-root .

# Harness orchestration (needs control protocol v3+; the daemon speaks v10 and accepts v1-v9 for older ops; daemon must be running):
.\target\debug\vibemux_probe.exe --write-cache --project-root .  # atomically writes the trusted detection cache to .vibemux\probe_cache.json (stdout report unchanged)
.\target\debug\vibemuxctl.exe harnesses --project-root .          # live refresh from the probe cache
.\target\debug\vibemuxctl.exe harnesses --cached --project-root . # persisted snapshot (no re-probe)
.\target\debug\vibemuxctl.exe switch grok --project-root .        # set project default (detection-gated)
```

`harnesses`/`switch` emit machine-readable JSON rows (`name/command/protocol/provider/available/path/roles/default`, plus `launcher`/`version`). `path` is the **resolved executable path** the probe's PATH scan actually found and used; cached (`--cached`) rows include the persisted `launcher`/`version` exactly like live rows. Only the probe's verified `--version` state counts as available; `switch` rejects undetected (`harness_not_detected`) or unknown (`store_unknown_harness`) harnesses before any state change, and a missing cache surfaces `harness_probe_cache_missing` with actionable remediation that names `vibemux_probe --write-cache --project-root <root>` and the real cache path. All harness state is written solely by `vibemuxd`'s single `WriterWorker` (schema 6).

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

Decision routing (ADR 034, pre-alpha, needs control protocol v8) is off until the operator creates `.vibemux\decision_config.json` (schema 1; example and rules in README "Decision routing") and restarts the daemon. It also needs dispatch routes, because the options are the detected harnesses with an executing route:

```powershell
cargo build -p vibemux_decision_peer --bin vibemux_decision_peer                                      # the peer must sit next to vibemuxd; needed only for --allow-external
.\target\debug\vibemuxctl.exe decide route --prompt-file prompt.txt --label rust --project-root .         # records advice; runs nothing
.\target\debug\vibemuxctl.exe decisions list --project-root .                                         # content-free views
.\target\debug\vibemuxctl.exe decisions show <decision_id> --project-root .
.\target\debug\vibemuxctl.exe dispatch submit --decision <decision_id> --prompt-file prompt.txt --project-root .  # same prompt, once, within 15 min; real prompt
```

A decision never grants execution: the decided submit re-runs every ADR 029 gate. The Jev peer runs only when the config sets `jev.enabled: true` **and** the request passes `--allow-external`; it receives enumerated facts only (option ids, operator option descriptions, allowlisted labels, a prompt size bucket, the project default), never the prompt. Do not enable Jev or pass `--allow-external` against the real API without the user's explicit authorization (paid API and key); tests use the `vibemux_decision_fixture` binary and a loopback server.

DSH sessions (ADR 032, pre-alpha, needs control protocol v7) are off until the operator creates `.vibemux\dsh_session.json` (schema 1; example and rules in [docs/dsh_session_protocol.md](docs/dsh_session_protocol.md)). Booting `dsh --profile sdk` once writes to `$DSH_HOME`, so do that under operator control first. The daemon must be running:

```powershell
.\target\debug\vibemuxctl.exe dsh open   --project-root .
.\target\debug\vibemuxctl.exe dsh prompt --prompt-file prompt.txt --project-root .   # real prompt; may use paid inference
.\target\debug\vibemuxctl.exe dsh poll   --after-sequence 0 --project-root .         # the only command that prints runtime content
.\target\debug\vibemuxctl.exe dsh status --project-root .
.\target\debug\vibemuxctl.exe dsh cancel --project-root .
.\target\debug\vibemuxctl.exe dsh close  --project-root .
```

At most one DSH session is live per project, and it lives in daemon memory: there is no canonical Task/Run record and a daemon restart loses it. Never treat `session.status: idle` as completion. Live validation against a real DSH runtime now covers an initialize-only session that reaches `ready` (see [docs/evidence/dsh_runtime_validation.md](docs/evidence/dsh_runtime_validation.md)); no turn has been sent, so prompt execution remains fixture-verified only.

Task, plugin, and terminal observation (pre-alpha, needs control protocol v4 for `tasks` and `terminal`; the daemon must be running) prints the daemon's own content-free read models and creates no task, so a task still comes from `dispatch submit` or `workflow start`. `terminal` needs a daemon that the operator started with `vibemuxd --plugin-config` for the WezTerm observer ([docs/terminal_observer_protocol.md](docs/terminal_observer_protocol.md)); it only lists, links, focuses, and unlinks a pane and never attaches, sends input, or launches a harness:

```powershell
.\target\debug\vibemuxctl.exe tasks list [--after <task_id>] [--limit <limit>] --project-root .
.\target\debug\vibemuxctl.exe tasks show <task_id> --project-root .
.\target\debug\vibemuxctl.exe plugins status --project-root .
.\target\debug\vibemuxctl.exe terminal inspect <task_id> <run_id> --project-root .
.\target\debug\vibemuxctl.exe terminal link <task_id> <run_id> --instance-id <id> --pane-id <pane_id> --project-root .
.\target\debug\vibemuxctl.exe terminal focus <binding_id> --project-root .
.\target\debug\vibemuxctl.exe terminal unlink <binding_id> --project-root .
```

To run `vibemuxctl` by name, `.\scripts\install_local_tools.ps1 -build -add_to_user_path` installs the release binaries together into `%LOCALAPPDATA%\VibeMux\bin` and appends that directory to the **user** PATH only (see the [unified CLI guide](docs/guides/unified_cli_guide.md)). The guide also maps each Python `vibemux` command to its Rust verb; `send` and standalone task creation deliberately have no Rust equivalent yet.

Dual-track workflows (ADR 031, pre-alpha, needs control protocol v6) are off until the operator creates `.vibemux\workflow_config.json` (schema 1; fields in README "Dual-track workflows") plus dispatch routes for the slots' harnesses, and restarts the daemon:

```powershell
.\target\debug\vibemuxctl.exe workflow prepare --request-file request.json --policy-file policy.json --project-root .
.\target\debug\vibemuxctl.exe workflow start --contract <sha256> --request-id <uuid> --project-root .
.\target\debug\vibemuxctl.exe workflow status|pause|cancel|purge <workflow_id> --project-root .
.\target\debug\vibemuxctl.exe workflow export <workflow_id> --out <new_dir> --project-root .
.\target\debug\vibemuxctl.exe slots list --project-root .
.\target\debug\vibemuxctl.exe context share --bundle <bundle_id> --to <session_id> --project-root .
.\target\debug\vibemuxctl.exe session inspect <session_id> [--prompts] --project-root .   # only --prompts prints prompt text
.\target\debug\vibemuxctl.exe prompt evaluate --candidate <id> --suite <id> --project-root .
.\target\debug\vibemuxctl.exe prompt versions|rollback --project-root .
```

The offline acceptance runner drives the real daemon through the integration tests and re-derives the 37 benchmark scenarios from logs and evidence digests. Python is only the test driver, and an offline run is at best `OFFLINE_PASS`:

```powershell
python scripts/verify_dual_track.py --mode offline --output <new_dir_outside_the_repo>
python scripts/verify_dual_track.py --validate <dir>\dual_track_report.json
```

Live mode (`--mode live --config <private_policy.json> --live_opt_in`) needs an explicit budget and configuration authorization from the user. It is not implemented beyond its gate.

If `inspect` reports `recoverable`, the output contains a 64-character hexadecimal SHA-256 confirmation that must be passed to a separate `daemon recover` invocation. Recovery never kills a PID and never opens or modifies the database — it only removes unchanged stale descriptor/socket/writer lock artifacts.

Release-start benchmark (script refuses a project that already has a daemon descriptor):

```powershell
.\scripts\benchmark_daemon_start.ps1 -project_root C:\path\to\test_project -sample_count 20
```

## Working conventions that bite

- **One task per branch/worktree.** Use `feat/<issue>-<short-name>`, `fix/<issue>-<short-name>`, `refactor/<issue>-<short-name>`, or `docs/<issue>-<short-name>`.
- **Never claim "tests pass" unless they were executed on the change you're handing off.** Record the exact commands and outcomes.
- **`vibemux_web` will not compile until its frontend is built.** Its `build.rs` asserts `crates/vibemux_web/web/dist`; run `npm ci && npm run build` in `crates/vibemux_web/web` (Node 22) first, or pass `--exclude vibemux_web` to workspace-wide cargo commands. CI runs the web `typecheck`/`test`/`audit`/`build` before the Rust steps.
- **Rust library crates should use `#![forbid(unsafe_code)]`.** `unsafe` is only allowed in a narrow platform module and requires an accepted ADR plus a `// SAFETY:` comment.
- **No unbounded channels, no async mutex across `.await`, no `unwrap`/`expect` on untrusted runtime input.** Blocking SQLite/Git/filesystem/process work goes through `spawn_blocking` or a dedicated worker.
- **No public Rust ABI plugin.** Plugins are out-of-process executables speaking length-delimited Protobuf over stdin/stdout (`vibemux_plugin_protocol`). WASM may be considered later for deterministic policy/transform plugins, but never as a substitute for OS-accessing harness or terminal plugins.
- **Do not describe planned behavior as implemented.** Keep plans and accepted ADRs explicit about unimplemented work. If implementation status changes, update `PROGRESS.md` in the same PR. Commands in docs must match the current CLI.
- **Secrets, prompts, full environments, and database paths are never emitted by lifecycle, recovery, or probe JSON.** Probe and frontend treat that as a hard contract. Harness dispatch events, status, and errors are content-free too; only `harness_dispatch_output` returns raw vendor records.
- **Keep `rust-analyzer` in `rust-toolchain.toml`'s `components` list, spelled exactly.** The rust-analyzer VS Code extension chooses its server by running `rustup which rust-analyzer` in each workspace folder that has a toolchain file. If that command fails — a missing or misspelled component — the extension silently falls back to its own bundled server, which targets a newer toolchain and then warns "Your workspace uses a rust-toolchain file with a toolchain too old…". Correcting the component list and reloading the window is the whole fix; do not "solve" it with `rust-analyzer.server.path`, which would put a machine path in tracked configuration. A misspelled component also makes rustup re-sync the channel on *every* cargo invocation, which leaves stale `target/` artifacts and phantom compile errors until `cargo clean`.

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
- The GUI is the Supervisor Chat workspace (ADR 028): it reads real task/run/event/artifact summaries over Control v4 and never fabricates a transcript or reply. Send to the Coordinator target stays unavailable until a continuous coordinator-chat adapter exists; for a Claude or Codex target with an executing dispatch route, Send runs one read-only ADR 029 Ask whose reply lives in daemon memory (ADR 030 amendment of 2026-10-23; one live Claude submit reached the vendor but was refused as not logged in under `--bare`, which the read-only route has since dropped; no authenticated turn has run). The ten pre-existing harnesses (Claude/Codex/OpenCode/Copilot/Grok/Qwen/iFlow/TRAE/CodeBuddy/Kimi) plus DSH appear as diagnostics and TUI dashboard rows. Nothing attaches to PTYs/ConPTYs or renders a native TUI; the optional WezTerm observer only lists and focuses an explicitly linked native pane. Treat any reference to live native-TUI attach, embedded terminals, or harness launch/input forwarding as premature. DSH is the one exception, and it is not terminal attach: `vibemux_dsh_tui` speaks DSH's own SDK JSON-RPC stdio protocol through `vibemux_dsh`, and it writes no canonical state.
- `vibemuxctl dispatch` (ADR 029) runs one-shot structured turns through the daemon; it is not a continuous chat adapter (the GUI's read-only Ask is a one-shot client of the same operations), and its live vendor evidence is initialize-only probes plus one unauthenticated Claude submit ([docs/evidence/harness_dispatch_validation.md](docs/evidence/harness_dispatch_validation.md)). Do not describe ACP execution, or concurrent or writable `vibemuxctl dispatch`, as implemented; writable worker turns in separate worktrees exist only inside ADR 031 workflows.
- Decisions (ADR 034) are advisory and fixture-verified only; no live Jev call has run. Do not describe Jev routing as live-validated or calibrated, forward prompt text or summaries to a decision backend, or let a decision bypass a dispatch gate.
- Dual-track workflows (ADR 031) have offline fixture evidence only. No AAG client, supervisor or compiler model, native TUI session, or live run exists, and a `live` workflow config cannot make a slot eligible to code. Never present fixture runs, initialize-only probes, or log panels as dual-TUI or live acceptance. Live acceptance needs the user's explicit budget and configuration authorization.
- The current A2A gateway remains **loopback-only and local-only** even though HTTP+JSON, JSON-RPC, gRPC, authentication, bounded streaming, cancellation, and artifacts are implemented within that local contract. Do not infer remote/TLS deployment, complete official ITK coverage, message history, automatic restart recovery, or unrestricted coding-harness integration.
- `vibemuxctl` is pre-alpha; it does not replace the installed Python `vibemux` command and it does not override a public database path.
