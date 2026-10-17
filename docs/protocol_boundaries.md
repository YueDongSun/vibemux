# Protocol boundaries

- **Internal Event**: append-only canonical SQLite model, independent of terminal output.
- **Local control IPC**: `vibemuxd::control` implements the private pre-alpha v10 authenticated Windows named-pipe/POSIX UDS transport: v1 `health`/`shutdown`, v2 read-only `plugin_status`, v3 harness registry operations, v4 frontend queries and terminal observation, v5 harness dispatch, v6 dual-track workflow operations, v7 DSH session operations, v8 advisory decision operations, v9 model-supervisor start and status, and v10 operator contract-edit successor admission. Each operation keeps its minimum version, and the daemon accepts v1–v10 requests. On Windows the descriptor/lock lives in a protected per-user hashed runtime and the pipe rejects remote clients; on POSIX the descriptor is `0600`. There is no TCP fallback. `vibemuxctl` covers the standalone daemon lifecycle, explicit stale metadata recovery, harness listing and switching, harness dispatch, and dual-track workflows; it does not directly touch SQLite and never exposes token/endpoint path.
- **PTY**: provides interactive input/output only and never infers completion/approval.
- **ACP/MCP**: no ACP or MCP gateway accepts outside agents. ACP v1 over stdio is a client-side harness adapter. The daemon (ADR 029) uses it only for initialize-only probes of OpenCode, Copilot, and Grok. The CLI-process native dispatcher (ADR 040) can also run an ACP prompt turn on a route whose operator set `allow_execution`; that path is fixture-verified only, and whether it should follow the daemon's probe-only rule is an open decision. `vibemux_mcp` (ADR 035) is a first-party, read-only stdio MCP frontend over an already-running daemon; see [native MCP read frontend](native_mcp_protocol.md).
- **Native harness CLI**: `vibemuxctl harness` (`catalog`, `replay`, `probe`, `dispatch`) runs in the CLI process through `vibemux_harness_native` (ADR 040). It drives the same `vibemux_harness::dispatch::ProtocolSession` engine as the daemon: the vendor handshake, the approval and permission declines, identifier correlation, and the hardened argv. It writes no canonical state. Its reports carry only per-record sequence, kind, and byte count, and the text view adds a per-kind tally and flags a forced termination, because a killed process's exit code is not the vendor's. Execution needs the route's `allow_execution`; a probe sends no prompt. The status is `PARTIAL — FIXTURE ONLY`. See [ADR 040](adr/040_native_harness_request_dispatch.md).
- **Harness dispatch**: `vibemuxd` drives Codex `exec --json`/`app-server` and Claude stream-json one-shot turns through pure `vibemux_harness::dispatch` state machines, launches the vendor CLI through the first-party trampoline inside a contained process tree, and records a content-free Task/Run outcome (store schema 4). It is opt-in through `.vibemux/harness_dispatch.json`, runs at most one dispatch per project with read-only protocol profiles that are not an OS sandbox, and returns raw vendor records only from the `harness_dispatch_output` operation. Live vendor validation covers initialize-only probes on native Windows ([Stage 7 evidence](evidence/harness_dispatch_validation.md)); execution turns are not yet validated live. See [ADR 029](adr/029_daemon_harness_dispatch.md).
- **Dual-track workflow**: Control v6 adds 15 `workflow_*` operations (prepare, start, status, pause, cancel, export, slots, share, session inspect/prompt/attach, policy versions/rollback/evaluate, and content purge). Answers are content-free except `workflow_session_prompt`, which returns the one prompt the operator asked for; `workflow_session_attach` is refused (`workflow_native_tui_unavailable`). Worker checkpoints (`vibemux_checkpoint`) and reviewer reports (`vibemux_review`) are fenced JSON claims in the final structured message, never terminal text. There is no AAG gateway client: route bindings are validated and recorded, but no model call is made. Evidence is offline fixture evidence only. See [ADR 031](adr/031_dual_track_workflow.md).
- **Plugin stdio**: `vibemux_plugin_protocol` implements the private pre-alpha Protobuf v1 schema, fixed-ceiling framing, manifest/negotiation, and session lifecycle; the wire crate itself has no process dependency. Two first-party plugins speak it. The WezTerm observer `vibemux_terminal_observer` is started by the daemon's plugin registry from the operator's startup configuration (see [terminal observation](terminal_observer_protocol.md)). The native harness plugin `vibemux_harness_native_plugin` is started by the ADR 040 dispatcher, once per probe or execution, in the CLI process. Neither is live-validated: the observer has not run against a live WezTerm, and the harness plugin has run only against fixture vendors. Plugin SDKs and third-party plugins are still unimplemented.
- **Plugin supervisor**: `vibemux_plugin_supervisor` has been validated with real Windows/Linux mock children for stdout-only framing, independent bounded stderr metadata, bounded queues, deadline/heartbeat/cancel/drain/shutdown/crash; M4.2 adds a daemon-owned registry, bounded restart budgets and quarantine with native Windows mock evidence. Plugin SDKs and third-party plugins remain absent. See [registry protocol](plugin_registry_protocol.md) for ephemeral status, startup configuration and shutdown limits.
- **A2A**: the original information-share API remains available. The new authenticated local task gateway implements HTTP+JSON/JSONRPC/gRPC through official SDK boundaries, bounded task/artifact/status/cancel/subscription mapping, and a daemon-owned supervisor backend with atomic canonical bindings and independent verifier gates. Remote bind/TLS, push, complete message history and full official ITK are not claimed. See [stateful protocol](a2a_supervisor_protocol.md) and [conformance evidence](a2a_conformance_validation.md).
- **Git artifact**: worktree, diff, and commit are auditable artifacts; merge is never automatic.

