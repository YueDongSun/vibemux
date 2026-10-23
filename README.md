# VibeMux

VibeMux is a Windows-first, local-first, terminal-native collaboration environment for multiple coding harnesses. The official target is Windows 10/11 + PowerShell; WSL2/Linux is for development and the optional tmux backend. Windows Terminal is only the entry point; WezTerm is the first programmable terminal backend.

> **Current status: pre-alpha.** The Python CLI remains the behavior reference for the complete prototype workflow. Rust implements the M3 daemon/store/local-IPC scope, M4 plugin foundations and registry, a local stateful A2A gateway and bounded supervisor workflow, opt-in one-shot harness dispatch whose live vendor validation so far covers only initialize-only probes, and a dual-track coding workflow verified offline with fixture workers only. VibeMux is not yet a functional multi-harness alpha.

## Current status

| Path | Status | Implemented scope | Not implemented or integrated |
|---|---|---|---|
| Python `vibemux` | `PARTIAL — BEHAVIOR REFERENCE` | Mock workflow, Git worktrees, safe diff/stop/cleanup, SQLite event log, resource ownership/reconciliation, and compatibility fixtures | Not the future authoritative core; new orchestration, A2A, and public plugin behavior must target Rust |
| Rust core / `vibemuxd` | `VERIFIED` within M3 | Typed IDs and state machines, canonical events, SQLite store, single-writer worker, authenticated local IPC, and start/health/inspect/recover/stop | Complete Task/Run CLI parity and Python-to-Rust state migration/default cutover |
| Plugin protocol / supervisor | `PARTIAL` overall M4 | Wire foundation, shell-free supervision, daemon registry, bounded restart/quarantine and read-only status | Writable plugin control, operator recovery, real vendor CLI plugins and SDKs |
| A2A / supervisor | `PARTIAL — LOCAL STATEFUL` | Authenticated HTTP+JSON/JSONRPC/gRPC; canonical bindings, artifacts, cancellation, subscriptions, independent review/verifier gates and local model-peer workflow | Remote/TLS deployment, full official ITK, message history, automatic restart recovery and unrestricted coding-harness integration |
| Harness dispatch (ADR 029) | `PARTIAL` | Opt-in daemon-owned one-shot Codex and Claude turns, ACP initialize-only probes, contained vendor process trees, schema-4 dispatch records, Control v5, and `vibemuxctl dispatch`; verified with fixture binaries on Windows and Linux and by live initialize-only probes of five vendor CLIs on native Windows | Live execution turns, ACP execution, writable or concurrent dispatch, durable transcripts, and frontend Send |
| Native harness dispatch (ADR 040) | `PARTIAL — FIXTURE ONLY` | Standalone `vibemux_harness_native` crate and `vibemuxctl harness`: bounded Codex app-server/exec, Claude stream-json, and ACP (OpenCode/Copilot/Grok) request dispatch in the CLI process, with Windows Job Object containment, configuration SHA-256 pinning, and content-free reports; verified against fixture binaries | Daemon admission, canonical Task/Run records, and A2A binding (ADR 029 is the daemon-owned path); no model generation executed; the Grok shutdown check did not pass; no Linux qualification |
| Unified CLI and dashboard (ADR 041) | `PARTIAL` | `vibemuxctl` `status`, `workspace status`, `probe`, `ui`, and `--format auto\|text\|json`; a read-only Git workspace inventory; the `vibemux_dashboard` multi-pane console with content-free daemon views (ADR 042) | Native-TUI attach, tool approval, and starting a daemon from the UI |
| Dual-track workflow (ADR 031) | `PARTIAL — OFFLINE ONLY` | Daemon-owned `cooperate`/`compare` workflows over two owned worktrees, TaskSpec source validation, fenced leases, context broker and shares, independent review, trusted verifier and integration gates, bounded prompt-policy optimizer, schema-5 records, Control v6, `vibemuxctl workflow`, and an offline acceptance runner; verified with fixture workers on native Windows | AAG gateway client, live supervisor/compiler/workers, native TUI sessions and handoff, paused-prompt editing, and any live acceptance |
| Probe / frontend | `PARTIAL` | Read-only launcher/gateway/A2A probes, a Ratatui diagnostic dashboard, and the egui Supervisor Chat workspace over Control v4 | Real PTY/ConPTY attach and native-TUI ownership/input/focus/resize/teardown |
| Native MCP frontend (ADR 035) | `PARTIAL — PRE-ALPHA` | Rust stdio metadata tools over the existing daemon client, explicit default-denied grant, bounded projections and contained reader lifecycle; native Windows fixture coverage | Live deployment, Linux qualification and operator acceptance |

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

