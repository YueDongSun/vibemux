# VibeMux

VibeMux is a Windows-first, local-first, terminal-native collaboration environment for multiple coding harnesses. The official target is Windows 10/11 + PowerShell; WSL2/Linux is for development and the optional tmux backend. Windows Terminal is only the entry point; WezTerm is the first programmable terminal backend.

> **Current status: pre-alpha.** The Python CLI remains the behavior reference for the complete prototype workflow. Rust implements the M3 daemon/store/local-IPC scope, M4 plugin foundations and registry, a local stateful A2A gateway and bounded supervisor workflow, and opt-in one-shot harness dispatch that has not yet been validated against live vendor CLIs. VibeMux is not yet a functional multi-harness alpha.

## Current status

| Path | Status | Implemented scope | Not implemented or integrated |
|---|---|---|---|
| Python `vibemux` | `PARTIAL — BEHAVIOR REFERENCE` | Mock workflow, Git worktrees, safe diff/stop/cleanup, SQLite event log, resource ownership/reconciliation, and compatibility fixtures | Not the future authoritative core; new orchestration, A2A, and public plugin behavior must target Rust |
| Rust core / `vibemuxd` | `VERIFIED` within M3 | Typed IDs and state machines, canonical events, SQLite store, single-writer worker, authenticated local IPC, and start/health/inspect/recover/stop | Complete Task/Run CLI parity and Python-to-Rust state migration/default cutover |
| Plugin protocol / supervisor | `PARTIAL` overall M4 | Wire foundation, shell-free supervision, daemon registry, bounded restart/quarantine and read-only status | Writable plugin control, operator recovery, real vendor CLI plugins and SDKs |
| A2A / supervisor | `PARTIAL — LOCAL STATEFUL` | Authenticated HTTP+JSON/JSONRPC/gRPC; canonical bindings, artifacts, cancellation, subscriptions, independent review/verifier gates and local model-peer workflow | Remote/TLS deployment, full official ITK, message history, automatic restart recovery and unrestricted coding-harness integration |
| Harness dispatch (ADR 029) | `PARTIAL` | Opt-in daemon-owned one-shot Codex and Claude turns, ACP initialize-only probes, contained vendor process trees, schema-4 dispatch records, Control v5, and `vibemuxctl dispatch`; verified with fixture binaries on Windows and Linux | Live vendor CLI validation, ACP execution, writable or concurrent dispatch, durable transcripts, and frontend Send |
| Probe / frontend | `PARTIAL` | Read-only launcher/gateway/A2A probes, a Ratatui diagnostic dashboard, and the egui Supervisor Chat workspace over Control v4 | Real PTY/ConPTY attach and native-TUI ownership/input/focus/resize/teardown |

Authoritative milestones, evidence, and release plan: [PROGRESS.md](PROGRESS.md).

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

The Rust 2024 workspace (`0.2.0-alpha.0`, MSRV 1.85) implements platform-independent typed IDs and Task/Run state machines, a canonical event envelope, SQLite migrations/WAL and atomic state-plus-event foundations, a dedicated single-writer worker, authenticated Windows named-pipe/POSIX UDS control, standalone `vibemuxd`, the pre-alpha `vibemuxctl` daemon lifecycle, explicit stale-runtime recovery, the declared Windows per-user control-runtime boundary, authenticated loopback A2A v1 HTTP+JSON/JSONRPC/gRPC task services, a bounded supervisor workflow, read-only probe/frontend shells, the M4 plugin foundations/registry, and opt-in daemon-owned harness dispatch (ADR 029).

Python `vibemux` remains the complete prototype entry point; Rust `vibemuxctl` provides daemon lifecycle, harness listing/switching, and harness dispatch commands. Complete Rust Task/Run command parity, Python-to-Rust state migration/default cutover, full Rust workspace/terminal parity, real vendor CLI plugins, plugin SDKs, remote A2A and full ITK conformance remain incomplete. Authoritative milestones and verification boundaries live in [PROGRESS.md](PROGRESS.md).

