# Harness Core Guide

## What Is Available

The Rust `vibemux_harness_native` crate exposes a local, bounded text-request dispatcher
and complete vendor JSON record capture. Its CLI has `catalog`, `replay`,
`probe`, and `dispatch` commands, also available as `vibemuxctl harness <command>`.
Native execution is opt-in.

Codex exec, Codex app-server, Claude stream-json, and ACP routes run on the
shared `vibemux_harness::dispatch::ProtocolSession` engine, the same engine the
daemon's ADR 029 dispatch uses. That engine owns the handshake, the approval and
permission declines, identifier correlation, and the launch arguments, so both
paths treat a vendor the same way. OpenCode, Copilot, and Grok share ACP framing
and lifecycle, not proprietary headless event schemas. Their direct JSON
protocols (`opencode_json`, `copilot_json`, `grok_stream_json`) can be replayed
but not run. The daemon allows only ACP probes; this CLI can also run an ACP
prompt on a route with `allow_execution`, and whether it should is an open
decision. [Protocol boundaries](../protocol_boundaries.md) lists each harness's
adapter support and the error codes. This captures what a managed process
exposes; it does not intercept private provider HTTP traffic or attach to an
existing desktop conversation or TUI.

## Offline First

From the repository root, with the pinned Rust 1.92.0 toolchain:

```powershell
cargo build --locked -p vibemux_harness_native --bins
cargo test --locked -p vibemux_harness_native --all-features
.\target\debug\vibemux_harness_native.exe catalog --config examples/harness/harness_config.json
.\target\debug\vibemux_harness_native.exe replay --protocol codex_app_server --input crates/vibemux_harness_native/tests/fixtures/codex_app_server.jsonl
```

Replay reads records without launching a harness or contacting a model.
Its successful exit means the stream was read, not that a task completed.
`catalog` lists every configured route, enabled or not, with its `harness`,
`protocol`, `enabled`, and `allow_execution` settings and two build facts:
`structured_probe` (the protocol has an initialize-only handshake) and
`native_execution` (a shared engine session drives the protocol). It never
includes a route's executable, working directory, arguments, environment
names, or model.
With `vibemuxctl --format text`, `catalog` adds an `Actions` column (`disabled`,
`none`, `probe`, `execute`, or `execute,probe`) and reports add a per-kind
record tally. Both are derived from the JSON report, which stays the contract.

The tests use a real local synthetic executable for all five routes. They cover
native stdin/stdout, Protobuf transport, permission denial, unknown records,
wrong IDs, malformed frames, missing terminal events, duplicate UUIDs, resource
leases, full queues, cancellation, deadlines, and forged plugin success.

## Private Configuration

Copy `examples/harness/harness_config.json` to an ignored local location such as
`.vibemux/harness_config.json`. Replace placeholder executables and working
directories locally; do not publish that copy. Relative paths resolve against
the configuration file's directory, not the caller's current directory.

`enabled: true` makes a route selectable. `allow_execution: true` separately
permits a text prompt. Leave the latter false for initialization probes.
Both are false in the public example. The configuration is closed and validated;
unknown fields and invalid limits are rejected.

Every `limits` field is optional and falls back to its default. A value outside
its range rejects the whole configuration with `harness_invalid_limits`.

| Field | Default | Range |
|---|---|---|
| `frame_bytes` | 131072 | 64 – 131072 |
| `capture_bytes` | 16777216 | `frame_bytes` – 16777216 |
| `record_count` | 16384 | 1 – 65536 |
| `queue_capacity` | 16 | 1 – 128 |
| `maximum_in_flight` | 4 | 1 – 16 |
| `request_history` | 4096 | 1 – 65536 |
| `timeout_ms` | 60000 | 100 – 600000 |
| `shutdown_ms` | 2000 | 100 – 10000 |

`frame_bytes` bounds each record's line including its line ending, so a CRLF
record has one byte less room than an LF record. `capture_bytes` bounds the
JSON text of all of a request's records, without line endings. A longer line
fails the request with `harness_frame_too_large`, and going past
`capture_bytes` or `record_count` fails it with `harness_capture_too_large`;
no record is truncated.

`arguments` is a fixed, operator-trusted launcher prefix, such as a Node script
path when no native vendor binary exists. User messages are never substituted
into it. On Windows the executable must resolve to a native `.exe`; do not point
the route at `.cmd`, `.bat`, or `.ps1`. Standard protocol arguments are supplied
by the adapter after the prefix. A prefix that carries a known approval,
sandbox, sharing, or debug-log flag, such as `--yolo` or
`--dangerously-skip-permissions`, alone or as `flag=value`, is rejected with
`harness_unsafe_arguments`. That list guards against mistakes and is not a
sandbox, so keep the prefix to launcher arguments.