The Rust 2024 workspace (`0.2.0-alpha.1`, MSRV 1.92) implements platform-independent typed IDs and Task/Run state machines, a canonical event envelope, SQLite migrations/WAL and atomic state-plus-event foundations, a dedicated single-writer worker, authenticated Windows named-pipe/POSIX UDS control, standalone `vibemuxd`, the pre-alpha `vibemuxctl` daemon lifecycle, explicit stale-runtime recovery, the declared Windows per-user control-runtime boundary, authenticated loopback A2A v1 HTTP+JSON/JSONRPC/gRPC task services, a bounded supervisor workflow, read-only probe/frontend shells, the M4 plugin foundations/registry, and opt-in daemon-owned harness dispatch (ADR 029).

The TUI JSON and plain-text dump entry points retain the default loopback probe
port. An explicit `VIBEMUX_FRONTEND_PROBE_PORT` override selects an integer port
from 1 through 65535 on loopback; invalid values exit with code 4 before probing.
This frontend-only adapter lets CLI verification use an owned ephemeral endpoint
and disposable configuration instead of the operator's running service.

For MCP clients, run `vibemux_mcp` from the repository root and point it at an
existing daemon descriptor:

```text
cargo run --locked -p vibemux_mcp --bin vibemux_mcp -- --descriptor PATH --allow_metadata_read
```

The host provides `host_status`, `health`, `frontend_tasks`, `frontend_task` and
`harness_dispatch_status`. Without the explicit metadata grant, only host status
is available. Initialization and tool discovery perform no producer I/O. The host
does not start a daemon or expose task mutation, prompts, captures or worktree
paths. See [the native MCP protocol](docs/native_mcp_protocol.md) for permission
reduction, bounds, cancellation, restart behavior and compatibility. The Hub MCP
companion remains the separate multi-producer aggregation surface.

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
- The vendor starts with an empty environment plus `SYSTEMROOT`, `WINDIR`, `TEMP`, `TMP`, and the listed `environment_names` (uppercase names, at most 24, no `VIBEMUX_` prefix). Which variables a vendor CLI needs to find its login is vendor-specific. `USERPROFILE`, `APPDATA`, and `LOCALAPPDATA` were enough for the initialize handshake of the CLIs in the [Stage 7 evidence](docs/evidence/harness_dispatch_validation.md), but an initialize does not authenticate, so what a real turn needs is not yet verified.
- Optional: a `model` on a Codex or Claude route, and a top-level `limits` object (`frame_bytes`, `capture_bytes`, `record_count`, `deadline_ms`, `shutdown_grace_ms`; defaults 128 KiB, 16 MiB, 16,384 records, 600 s, and 2 s). A vendor that exits more slowly than `shutdown_grace_ms` after its last record is killed and reported `failed`: Grok 1.0.41 takes about 2.1 s, and its probe passes with `5000`.

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
- Limits: Codex and Claude execution only; at most one active dispatch per project; vendor processes run in the project root with read-only protocol profiles, which are **not** an OS sandbox; transcripts live in daemon memory and are lost on restart. If the daemon stops during a run, the attempt becomes `recovery_pending` and blocks new dispatches until you `dispatch cancel` it. Live validation covers initialize-only probes on native Windows, and no live `submit` has run; see [ADR 029](docs/adr/029_daemon_harness_dispatch.md) and the [Stage 7 evidence](docs/evidence/harness_dispatch_validation.md).

## Native harness request dispatch (pre-alpha, ADR 040)

`vibemuxctl harness` runs bounded request dispatch **in the CLI process** through the standalone `vibemux_harness_native` crate. It is separate from the daemon-owned dispatch above: it reads no `.vibemux/harness_dispatch.json`, takes no daemon admission, and writes no canonical Task/Run record or store row. Use it to replay captured vendor streams, probe a configured route, or run a bounded request from a private configuration.

| | ADR 029 `vibemuxctl dispatch` | ADR 040 `vibemuxctl harness` |
|---|---|---|
| Owner | `vibemuxd` (needs a running daemon) | the CLI process |
| Records | canonical Task/Run, schema-4 dispatch records | none; content-free JSON report |
| Routes | Codex and Claude execute; ACP is probe-only | Codex app-server/exec and Claude stream-json execute; ACP for OpenCode, Copilot, and Grok also executes when `allow_execution` is set (an open policy decision) |
| Protocol engine | shared `vibemux_harness::dispatch::ProtocolSession` | the same shared engine |
| Configuration | `.vibemux/harness_dispatch.json` | a private `--config` file, pinned by SHA-256 |

```powershell
.\target\debug\vibemuxctl.exe harness catalog --config examples/harness/harness_config.json
.\target\debug\vibemuxctl.exe harness replay --protocol codex_app_server --input crates/vibemux_harness_native/tests/fixtures/codex_app_server.jsonl
.\target\debug\vibemuxctl.exe help harness
```