The Rust workspace also includes the isolated `vibemux_plugin_protocol` M4.0 foundation: a checked-in Protobuf v1 schema, bounded framing, a TOML manifest, permission/capability negotiation, and a handshake lifecycle. The wire crate itself does not spawn processes and cannot mutate Task/Run state or SQLite.

M4.1 `vibemux_plugin_supervisor` has been validated on Windows/Linux against a real mock child for shell-free spawn, clean environment, bounded queues, stderr cap, heartbeat/cancel/drain/shutdown, and crash containment. The isolated process-supervisor foundation is implemented. M4.2 now integrates that supervisor into a daemon-owned registry with bounded restart/quarantine and read-only status. The plugin registry still cannot mutate Task/Run state. Stateful A2A orchestration uses a separate authenticated backend and the existing single writer.

## Local A2A supervisor workflow

An explicitly configured daemon can serve authenticated local A2A tasks and coordinate separate planner, worker and reviewer model-peer processes. Canonical completion requires independent review and a local verifier, not merely a peer's terminal state. The current recipe produces bounded structured-data artifacts; it does not execute model-generated code, invoke vendor CLI tools, auto-commit, or merge branches.

Build first with `cargo build --locked --workspace -j 2`, then run from the repository root on Windows (adjust paths for `CARGO_TARGET_DIR`):

```text
./target/debug/vibemuxd.exe --project-root <project> --supervisor-config <private_config.json>
./target/debug/vibemux_supervisor.exe --config <private_config.json> --job <work_order.json>
```

The one-shot runner owns a fresh daemon and refuses an already-owned project writer. Default daemon startup launches no model peers. See the [protocol and configuration templates](docs/a2a_supervisor_protocol.md), [conformance procedure](docs/a2a_conformance_validation.md), [verified handoff](docs/m7_a2a_handoff.md), and [progress ledger](PROGRESS.md) for verified scope and limitations. Keep completed provider configurations and credentials outside version control.

## Rust daemon lifecycle (M3 verified scope)

During the migration, Rust only writes `.vibemux/vibemux_rust.sqlite3` and never opens the Python `.vibemux/vibemux.sqlite3`. `vibemuxctl` will not overwrite an installed Python `vibemux` command:

```powershell
cargo build -p vibemuxd --bin vibemuxd -p vibemux_cli --bin vibemuxctl
.\target\debug\vibemuxctl.exe daemon start --project-root .
.\target\debug\vibemuxctl.exe daemon health --project-root .
.\target\debug\vibemuxctl.exe daemon inspect --project-root .
.\target\debug\vibemuxctl.exe daemon stop --project-root .
```

`start` uses authenticated IPC health (not PID) as the ready signal. Stale descriptor/lock artifacts fail closed; the lifecycle client does not automatically remove stale metadata or terminate unknown processes.

After a daemon crashes, run `inspect` first. Only when status is `recoverable` may the returned 64-character hexadecimal SHA-256 confirmation be fed into a separate recovery command. Replace the quoted placeholder with the exact confirmation from `inspect`:

```powershell
.\target\debug\vibemuxctl.exe daemon recover --confirmation '<confirmation_from_inspect>' --project-root .
```

Recovery never kills a PID and never opens or modifies the Python/Rust database. It refuses if any artifact, PID, or confirmation changes.

Windows stores the bearer descriptor and primary cooperative writer lock under `%LOCALAPPDATA%\VibeMux\runtime\<project_hash>`; the protected root and its inheritance rules allow only the current logon user, `SYSTEM`, and Administrators. The databases and a compatibility writer lock remain project-local; the new daemon holds both writer locks to exclude legacy writers. Agents under the same logon SID are the collaboration trust domain. POSIX continues to use the project-local `.vibemux/` descriptor at mode `0600` and a randomized UDS path.

The release-start performance can be re-measured with a fixed script; the script refuses a project that already has a daemon descriptor:

```powershell
cargo build --release -p vibemuxd --bin vibemuxd -p vibemux_cli --bin vibemuxctl
.\scripts\benchmark_daemon_start.ps1 -project_root C:\path\to\test_project -sample_count 20
```

## Harness dispatch (pre-alpha, ADR 029)

