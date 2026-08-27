# Harness Core Guide

## What Is Available

The Rust `vibemux_harness` crate exposes a local, bounded text-request dispatcher
and complete vendor JSON record capture. Its CLI has `catalog`, `replay`,
`probe`, and `dispatch` commands. Native execution is opt-in.

Codex app-server, Claude stream-json, and ACP are separate protocol adapters.
OpenCode, Copilot, and Grok share ACP framing and lifecycle, not proprietary
headless event schemas. This captures what a managed process exposes; it does
not intercept private provider HTTP traffic or attach to an existing desktop
conversation or TUI.

## Offline First

From the Rust worktree, with its pinned Rust 1.85 toolchain:

```powershell
cargo build --locked -p vibemux_harness --bins
cargo test --locked -p vibemux_harness --all-features
.\target\debug\vibemux_harness.exe catalog --config examples/harness/harness_config.json
.\target\debug\vibemux_harness.exe replay --protocol codex_app_server --input crates/vibemux_harness/tests/fixtures/codex_app_server.jsonl
```

Replay reads records without launching a harness or contacting a model.
Its successful exit means the stream was read, not that a task completed.

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

`arguments` is a fixed, operator-trusted launcher prefix, such as a Node script
path when no native vendor binary exists. User messages are never substituted
into it. On Windows the executable must resolve to a native `.exe`; do not point
the route at `.cmd`, `.bat`, or `.ps1`. Standard protocol arguments are supplied
by the adapter.

Only the named `environment_names` plus a small OS startup set are inherited.
Variable names must be uppercase. Secret values stay in the local environment
or the vendor's existing authentication mechanism, never in this JSON. VibeMux
does not copy vendor authentication files into the project.

`model: null` preserves the vendor's selection. A configured Codex or Claude
model is passed explicitly; ACP model selection is not implemented and a
non-null override is rejected rather than silently ignored. The external task
router's GPT-5.6 policy is independent of the harness wire protocol.

Changing configuration invalidates an existing dispatcher. Construct a new
dispatcher to activate the new validated snapshot. Do not edit it underneath
in-flight work.

## Probe Before Execution

```powershell
.\target\debug\vibemux_harness.exe probe --config .vibemux/harness_config.json --harness codex
```

Probe submits initialization only. It does not authenticate, create a vendor
session, or send a model prompt. The vendor executable may perform its normal
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
reusable production identifier.

```powershell
.\target\debug\vibemux_harness.exe dispatch --config .vibemux/harness_config.json --request examples/harness/harness_request.json
```

The library accepts a cancellation watch channel. The one-shot CLI currently
has no interactive cancel command; deadline and process cleanup remain bounded.
The dispatcher does not create worktrees: supply a dedicated workspace. It also
does not authorize a harness to edit files or execute arbitrary tools.

`completed` is a protocol/process observation. It does not prove the requested
code change is correct or authorize canonical Task completion, commits, or
publication. `unverified` means no trustworthy terminal event was observed.
`failed` includes malformed input, inconsistent evidence, native failure,
deadline expiry, and cleanup failure.

An optional, real-model example asserts a fixed reply without printing content:

```powershell
cargo run --locked -p vibemux_harness --example live_codex_smoke -- .vibemux/harness_config.json .\target\debug\vibemux_harness_plugin.exe
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