Public example configurations disable execution. Real model requests need explicit local configuration and authorization and may consume account usage. Unknown vendor fields are retained; oversized, malformed, uncorrelated, and incomplete streams fail explicitly. Public output omits prompts, vendor bodies, endpoint details, and local configuration.

The last qualified native initialization checks passed for Codex, Claude, OpenCode, and Copilot; Grok's shutdown check did not pass. Do not read synthetic tests or those initialization checks as model-generation evidence. Windows Job Objects control owned process lifetime; they are not a security sandbox, and worktrees isolate concurrent file changes, not filesystem or network authority. See [ADR 040](docs/adr/040_native_harness_request_dispatch.md) and the [harness core guide](docs/guides/harness_core_guide.md).

## Decision routing (pre-alpha, ADR 034, fixture-verified only)

The daemon can recommend which harness should run a prompt and record that choice as a content-free decision. A decision is advice: it runs nothing, and `dispatch submit --decision` re-runs every dispatch gate before anything executes. Routing is off until you create `.vibemux/decision_config.json`. The daemon reads it once at startup, so restart the daemon after editing it. Example:

```json
{
  "schema_version": 1,
  "thresholds": { "harness_route": 0.9 },
  "labels": ["rust", "docs", "refactor"],
  "local_rules": [
    { "label": "rust", "harness": "claude", "priority": 10 },
    { "label": "docs", "harness": "codex", "priority": 5 }
  ],
  "option_descriptions": {
    "claude": "Multi-file refactors and Rust changes",
    "codex": "Small focused edits and documentation",
    "escalate": "None of the harnesses fits; the operator decides"
  },
  "jev": {
    "enabled": false,
    "request_model": "jev-latest",
    "pinned_model": "jev-1.13.0",
    "credential_env": "TYPESAFE_API_KEY"
  }
}
```

- The options are the harnesses with an enabled `allow_execution` route in `harness_dispatch.json` that the probe detected, plus `escalate`. Option ids are harness command names (`opencode`, not `open_code`). With no eligible harness, `decide route` fails with `decision_no_eligible_harness`; with exactly one, it picks that one locally.
- Resolution order: an accepted external answer whose confidence reaches the threshold; otherwise the highest-priority `local_rules` entry whose label the request carries; otherwise the project default from `vibemuxctl switch`, if eligible; otherwise the decision is `escalated` and you pick a harness yourself. The threshold must be in [0.5, 1.0].
- Labels are an allowlist: a request may carry only labels listed in the config.
- **External backend (TypeSafe Jev).** No call is made unless `jev.enabled` is `true` **and** the request passes `--allow-external`. The daemon then reads the API key from the environment variable named by `credential_env` (uppercase, no `VIBEMUX_` prefix, not a system variable such as `PATH` or `TEMP`), in the environment inherited from the shell that ran `daemon start` (rotating the key needs a daemon restart), and passes it only to a one-shot `vibemux_decision_peer` process, which must sit next to `vibemuxd`. The provider receives only the option ids, your `option_descriptions`, the request labels, a prompt size bucket, and the project default. It never receives the prompt or its digest. Keep descriptions and labels free of secrets. Answers are accepted only from `pinned_model`, and the confidence used is the lower of the reported value and VibeMux's own normalized-peak computation. One external call runs at a time; a request made while another is in flight records `decision_peer_busy` instead of waiting. Stopping the daemon cancels an in-flight call and records `decision_peer_cancelled`. A failed, rejected, or low-confidence answer falls back to the local order above.

```powershell
cargo build -p vibemuxd --bin vibemuxd --bin vibemux_launch_trampoline -p vibemux_cli --bin vibemuxctl -p vibemux_decision_peer --bin vibemux_decision_peer
.\target\debug\vibemuxctl.exe daemon start --project-root .
.\target\debug\vibemuxctl.exe decide route --prompt-file prompt.txt --label rust --project-root .
.\target\debug\vibemuxctl.exe decide route --prompt-file prompt.txt --label docs --allow-external --project-root .
.\target\debug\vibemuxctl.exe decisions list --project-root .
.\target\debug\vibemuxctl.exe decisions show <decision_id> --project-root .
.\target\debug\vibemuxctl.exe dispatch submit --decision <decision_id> --prompt-file prompt.txt --project-root .
```

- `decide route` reads the prompt like `dispatch submit` (a file, or stdin for `-`) and prints the decision: status (`resolved` or `escalated`), chosen harness, source (`single_option`, `external`, `local_rule`, `project_default`, or `none`), and the external evidence. Views never carry the prompt or a digest. `decisions list` pages with `--after-sequence` and `--limit`.
- `dispatch submit --decision` takes the harness from the decision; naming one as well is refused. The prompt must be the same text the decision was made for, the decision must be used within 15 minutes, the dispatch config must be unchanged, and a decision runs at most once. Repeating it with the same `--request-id` and prompt is an idempotent replay: it returns the original receipt as long as the ordinary dispatch gates still pass (the harness is detected, its route and the dispatch config are unchanged). Like any `submit`, it sends a real prompt to the vendor CLI and may use paid model inference.
- Status: verified with fixture peers and fixture harnesses only. No live Jev call has run. See [ADR 034](docs/adr/034_decision_port.md) and the [decision orchestration roadmap](docs/decision_orchestration_roadmap.md).