A running daemon can send one prompt to an installed Codex or Claude CLI, capture its structured output, and record the outcome as a canonical Task/Run. Dispatch is off until you create `.vibemux/harness_dispatch.json`. The daemon reads it once at startup, so restart the daemon after editing it. Example (adjust the executable paths):

```json
{
  "schema_version": 1,
  "routes": [
    {
      "harness": "codex",
      "protocol": "codex_app_server",
      "executable": "C:\\Tools\\codex\\codex.exe",
      "environment_names": ["USERPROFILE", "APPDATA", "LOCALAPPDATA"],
      "enabled": true,
      "allow_execution": true
    },
    {
      "harness": "claude",
      "protocol": "claude_stream_json",
      "executable": "C:\\Tools\\claude\\claude.exe",
      "environment_names": ["USERPROFILE", "APPDATA", "LOCALAPPDATA"],
      "enabled": true,
      "allow_execution": true
    },
    {
      "harness": "open_code",
      "protocol": "acp",
      "executable": "C:\\Tools\\opencode\\opencode.exe",
      "enabled": true
    }
  ]
}
```

- `protocol` is `codex_exec` or `codex_app_server` for Codex, `claude_stream_json` for Claude, and `acp` for OpenCode (`open_code`), Copilot, and Grok. ACP routes are probe-only: `allow_execution: true` on an `acp` route is rejected. `codex_exec` supports `submit` but not `probe`.
- `executable` must be an absolute path to a `.exe` on Windows (`.cmd`, `.bat`, and PowerShell shims are rejected) that lies outside the project and whose file name is the harness command (`codex.exe`, `claude.exe`, `opencode.exe`, ...). The config accepts no extra arguments; each protocol profile fixes the argv, including its read-only posture.
- The vendor starts with an empty environment plus `SYSTEMROOT`, `WINDIR`, `TEMP`, `TMP`, and the listed `environment_names` (uppercase names, at most 24, no `VIBEMUX_` prefix). Which variables a vendor CLI needs to find its login is vendor-specific and not yet verified live.
- Optional: a `model` on a Codex or Claude route, and a top-level `limits` object (`frame_bytes`, `capture_bytes`, `record_count`, `deadline_ms`, `shutdown_grace_ms`; defaults 128 KiB, 16 MiB, 16,384 records, 600 s, and 2 s).

Build the launch trampoline with the daemon, refresh detection, then use `vibemuxctl dispatch`:

```powershell
cargo build -p vibemuxd --bin vibemuxd --bin vibemux_launch_trampoline -p vibemux_cli --bin vibemuxctl -p vibemux_probe --bin vibemux_probe
.\target\debug\vibemux_probe.exe --write-cache --project-root .
.\target\debug\vibemuxctl.exe daemon start --project-root .
.\target\debug\vibemuxctl.exe harnesses --project-root .
.\target\debug\vibemuxctl.exe dispatch catalog --project-root .
.\target\debug\vibemuxctl.exe dispatch probe claude --project-root .
.\target\debug\vibemuxctl.exe dispatch submit codex --prompt-file prompt.txt --project-root .
.\target\debug\vibemuxctl.exe dispatch status <request_id> --project-root .
.\target\debug\vibemuxctl.exe dispatch output <request_id> --project-root .
.\target\debug\vibemuxctl.exe dispatch cancel <request_id> --project-root .
```

- `probe` runs the initialize handshake only and sends no prompt. `submit` sends a real prompt to the vendor CLI, which may use paid model inference. It reads the prompt from the file, or from stdin for `-`, never from the command line; refuses a blank, non-UTF-8, or over-32-KiB prompt; and returns a receipt as soon as the attempt is durably admitted. Pass `--request-id <uuid>` to make a retry idempotent.
- `status` is content-free. `output` is the only command that prints vendor content: whole raw JSON records, plus `next_after_sequence` for a later `--after-sequence`. Every command prints one JSON object; errors print `ok: false` with an `error_code`.
- Limits: Codex and Claude execution only; at most one active dispatch per project; vendor processes run in the project root with read-only protocol profiles, which are **not** an OS sandbox; transcripts live in daemon memory and are lost on restart. If the daemon stops during a run, the attempt becomes `recovery_pending` and blocks new dispatches until you `dispatch cancel` it. No live vendor CLI has been validated yet; see [ADR 029](docs/adr/029_daemon_harness_dispatch.md).