## Harness adapter capability

`vibemux_harness::dispatch::adapter_capability` derives, per harness, which structured adapters this build has and the strongest thing any of them can do. The dashboard, the TUI (an `Adapters:` diagnostics line where the panel has room, and `adapter=` in its copied snapshot), the GUI, and `vibemuxctl probe` text all show this one answer. It describes the build, not the operator's configuration: a route still needs dispatch configuration, detection, and `allow_execution` before anything runs.

| Harness | Structured protocols | Support |
|---|---|---|
| Codex | `codex_exec`, `codex_app_server` | `execute` |
| Claude | `claude_stream_json` | `execute` |
| DSH | `dsh_sdk_json_rpc` | `execute` |
| OpenCode, Copilot, Grok | `acp` | `probe_only` |
| Qwen, iFlow, TRAE, CodeBuddy, Kimi | none | `terminal_only` |

`probe_only` for ACP means the daemon refuses to execute an ACP turn. No verified per-vendor deny posture exists yet, and OpenCode grants edit and bash by default. `codex_exec` has no initialize-only handshake, so probing Codex needs a `codex_app_server` route. `vibemuxctl --format text harness catalog` adds a per-route `Actions` column: `disabled`, `probe`, `execute`, or `none`. It combines these build facts with the route's `enabled` and `allow_execution` settings, and it is not a readiness check.

## Harness protocol error codes

Both dispatch paths detect protocol violations in the shared engine (`DispatchError`), so they reject the same vendor records. Each path reports its own fixed, content-free code.

| Violation | Daemon dispatch (ADR 029) | Native CLI (ADR 040) |
|---|---|---|
| Malformed JSON-RPC envelope | `harness_protocol_invalid_jsonrpc` | `harness_invalid_jsonrpc` |
| Request or response id that cannot be echoed | `harness_protocol_invalid_rpc_id` | `harness_invalid_rpc_id` |
| Missing or non-object RPC result | `harness_protocol_invalid_rpc_result` | `harness_invalid_rpc_result` |
| Error reply to one of the client's requests | `harness_protocol_rpc_error` | `harness_rpc_error` |
| Missing or malformed session, thread, or turn identifier | `harness_protocol_invalid_identifier` | `harness_invalid_identifier` |
| Invalid initialize reply | `harness_protocol_invalid_initialize` | `harness_invalid_initialize` |
| Initialize reply names an unsupported protocol version (ACP requires 1) | `harness_protocol_unsupported_version` | `harness_unsupported_protocol_version` |
| Response whose id does not match the pending request | `harness_protocol_uncorrelated_response` | `harness_uncorrelated_response` |
| Response after the session finished or in the wrong phase | `harness_protocol_unexpected_response` | `harness_unexpected_response` |
| Event for another session | `harness_protocol_uncorrelated_session` | `harness_uncorrelated_session` |
| Event for another turn | `harness_protocol_uncorrelated_turn` | `harness_uncorrelated_turn` |
| Terminal record outside the prompt phase | `harness_protocol_unexpected_terminal` | `harness_unexpected_terminal` |
| Prompt or turn end that resolves to no terminal status | `harness_protocol_invalid_terminal` | `harness_invalid_terminal` |

The native CLI also maps `InvalidPrompt` to `harness_invalid_prompt`, `WorkingDirectoryInvalid` to `harness_invalid_cwd`, `ProbeUnsupported` to `harness_native_execution_unsupported`, and `ConfigRouteInvalid` to `harness_unsupported_protocol`. Any other engine error becomes `harness_session_internal`.

A native plugin's completion keeps its `error_code` only if the code is on a fixed allowlist: the thirteen protocol codes above plus the dispatcher's own transport codes, such as `harness_cancelled`, `harness_deadline_exceeded`, `harness_eof_without_terminal`, and `harness_vendor_failed`. Any other code, including a case-changed, padded, or content-bearing one, is reported as `harness_vendor_failed`, so vendor text cannot reach the report through an error code.

Both paths answer vendor-to-client requests the same way. A Codex approval request gets `decline`. An ACP `session/request_permission` gets outcome `cancelled`. Every Claude `control_request` gets an error `control_response`. Every other JSON-RPC request gets `-32601` without echoing its parameters. That covers ACP filesystem and terminal requests, because the client advertises neither capability. Unknown or future vendor events stay `opaque` and are kept intact; they never complete a request.