## Dual-track workflows (pre-alpha, ADR 031, offline only)

The daemon can run a supervised two-worker workflow on one repository.

- **`cooperate`:** each worker owns disjoint paths of one task.
- **`compare`:** two competitors implement the same task, and at most one gated result is selected.

Workers run in separate owned worktrees. Each candidate is collected by the daemon, reviewed by an independent session, verified by a trusted verifier outside the repository, and integrated into a fresh worktree. Nothing is written to your checkout, committed, merged, or pushed.

**Evidence so far is offline only.** Workers are fixture processes. No model, AAG gateway, or vendor CLI has been contacted, and a `live` config cannot make a slot eligible to code yet. See [ADR 031](docs/adr/031_dual_track_workflow.md) for the design and its limitations.

The workflow service needs `.vibemux/workflow_config.json` (schema 1; read once at daemon start) and harness dispatch routes for the slots' harnesses. Without the config, every workflow command answers `workflow_unconfigured`. The config requires:

- `evidence_class`: `fixture` or `live`;
- `git_executable` and `verifier.directory`/`verifier.executable`: absolute paths outside the project;
- `slots`: up to 8 of `{slot_id, harness, route_role}` with `route_role` `worker_a`, `worker_b`, or `reviewer`;
- `verifier.suites`: argv templates that must contain `{candidate}` and `{out}` and may use `{verifier_dir}`;
- `verifier.timeout_ms`, `heartbeat_ms`, and `candidate_limits` (`max_files`, `max_file_bytes`, `max_total_bytes`).