## Mock workflow

```powershell
$task = vibemux task "validate isolation"
$run = vibemux spawn $task --harness mock --terminal-backend mock
vibemux send $run "PING" --submit
vibemux trace
vibemux stop $run
```

## Harness switching

Besides mock and opencode/claude/copilot/pi/grok/gemini, the built-in harness registry includes Chinese CLI agents:

| name | product | vendor | command |
| --- | --- | --- | --- |
| `qwen` | Qwen Code | Alibaba | `qwen` |
| `iflow` | iFlow CLI | Alibaba (iFlow) | `iflow` |
| `trae` | TRAE CLI | ByteDance | `trae` |
| `codebuddy` | CodeBuddy Code | Tencent | `codebuddy` |
| `kimi` | Kimi Code CLI | Moonshot AI | `kimi` |

`vibemux harnesses` probes every harness and persists the snapshot to `.vibemux/harnesses.json` (`--cached` reads the snapshot, `--json` prints JSON) while recording a `harness_probed` event; `vibemux switch <name>` writes the project default harness into `.vibemux/config.json` and records a `harness_switched` event. **Detection gating: both `switch` and `spawn` require the command to be detected locally; an undetected harness is rejected before any state change** — the task stays OPEN and no run row, event, worktree, or pane is created. The mock harness is always available, so offline flows are unaffected.

```powershell
vibemux harnesses
vibemux switch qwen                 # errors out if qwen is not installed locally
$task = vibemux task "Refactor login module"
$run = vibemux spawn $task          # uses the project default harness (qwen)
$run2 = vibemux spawn $task --harness iflow   # one-off override to iflow (must also be installed)
```

Run roles are persisted as well: `spawn --role worker|reviewer|orchestrator` stores the role on the Run and in `run_prepared`/`run_started` events, and accumulates it into the harness snapshot's `roles` field (visible via `vibemux harnesses`); `vibemux status` shows each run's role. Unknown roles are rejected.

## Frontend binaries, dashboard themes, and machine-readable output

The frontend crate builds three binaries:

- `vibemux_frontend` — a native Supervisor Chat workspace (ADR 028): one Coordinator home, task cards, responsive task details, and independent task windows. Run it from the project root. It reads real task/run/event/artifact summaries through Control v4, keeps Agents/Settings/Diagnostics secondary, and preserves all five appearance palettes. The composer retains a local draft but Send is unavailable until a continuous coordinator-chat adapter exists. It never fabricates a transcript or response.
- `vibemux_frontend_tui` — the interactive Ratatui debug dashboard over the probe report. Flags: `--json`, `--theme <name>`. Keys: `t` cycles themes, `c` captures a snapshot, `q` / `Esc` exit.
- `vibemux_frontend_dump` — one-shot plain-ASCII snapshot of the same dashboard (supersedes the removed `--once` flag).

The redesigned GUI defaults to **Studio**: an off-white canvas, quiet sidebar,
dark-green accent, native Windows typography, and a continuous task activity
list. Six themes are available in **Settings → Appearance**: `studio`, `github`,
`claude`, `vscode`, `nord`, and `gruvbox`. Existing saved appearance preferences
are preserved; select Studio in Settings to adopt the new light style.

Task windows have Overview, Activity, Artifacts and Terminal tabs. Closing a
window does not stop its task. `Ctrl+1..0` accesses the ten harness diagnostics;
Escape closes overlays before returning to Coordinator. Task and terminal
connection states are separate. An explicitly configured WezTerm observer can
associate an existing native pane with an A2A-backed Run and focus it after
identity checks. Watch and interact with the CLI in WezTerm; embedded terminal
rendering and harness launch/input remain future work. See
[terminal observation setup and protocol](docs/terminal_observer_protocol.md).

Launch an existing build with `target/debug/vibemux_frontend.exe`, or use
`cargo run -p vibemux_frontend --bin vibemux_frontend`. The GUI does not start
the daemon automatically. A stopped or older daemon produces an explicit
unavailable state. The existing structured supervisor recipe is not a continuous
coding-agent chat service.