Only the named `environment_names` plus the OS startup set (`SYSTEMROOT`,
`WINDIR`, `TEMP`, and `TMP`) are inherited; the vendor gets no other variable,
not even `PATH`. A startup variable is passed only when it is set and valid
Unicode. Any other named variable that is unset or not valid Unicode fails the
request with `harness_environment_unavailable` before a process starts.
Variable names must be distinct, at most 128 bytes, and use only uppercase
ASCII letters, digits, and `_`; `VIBEMUX_PLUGIN_SESSION_ID` is reserved. A bad
name is rejected with `harness_invalid_environment`. Secret values stay in the
local environment or the vendor's existing authentication mechanism, never in
this JSON. VibeMux does not copy vendor authentication files into the project.

`model: null` preserves the vendor's selection. A configured Codex or Claude
model is passed explicitly; ACP model selection is not implemented and a
non-null override is rejected rather than silently ignored. A model name must
not start with `-`, so it is never read as a flag; such a route is rejected
with `harness_invalid_route`, as the daemon rejects it. The external task
router's GPT-5.6 policy is independent of the harness wire protocol.

Changing configuration invalidates an existing dispatcher. Construct a new
dispatcher to activate the new validated snapshot. Do not edit it underneath
in-flight work.

## Probe Before Execution

```powershell
.\target\debug\vibemux_harness_native.exe probe --config .vibemux/harness_config.json --harness codex
```

Probe submits initialization only. It does not authenticate, create a vendor
session, or send a model prompt. `codex_exec` has no initialization handshake,
so probe Codex through a `codex_app_server` route. The vendor executable may perform its normal
startup and local configuration discovery. A clean `probed` result requires
the expected reply and successful owned-process shutdown. Launcher presence,
initialization, authentication, and generation are separate verification levels.

The public CLI deliberately omits raw content. Trusted Rust callers can inspect
`Outcome.records` and `Record::raw_json()` in memory. Do not serialize those raw
records into issues, public logs, telemetry, Agent Cards, or repository fixtures.

## Explicit Execution

After approving the selected vendor, working directory, model cost, and sandbox
boundary, enable execution in the private configuration. Create a new request
UUID for each attempt; the public request file is a format example, not a
reusable production identifier. The prompt must contain a non-whitespace
character and be at most 128 KiB of UTF-8; otherwise the request fails with
`harness_invalid_prompt`. A schema version other than 1 or a nil UUID fails
with `harness_invalid_request`. The daemon's `vibemuxctl dispatch submit` keeps
its own 32 KiB prompt limit. The request file is read and checked before the
configuration is opened. Its size limit fits a maximum prompt even when every
character is written as a six-byte `\u00XX` JSON escape; a larger file fails
with `harness_input_too_large`.

```powershell
.\target\debug\vibemux_harness_native.exe dispatch --config .vibemux/harness_config.json --request examples/harness/harness_request.json
```

The library accepts a cancellation watch channel. The one-shot CLI currently
has no interactive cancel command; deadline and process cleanup remain bounded.
The dispatcher does not create worktrees: supply a dedicated workspace. It also
does not authorize a harness to edit files or execute arbitrary tools.

`completed` is a protocol/process observation. It does not prove the requested
code change is correct or authorize canonical Task completion, commits, or
publication. `unverified` means no trustworthy terminal event was observed.
`failed` includes malformed input, inconsistent evidence, native failure,
deadline expiry, and cleanup failure. A terminal event stands only when the
vendor process then exits with code 0 and is not force-terminated; otherwise
the request is `failed` with `harness_process_failed`, as in the daemon. An
exchange error, such as a deadline or a protocol violation, keeps its own code.

An optional, real-model example asserts a fixed reply without printing content:

```powershell
cargo run --locked -p vibemux_harness_native --example live_codex_smoke -- .vibemux/harness_config.json .\target\debug\vibemux_harness_native_plugin.exe
```

This command sends a model request and may consume account usage. It is never
part of the offline tests. It was not authorized or executed during this slice's
validation; keeping an example is not evidence that generation works.

## Current Live Boundary

On 2026-08-27, initialization plus clean exit passed for Codex 0.149.1,
Claude Code 2.1.224, OpenCode 1.18.21, and Copilot 1.0.75. Grok 0.2.114 produced
two JSON records but failed the shutdown deadline. Its live route is not marked
ready. Do not increase a timeout merely to manufacture a pass.

No real model-generation result, Linux execution, remote A2A task, daemon
registration, or vendor tool execution is claimed. See
[the A2A implementation and learning path](a2a_implementation_learning_path.md)
for the next bounded steps.

The final [validation record](../reports/harness_validation_20260827.md) distinguishes
automated checks, native initialization, failed qualification, and authorization
limits.