Optional fields are `gateway` (required for `live`, with a binding for every slot's role) and `content_retention_days` (without it no prompt or bundle text is kept). `crates/vibemuxd/tests/workflow_support/mod.rs` writes a complete fixture config, request, and policy.

```powershell
.\target\debug\vibemuxctl.exe workflow prepare --request-file request.json --policy-file policy.json --project-root .
.\target\debug\vibemuxctl.exe workflow start --contract <sha256> --request-id <uuid> --project-root .
.\target\debug\vibemuxctl.exe workflow status <workflow_id> --project-root .
.\target\debug\vibemuxctl.exe workflow pause <workflow_id> --project-root .
.\target\debug\vibemuxctl.exe workflow export <workflow_id> --out <new_dir> --project-root .
.\target\debug\vibemuxctl.exe slots list --project-root .
.\target\debug\vibemuxctl.exe context share --bundle <bundle_id> --to <session_id> --project-root .
.\target\debug\vibemuxctl.exe session inspect <session_id> --prompts --project-root .
.\target\debug\vibemuxctl.exe prompt evaluate --candidate <candidate_id> --suite <suite_id> --project-root .
.\target\debug\vibemuxctl.exe prompt versions --project-root .
.\target\debug\vibemuxctl.exe prompt rollback --project-root .
```

- **Request file.** It holds `request_text`, the compiled `task_specs`, the slot `assignments`, the `reviewer_slot`, and `integration_suites`. Prepare validates every TaskSpec against the exact request text and the operator policy. A rejected contract answers with content-free violation codes.
- **Policy file.** The trusted operator policy: `writable_roots`, `protected_paths`, `allowed_tools`, `verifier_suites`, budget caps, and loop limits.
- **Other commands:**
  - `workflow cancel` and `workflow purge` (delete retained content) take a workflow ID.
  - `session attach` is refused with `workflow_native_tui_unavailable`.
  - Only `session inspect --prompts` prints prompt text.
  - `prompt evaluate` reads `.vibemux/prompt_suites/<suite_id>.json` and `.vibemux/prompt_candidates/<candidate_id>.json`.
- **Acceptance runner.** `python scripts/verify_dual_track.py --mode offline --output <new_dir_outside_the_repo>` runs the Rust suite, pytest, and the TaskBoard Lite calibration. It then re-derives the 37 benchmark scenarios from logs and evidence digests and writes `dual_track_report.json`.
  - An offline run is at best `OFFLINE_PASS`. Live classes are `BLOCKED`.
  - `--validate <report>` re-checks a written report against its artifacts.
  - Results: [offline evidence](docs/evidence/dual_track_offline_acceptance.md).

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

## Unified CLI and read-only dashboard (ADR 041)

```powershell
cargo build --locked --workspace --bins
.\target\debug\vibemuxctl.exe --help
.\target\debug\vibemuxctl.exe --format text status
.\target\debug\vibemuxctl.exe --format text workspace status
```

These commands do not install a global command, start a daemon, or invoke a model. `status` combines the local Git inventory with an existing daemon's health when one is already running. `workspace status` disables Git filter drivers, so its change counts can differ from normal `git status`; it never fetches, merges, or deletes anything. Both commands need Git 2.36 or newer, and a partial clone needs Git 2.45 or newer (for `--no-lazy-fetch`); older Git fails with `workspace_git_too_old` rather than risk a network fetch.

Use `--format text`, `--format json`, or `--json` before the command. Auto mode prints readable text in a terminal and JSON when redirected; `help` and `version` stay text. The daemon-backed verbs (`daemon`, `harnesses`, `switch`, `dispatch`, `decide`, `decisions`, `dsh`, `workflow`, and the rest) keep their JSON contracts and their actionable `error` text in every mode; `daemon` prints text in a terminal and JSON when piped.

```powershell
.\target\debug\vibemuxctl.exe ui --report crates/vibemux_probe/tests/fixtures/probe_report_v1.json
.\target\debug\vibemuxctl.exe --format text ui --once --report crates/vibemux_probe/tests/fixtures/probe_report_v1.json --width 80 --height 24
```

`ui` is the read-only multi-pane console (also built as `vibemux_dashboard`, which takes `--project-root <path>` and defaults to the current directory). These examples use a synthetic saved report and invoke no harness. It has seven views: Overview, Harnesses, and Workspace read the probe report and the local Git inventory; Tasks (task list plus the runs, events, and artifacts of the task opened with Enter), Daemon (health, plugins, dispatch routes), Sessions (DSH session status, workflow slots), and Decisions read the running daemon's content-free state, polled every 5 seconds on a background thread. Keys: `1`-`7` pick a view, `L` cycles the single, split (two panes), and grid (two by two) layouts, `Tab` moves to the next view or pane, `z` zooms the focused pane, `j`/`k` or arrows select, Enter opens a task or harness details, `/` filters the focused list (Enter keeps, Esc clears), `r` polls the daemon now, `t` cycles the six terminal themes (`VIBEMUX_FRONTEND_THEME` picks the first), `?` shows help, and `q`/`Esc` quit. A layout that does not fit the terminal falls back (grid needs 120x32, split 90 columns). The console never starts a daemon; a daemon view says why it is empty, for example `daemon_not_running` with the `vibemuxctl daemon start` hint, "not configured", or "unsupported by this daemon". It reads no prompt, transcript, or vendor output. `--once` renders the probe and workspace views only. Native CLI TUI slots stay reserved; there is no ConPTY attachment or tool approval action. Without `--report`, the fixed read-only probe runs once. Launcher, authentication, inference, and task completion are separate evidence levels, and saved reports are validated, labeled, and projected to safe metadata. See [ADR 041](docs/adr/041_unified_cli_and_workspace_layout.md), [ADR 042](docs/adr/042_multi_pane_console_and_ui_functions.md), and the [unified CLI guide](docs/guides/unified_cli_guide.md).

## Frontend binaries, dashboard themes, and machine-readable output

The frontend crate builds five binaries:

- `vibemux_frontend` — a native Supervisor Chat workspace (ADR 028): one Coordinator home, task cards, responsive task details, and independent task windows. Run it from the project root. It reads real task/run/event/artifact summaries through Control v4, keeps the Console, Settings, and Diagnostics secondary, and preserves all five appearance palettes. Send runs a read-only Ask of Claude or Codex when the folder's daemon has an executing dispatch route for it (see below); the Coordinator target keeps a local draft, because no continuous coordinator-chat adapter exists. It never fabricates a transcript or response.
- `vibemux_frontend_tui` — the interactive Ratatui debug dashboard over the probe report. Flags: `--json`, `--theme <name>`. Keys: `j`/`k` select a harness (the footer names its launcher, authentication, inference, version, route, code, and adapter support), `r` re-runs the same read-only probe in the background (the footer shows `probing`; it sends no prompt), `t` cycles themes, `c` captures a snapshot, `q` / `Esc` exit.
- `vibemux_frontend_dump` — one-shot plain-ASCII snapshot of the same dashboard (supersedes the removed `--once` flag).
- `vibemux_dashboard` — the read-only multi-pane console behind `vibemuxctl ui` (ADR 041, ADR 042): probe, Git inventory, and content-free daemon views in single, split, or grid layouts. It uses the same six terminal themes as `vibemux_frontend_tui`, with its own keys.
- `vibemux_dsh_tui` — the DeepSeek Harness terminal surface: one live DSH SDK session with a prompt editor, a scrolling record view, and a status line. Flags: `--project-root <path>`, `--config <path>`, `--theme <name>`, `--print-config` (with `--json`). Keys: `Enter` sends, `Esc` cancels the running turn or clears the input, `Ctrl-C` / `Ctrl-Q` quit, `Up`/`Down`/`PageUp`/`PageDown` scroll, `Home`/`End` jump to the oldest and newest lines, and `Ctrl-F` searches the in-memory transcript (type the query, `Enter`/`Down` older match, `Up` newer match, `Esc` closes the search without cancelling the turn; nothing is written to disk). It is the only frontend surface that owns a runtime process, it writes no canonical state, and it never opens the core database. Setup and the wire contract are in [docs/dsh_session_protocol.md](docs/dsh_session_protocol.md) and [ADR 032](docs/adr/032_dsh_harness_adapter.md).

The GUI defaults to **Claude light**: a cream canvas, terracotta accent, serif
display type from the system's Georgia or Cambria, and a Claude Desktop style
sidebar with **New task**, Conversation, Console, a Recents list, and a
workspace menu. The sidebar collapses to an icon rail. **Settings** (`Ctrl+,`
or the sidebar) is a full page with General, Appearance, Connection, Keyboard
shortcuts, and About sections. Seven themes are available in **Settings →
Appearance** as preview cards: Claude Light (`claude_light`), Claude Dark
(`claude`), Studio, GitHub, VS Code, Nord, and Gruvbox. Appearance also sets
Density (Comfortable or Compact) and Text size (Small, Default, or Large);
General sets the start page (Conversation or Console) and the startup window
size; Connection shows the daemon connection read-only. Every theme shares one
rounded scale: 10 px buttons, 14 px cards and panels, a 20 px composer, and
pill-shaped chips. Existing saved appearance preferences are preserved. Enter
sends a read-only Ask when the composer targets a ready harness; otherwise it
keeps the draft and says why it was not sent. Shift+Enter inserts a newline.

Task windows have Overview, Activity, and Artifacts tabs; the Overview's
Linked terminal section shows the Run's terminal link with Focus, Unlink, and
Find terminals. Closing a window does not stop its task. `Ctrl+1..0` opens the ten pre-existing harnesses' details in the Console;
Escape closes overlays before returning to Coordinator. `Ctrl+K` opens a quick
switcher over the loaded tasks, the agents, and a fixed action list (New task,
Settings, Diagnostics, Keyboard shortcuts, Toggle sidebar, Light, Dark): type
to filter, Up/Down to move, Enter to open (a task opens its drawer), Esc to
close. `Ctrl+N` starts a new task, `Ctrl+B` toggles the sidebar, `Ctrl+,` opens
Settings, and `Ctrl+/` lists every shortcut. These chords work while the
composer has focus, are ignored during IME composition, and never send
anything (ADR 030 phase 2, partial; ADR 042). Task and terminal
connection states are separate. An explicitly configured WezTerm observer can
associate an existing native pane with an A2A-backed Run and focus it after
identity checks. Watch and interact with the CLI in WezTerm; embedded terminal
rendering and harness launch/input remain future work. See
[terminal observation setup and protocol](docs/terminal_observer_protocol.md).

Launch an existing build with `target/debug/vibemux_frontend.exe`, or use
`cargo run -p vibemux_frontend --bin vibemux_frontend`. On Windows the GUI
never opens a console window, in debug and release builds alike (like
`pythonw`); a startup failure or a panic is written to `frontend_errors.log`
next to the settings file (`%APPDATA%\vibemux\`).

The GUI observes one project folder at a time. **Open folder…** (in the
sidebar's Folders list, the workspace menu, the collapsed rail, or Settings →
General) takes a typed or pasted path, or the native Windows folder picker,
and the sidebar keeps the eight most recent folders. The active folder is
saved and reopened at the next launch; without one, the GUI uses the folder
it was started from. The GUI never starts the daemon by itself: when the
active folder has no running daemon, a **Start VibeMux** card appears, and
Settings → Connection offers Start or Stop. These run `vibemuxctl --json
daemon start|stop --project-root <folder>` with no console window, so
`vibemuxctl` and `vibemuxd` must sit next to the GUI executable (a workspace
build places them together). A stopped or older daemon produces an explicit
unavailable state. The existing structured supervisor recipe is not a continuous
coding-agent chat service.

**Console.** The sidebar's **Console** page shows the terminal console's views
(`vibemuxctl ui`) as cards: Overview, Harnesses, Workspace, Tasks, Daemon
(health, dispatch routes, plugins), Sessions (DSH and workflow slots), and
Decisions. Harnesses lists the probe results with **Re-probe** (read-only
`--version` checks) and, for a harness that is Ask-ready in the folder, an
**Ask** button that opens the conversation with it targeted without sending.
While the page is open the GUI polls the folder's daemon for content-free
state every 5 seconds (**Refresh** polls now) and reads the Git inventory
through `vibemuxctl --json workspace status` (cached refs, no fetch). It
never starts a daemon or a harness.

**Ask a harness (read-only).** Pick Claude or Codex in the composer's target
chip, type a prompt (at most 32 KiB), and press Enter or Send. The GUI asks
the active folder's daemon to run one [harness dispatch](#harness-dispatch-pre-alpha-adr-029)
turn with the read-only profile and shows the reply in the conversation as
it arrives; Cancel asks the daemon to stop it. One Ask runs at a time. Claude's
route gives it no tools, so it cannot open files; Codex can read
the folder under a read-only sandbox. Claude's read-only route runs without
`--bare`, so it can use the subscription login Claude Code keeps under your
user profile. It still has no tools, no MCP servers, and no user or project
settings, but it can read CLAUDE.md files into the turn's context (ADR 029,
amendment of 2026-10-23). An authenticated live Claude turn has not been
verified yet. The
first Ask in a folder asks you to confirm once, because the turn may use
your subscription or paid inference. Replies live in the daemon's memory and
are lost when it restarts; the window keeps its last 20 Asks and clears them
when you switch folders. The read-only profile is not an OS sandbox. In a
folder without `.vibemux\harness_dispatch.json`, picking Claude or Codex shows
an **Enable Claude and Codex (read-only)** card: it runs the read-only probe,
writes default read-only routes for a native `claude.exe` or `codex.exe`
found outside the folder (a script shim such as an npm `codex.cmd` is skipped
with the reason), and restarts the folder's daemon so it loads them. It never
replaces an existing config; edit that file and restart the daemon instead.
Setup needs `vibemuxd` and `vibemux_launch_trampoline` next to the GUI
(`cargo build -p vibemuxd --bin vibemuxd --bin vibemux_launch_trampoline -p vibemux_cli --bin vibemuxctl`).
The Coordinator target stays draft-only.

The screenshot-only `vibemux_frontend_preview` binary is built with
`--features gui_screenshot`; its scenes are visibly marked **DEMO DATA** and
never contact a daemon or model. Example:
`cargo run -p vibemux_frontend --features gui_screenshot --bin vibemux_frontend_preview -- chat claude_light 1440 900 1`.
Add an absolute PNG path as the last argument to capture a settled native frame.
Scenes are `chat`, `drawer`, `task`, `settings`, `appearance`, `disconnected`,
`welcome`, `collapsed`, `ask` (a finished and a running Ask), `ask_setup`
(the setup card), and `console`, `console_harnesses`, and `console_daemon`
(the Console's Overview, Harnesses, and Daemon views). An unknown scene or theme, a size or scale that is not a number,
or an extra argument exits with code 4 instead of drawing a different scene.
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
python scripts/check_public_artifacts.py

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

`vibemux_probe` prints the versioned report JSON to stdout; pass `--write-cache --project-root <dir>` to additionally persist it atomically to `<dir>/.vibemux/probe_cache.json` (the trusted cache the daemon's `vibemuxctl harnesses`/`switch` surface reads; the stdout report is unchanged). `vibemux_frontend_dump` prints the ASCII snapshot and exits. `vibemux_frontend_tui` starts the interactive Ratatui debug dashboard; press `j`/`k` to select a harness, `r` to re-run the read-only probe, `t` to cycle themes, `c` to capture a snapshot, `q` / `Esc` to exit. `vibemux_frontend` opens the Supervisor Chat GUI described above. All eleven harnesses (Claude, Codex, OpenCode, Copilot, Grok, Qwen, iFlow, TRAE, CodeBuddy, Kimi, DSH) appear as TUI dashboard rows and GUI diagnostics; real PTY/ConPTY attach is not implemented and must not be treated as usable. DSH additionally has the live `vibemux_dsh_tui` session surface, which speaks DSH's own SDK protocol rather than attaching to a terminal.

## DeepSeek Harness (DSH)

DSH is registered as the eleventh harness and is driven through its first-party
SDK protocol: newline-delimited JSON-RPC 2.0 over stdio, served by the `sdk`
profile bundle. Setup is opt-in and lives outside the repository.

### Prerequisite check

`scripts\setup_dsh_prereq.ps1` reports whether this machine can drive the DSH
runtime, and with `-Apply` provisions what is missing. The default is read-only;
mutation happens only with `-Apply`:

```powershell
.\scripts\setup_dsh_prereq.ps1                       # report only; exit 0 ready, 2 not ready
.\scripts\setup_dsh_prereq.ps1 -Json                 # one machine-readable object
.\scripts\setup_dsh_prereq.ps1 -Apply -WriteSessionConfig
.\scripts\setup_dsh_prereq.ps1 -SkipTui              # the sdk profile only
```

Required checks: a native `node` image, npm and pnpm, an npm-global
`@deepseek-ai/dsh` at or above the supported version, and the `sdk` profile the
VibeMux adapter drives. Unless `-SkipTui` is passed, the interactive `tui`
profile is required as well. `-Apply` runs only documented operator commands —
`npm install -g @deepseek-ai/dsh@<version>`, `dsh --profile sdk --help`, and
`dsh plugin --profile tui add <bundle>` — and it never deletes anything, never
edits a profile by hand, and never prints a secret or an environment dump.
`-SkipProbes` runs the structural checks without starting a child process.

The check requires the CLI version this repository's DSH adapter is verified
against; see [docs/dsh_session_protocol.md](docs/dsh_session_protocol.md) §1 for
the exact version bound and what re-verification a different release needs.

A READY result means the runtime and profiles are present. The adapter itself
has since been exercised live against `dsh` 0.2.0-rc.2: it completes the
initialize handshake and reaches `ready` (see the
[validation record](docs/evidence/dsh_runtime_validation.md)). Turn-level
behavior against a real runtime is still unverified — no prompt has been sent
to a vendor from this repository.

### VibeMux's DSH terminal surface

```powershell
cargo run -p vibemux_frontend --bin vibemux_dsh_tui -- --project-root .
```

`.vibemux\dsh_session.json` describes the launcher pair. `-WriteSessionConfig`
writes this template, filling in the two paths it resolved:

```json
{
  "schema_version": 1,
  "executable": "<absolute path of the native node image>",
  "entry_script": "<absolute path of @deepseek-ai/dsh/lib/bin.js>",
  "profile": "sdk",
  "provider": "deepseek-official",
  "model": "deepseek-v4-flash",
  "environment_names": ["DEEPSEEK_API_KEY"]
}
```

`executable` must be a native image whose file stem is `node` — on a machine
where Node is installed outside npm (for example with nvm), it is **not**
`<npm prefix>\node.exe`; resolve it with `where node`. Both paths must lie
outside the project root. See
[docs/dsh_session_protocol.md](docs/dsh_session_protocol.md) for every field.

Both paths are verified before anything starts: the executable must be a
canonical `node` image, and the entry script must be a `.js` file the installed
`@deepseek-ai/dsh` package names in its own `bin` map. Neither may live inside
the project root, so a repository cannot ship the code a session runs.

`vibemuxctl dsh …` drives the same session through the daemon over Control v7:

```powershell
.\target\debug\vibemuxctl.exe dsh open   --project-root .
.\target\debug\vibemuxctl.exe dsh prompt --prompt-file prompt.txt --project-root .
.\target\debug\vibemuxctl.exe dsh poll   --after-sequence 0 --project-root .
.\target\debug\vibemuxctl.exe dsh status --project-root .
.\target\debug\vibemuxctl.exe dsh cancel --project-root .
.\target\debug\vibemuxctl.exe dsh close  --project-root .
```

`dsh poll` is the only DSH command that prints runtime content; the others print
a content-free status projection. A prompt comes from a file or stdin, never
from argv.

A DSH session currently has no canonical Task/Run record: it lives in daemon
memory and is lost on a daemon restart. Never treat `session.status: idle` as
completion — only a correlated `turn/end` event with a recognized reason is
terminal evidence.

### Upstream interactive terminal UI

DSH's own terminal UI is a separate, optional, out-of-tree surface: the `tui`
profile. Once it is installed, an operator can run it directly in a real
terminal, where `dsh tui` is the positional shorthand for `dsh --profile tui`:

```powershell
dsh tui            # or: dsh --profile tui
```

VibeMux neither drives nor attaches to that UI: it speaks the `sdk` profile over
stdio, which is a different surface with its own contract. The prerequisite
script checks the `tui` profile only so an operator is not left with a profile
that cannot compose; pass `-SkipTui` to ignore it.

## See also

- [docs/architecture.md](docs/architecture.md) — current modules, data flow, and contracts
- [docs/platform_support.md](docs/platform_support.md) — supported platforms and validation environments
- [docs/protocol_boundaries.md](docs/protocol_boundaries.md) — wire contracts and what is/isn't supported
- [docs/adr/](docs/adr/) — accepted architecture decisions
- [AGENTS.md](AGENTS.md) — engineering rules for contributors and agents
- [docs/index.md](docs/index.md) — documentation map; [guides](docs/guides/), [branch workflow](docs/workflow/branch_workflow.md), and dated [reports](docs/reports/)
- [docs/adr/040_native_harness_request_dispatch.md](docs/adr/040_native_harness_request_dispatch.md) and [docs/adr/041_unified_cli_and_workspace_layout.md](docs/adr/041_unified_cli_and_workspace_layout.md) — standalone native dispatch and the unified CLI
- [PROGRESS.md](PROGRESS.md) — milestones, evidence, and release plan