The screenshot-only `vibemux_frontend_preview` binary is built with
`--features gui_screenshot`; its scenes are visibly marked **DEMO DATA** and
never contact a daemon or model. Example:
`cargo run -p vibemux_frontend --features gui_screenshot --bin vibemux_frontend_preview -- chat studio 1440 900 1`.
Add an absolute PNG path as the last argument to capture a settled native frame.
Scenes are `chat`, `drawer`, `task`, `task_terminal`, and `disconnected`.
The `task` scenes capture the actual independent native viewport.

The TUI ships six themes: `classic` (8-color default for dark terminals), `high-contrast` (bright variants for dark terminals and low-vision users), `mono` (no foreground colors at all; emphasis via bold/dim only, safe for colorless terminals, color-blind users, and any background), `light` (dark variants tuned for white or very light terminal backgrounds), and the brand themes `nord` and `gruvbox` (xterm-256 indices resolved from the brand palettes, so they render the same hues on WezTerm and tmux without a truecolor dependency). Select with `--theme <name>` or `VIBEMUX_FRONTEND_THEME`; the footer always shows the active theme, and pressing `t` in the interactive dashboard cycles themes in place. Every accent color is covered by a WCAG AA (4.5:1) contrast audit against its background, and all state information is carried by full-word text labels so color is never the only channel.

The canonical GUI palette values are exported to `config/theme_palettes.json` (pinned to the compiled-in palettes by a parity test) and consumed by the Python CLI: `VIBEMUX_THEME=<name>` restyles human-facing `vibemux` output (tables, errors, status lines), while `--json` output stays theme-independent. `vibemux theme --list` prints the available themes, and `vibemux theme --backend wezterm|tmux --theme <name>` prints a matching pane-chrome snippet (WezTerm `config.colors` lua table, or tmux `set -g` options) that you adopt yourself — VibeMux never writes or mutates your terminal configuration (see [ADR 026](docs/adr/026_multi_surface_theme_palettes.md)).

Machine-readable evidence for scripts and CI: `vibemux_frontend_tui --json` prints the versioned probe report; `vibemux_frontend_dump` prints a plain-text snapshot. Theme selection never affects either output.

Golden render fixtures per theme (80x24 and 120x32) live in `crates/vibemux_frontend/tests/fixtures/golden/`; regenerate after an intentional visual change with `cargo test -p vibemux_frontend write_golden_fixtures -- --ignored`.

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
cargo run -p vibemux_frontend --bin vibemux_frontend_dump
cargo run -p vibemux_frontend --bin vibemux_frontend_tui
cargo run -p vibemux_frontend --bin vibemux_frontend
```

`vibemux_probe` prints the versioned report JSON to stdout; pass `--write-cache --project-root <dir>` to additionally persist it atomically to `<dir>/.vibemux/probe_cache.json` (the trusted cache the daemon's `vibemuxctl harnesses`/`switch` surface reads; the stdout report is unchanged). `vibemux_frontend_dump` prints the ASCII snapshot and exits. `vibemux_frontend_tui` starts the interactive Ratatui debug dashboard; press `t` to cycle themes, `c` to capture a snapshot, `q` / `Esc` to exit. `vibemux_frontend` opens the Supervisor Chat GUI described above. All ten harnesses (Claude, Codex, OpenCode, Copilot, Grok, Qwen, iFlow, TRAE, CodeBuddy, Kimi) appear as TUI dashboard rows and GUI diagnostics; real PTY/ConPTY attach is not implemented and must not be treated as usable.

## See also

- [docs/architecture.md](docs/architecture.md) — current modules, data flow, and contracts
- [docs/platform_support.md](docs/platform_support.md) — supported platforms and validation environments
- [docs/protocol_boundaries.md](docs/protocol_boundaries.md) — wire contracts and what is/isn't supported
- [docs/adr/](docs/adr/) — accepted architecture decisions
- [AGENTS.md](AGENTS.md) — engineering rules for contributors and agents
- [PROGRESS.md](PROGRESS.md) — milestones, evidence, and release plan
