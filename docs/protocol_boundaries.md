# Protocol boundaries

- **Internal Event**: append-only canonical SQLite model, independent of terminal output.
- **Local control IPC**: `vibemuxd::control` implements the private pre-alpha v17 authenticated Windows named-pipe/POSIX UDS transport: v1 `health`/`shutdown`, v2 read-only `plugin_status`, v3 harness registry operations, v4 frontend queries and terminal observation, v5 harness dispatch, v6 dual-track workflow operations, v7 DSH session operations, v8 advisory decision operations, v9 model-supervisor start and status, v10 operator contract-edit successor admission, v11 persistent chat operations, v12 lifetime chat-owner operations, v13 owner-execution operations, v14 interactive approval reads, decisions and receipt recovery, v15 checked workspace metadata/directory presentation reads, v16 explicit workspace cleanup Plan/Confirm/Status/Reconcile (Windows-only effect execution; other platforms refuse it), and v17 private live command-progress reads. Each operation keeps its minimum version, and the daemon accepts v1–v17 requests. On Windows the descriptor/lock lives in a protected per-user hashed runtime and the pipe rejects remote clients; on POSIX the descriptor is `0600`. There is no TCP fallback. `vibemuxctl` covers the standalone daemon lifecycle, explicit stale metadata recovery, harness listing and switching, harness dispatch, DSH sessions, advisory decisions, and dual-track workflows; it does not directly touch SQLite and never exposes token/endpoint path.
- **PTY**: provides interactive input/output only and never infers completion/approval.
- **ACP/MCP**: no ACP or MCP gateway accepts outside agents. ACP v1 over stdio is a client-side harness adapter. The daemon (ADR 029) uses it only for initialize-only probes of OpenCode, Copilot, and Grok. The CLI-process native dispatcher (ADR 040) can also run an ACP prompt turn on a route whose operator set `allow_execution`; that path is fixture-verified only, and whether it should follow the daemon's probe-only rule is an open decision. `vibemux_mcp` (ADR 035) is a first-party, read-only stdio MCP frontend over an already-running daemon; see [native MCP read frontend](native_mcp_protocol.md).
- **Native harness CLI**: `vibemuxctl harness` (`catalog`, `replay`, `probe`, `dispatch`) runs in the CLI process through `vibemux_harness_native` (ADR 040). It drives the same `vibemux_harness::dispatch::ProtocolSession` engine as the daemon: the vendor handshake, the approval and permission declines, identifier correlation, and the hardened argv. It writes no canonical state. Its reports carry only per-record sequence, kind, and byte count, and the text view adds a per-kind tally and flags a forced termination, because a killed process's exit code is not the vendor's. Execution needs the route's `allow_execution`; a probe sends no prompt. The status is `PARTIAL — FIXTURE ONLY`. See [ADR 040](adr/040_native_harness_request_dispatch.md).
- **Harness dispatch**: `vibemuxd` drives Codex `exec --json`/`app-server` and Claude stream-json one-shot turns through pure `vibemux_harness::dispatch` state machines, launches the vendor CLI through the first-party trampoline inside a contained process tree, and records a content-free Task/Run outcome (store schema 4). It is opt-in through `.vibemux/harness_dispatch.json`, runs at most one dispatch per project with read-only protocol profiles that are not an OS sandbox, and returns raw vendor records only from the `harness_dispatch_output` operation. Live vendor validation covers initialize-only probes on native Windows ([Stage 7 evidence](evidence/harness_dispatch_validation.md)); execution turns are not yet validated live. See [ADR 029](adr/029_daemon_harness_dispatch.md).
- **Dual-track workflow**: Control v6 adds 15 `workflow_*` operations (prepare, start, status, pause, cancel, export, slots, share, session inspect/prompt/attach, policy versions/rollback/evaluate, and content purge). Control v9 adds `workflow_supervisor_start` and `workflow_supervisor_status`, which run the model supervisor driver instead of the deterministic coordinator ([ADR 037](adr/037_model_supervisor_execution.md)), and Control v10 adds `workflow_contract_edit`, which admits an operator contract-edit successor ([ADR 039](adr/039_operator_contract_edits.md)). Answers are content-free except `workflow_session_prompt`, which returns the one prompt the operator asked for; `workflow_session_attach` is refused (`workflow_native_tui_unavailable`). Worker checkpoints (`vibemux_checkpoint`) and reviewer reports (`vibemux_review`) are fenced JSON claims in the final structured message, never terminal text. Model calls go through the daemon's bounded loopback AAG transport ([ADR 036](adr/036_runtime_execution_authority.md)). It refuses every live call before submission (`gateway_live_attestation_unavailable`) until AAG exposes request-scoped route evidence, and only test builds can construct the permit its fake-server calls need, so no model call is made. Evidence is offline fixture evidence only. See [ADR 031](adr/031_dual_track_workflow.md).
- **Plugin stdio**: `vibemux_plugin_protocol` implements the private pre-alpha Protobuf v1 schema, fixed-ceiling framing, manifest/negotiation, and session lifecycle; the wire crate itself has no process dependency. Two first-party plugins speak it. The WezTerm observer `vibemux_terminal_observer` is started by the daemon's plugin registry from the operator's startup configuration (see [terminal observation](terminal_observer_protocol.md)). The native harness plugin `vibemux_harness_native_plugin` is started by the ADR 040 dispatcher, once per probe or execution, in the CLI process. Neither is live-validated: the observer has not run against a live WezTerm, and the harness plugin has run only against fixture vendors. Plugin SDKs and third-party plugins are still unimplemented.
- **Plugin supervisor**: `vibemux_plugin_supervisor` has been validated with real Windows/Linux mock children for stdout-only framing, independent bounded stderr metadata, bounded queues, deadline/heartbeat/cancel/drain/shutdown/crash; M4.2 adds a daemon-owned registry, bounded restart budgets and quarantine with native Windows mock evidence. Plugin SDKs and third-party plugins remain absent. See [registry protocol](plugin_registry_protocol.md) for ephemeral status, startup configuration and shutdown limits.
- **A2A**: the original information-share API remains available. The new authenticated local task gateway implements HTTP+JSON/JSONRPC/gRPC through official SDK boundaries, bounded task/artifact/status/cancel/subscription mapping, and a daemon-owned supervisor backend with atomic canonical bindings and independent verifier gates. Remote bind/TLS, push, complete message history and full official ITK are not claimed. See [stateful protocol](a2a_supervisor_protocol.md) and [conformance evidence](a2a_conformance_validation.md).
- **Git artifact**: worktree, diff, and commit are auditable artifacts; merge is never automatic.

## Private command tool records

Private transcript Tool records also admit the closed version-1
`vibemux_command_tool_record` body in [ADR 070](adr/070_private_command_tool_records.md).
The pure adapter requires current Codex thread/turn correlation; the actual
writer sink binds the generation/fence and durable turn/attempt before private
append. Commands, directories and output never enter ordinary metadata/events.
Display loss is separate from vendor outcome. Old opaque text stays readable;
no Control/store/native-command version changes or permissions are introduced.

[ADR 071](adr/071_private_file_change_records.md) adds the seven-field
`vibemux_file_change_tool_record` body with bounded nested changes. Command v1
is unchanged. Both kinds share one ordered ledger and admission budget; paths,
move paths and diff text remain private inert observations. No file read,
application, revert, link or affirmative approval is implied by a report.

## Native Console diagnostics

The native Main-only `probe_rerun` command uses the desktop's separate
[local-diagnostics contract](adr/069_native_console_diagnostics.md): closed
`{ project_key, request }` start/status/cancel requests and schema-1 snapshots.
Renderer input cannot select an executable, argv, environment, endpoint, model
or provider. Task children receive no diagnostic capability. App-owned receipts,
global admission and joined cleanup remain outside canonical Task/Run state.
This activates an existing command; Control v16, store schema 14 and the
32-command count remain unchanged. Console Refresh stays read-only.

## Terminal cleanup observation boundary

The optional plugin method `terminal:cleanup_inventory_v1` reuses existing
inventory payloads with complete-subtree, complete-or-error semantics. Its
default-off observer configuration, explicit manifest capability and negotiated
observation/process grants are independent requirements. Existing two-capability
configurations keep their exact-directory inventory/focus behavior. Core checks
the method's grant and plugin session before sending and after replying; no
fallback turns unsupported or incomplete observation into absence. See
[ADR 067](adr/067_terminal_cleanup_inventory.md) and the
[rollout/limits](terminal_observer_protocol.md#opt-in-cleanup-subtree-observation).
This read-only method itself adds no Control/native command. The separate
cleanup service now uses Control v16, store schema 14 and native command 32.
Worktree deletion is not exposed by this read-only boundary.

## Private cleanup persistence boundary

Store schema 14 adds inert previews, atomic cleanup claims, executor/Arm audit,
positive quiescence and immutable outcomes. Fresh confirmation alone returns a
linear execution claim; reads/replays/reconciliation return no live authority.
Definite evidence-write errors retain only their already-retired proof for safe
persistence retry. Unknown remains reserved. The daemon/client must be upgraded
together; a binary limited to schema 13 refuses the migrated database.
See the accepted store and service scope in
[ADR 066](adr/066_explicit_worktree_removal.md).

## Explicit cleanup control and native boundary

Control v16 adds one `workspace_cleanup` operation with closed Plan, Confirm,
Status and Reconcile requests. Input contains canonical scope IDs, operation IDs
and plan/version references; it accepts no path, force flag, terminal selector or
proof boolean. Status can discover the latest Run record without a prior plan ID.
Replies contain bounded IDs, phase/version, recorded branch, expiry and outcome;
OS identities, process IDs, paths and receipts remain private Rust data.

The Windows service owns accepted continuations beyond RPC/window waiters.
Fresh confirmation claims first, rechecks final physical/clean/HEAD/terminal
evidence under its exclusive resource gate, commits Arm and consumes a one-shot
permit before Go. Terminal/Directory jobs use the same bounded owner and shared
resource gates. Unknown keeps its reservation. Reconcile consumes live retired
custody or already-durable positive quiescence and performs no deletion. Other
platforms explicitly return `workspace_cleanup_platform_unsupported`.

The Main window's native `workspace_cleanup_request` is command 32. Actual Tauri
caller identity, generation and registered project are checked before/after
forwarding. Rust derives Core project identity from the full registered Task/Run;
renderer input contains only registration key and target/action IDs. Task-child
capabilities remain unchanged. Plan review and Confirm are separate explicit
actions; uncertain responses trigger status-only recovery. No automatic Confirm,
delete retry or reconciliation is provided by the UI.

## Workspace presentation boundary

Control v15 adds `workspace_read`; existing operation minima remain unchanged.
Its closed argument contains non-nil `project_id`, `task_id`, `run_id` and
`mode` (`metadata` or `directory`). The argument limit is 2048 bytes and the
operation deadline is 30 seconds. Replies echo canonical scope. Metadata
contains a complete recorded branch of at most 1024 UTF-8 bytes, or null for an
unplanned workspace; it contains no directory. Directory mode also returns a
checked absolute path of at most 4096 UTF-8 bytes and the existing logical
resource key, within the authenticated native/Core channel. Debug output
redacts paths, branch values and keys. The resource key is path-derived and
does not establish OS file-object identity.

The writer resolves the complete owner/A2A binding, refuses conflicts and checks
the requested physical Run slot. Directory inspection creates no namespace and
reloads authority after filesystem/Git I/O. Presentation admits verified, settled
Open or Finalized owners, while recovery projections and unmanaged A2A
directories remain unavailable. Both authority loads refuse active cleanup and
the old Run's Removed tombstone. Metadata-only branch copying is independent of
directory existence. The presentation read emits no canonical state changes.

The native renderer boundary uses the existing `open_link` command with exactly
one `request`, tagged `https` or `worktree`. A worktree request supplies only
registration key, TaskId, RunId and action (`open`, `copy_path`, `copy_branch`).
HTTPS supplies only a bounded URL. Actual native caller identity, directory
resolution and copied text are never renderer arguments. Replies contain only
kind, scoped IDs/action when applicable and acknowledgement. Old descriptors
refuse the new read before transport; older clients may reject a v15 descriptor.
Upgrade bundled clients and daemon together. See
[ADR 065](adr/065_native_links_and_worktree_access.md).

## Lifetime chat owner boundary

Control v12 adds outer operation `chat_owner` and response payload kind
`chat_owner`. `ControlClient::chat_owner` accepts the closed `ChatOwnerRequest`
enum. Its argument is a JSON-encoded request string within the existing
authenticated Control envelope. The daemon echoes the requested envelope
version. `chat_owner` requires v12, while legacy `chat` retains its v11 minimum.

The legacy `chat` list argument keeps its existing shape.

```json
{"operation":"list","limit":16,"archived":false}
```

The corresponding `chat_owner` list argument has its own closed schema.

```json
{"operation":"list","limit":16,"include_finalized":false}
```

Owner requests include `open`, `lookup_open`, `status`, `title`, `reconcile`
and `stop`. Open requires a non-nil operation UUID, harness selection and both
`consent_private_storage` and `consent_workspace`. Reconcile and Stop bind an
operation UUID to the chat and expected owner version. Status excludes private
paths, tokens and title content. Title is an explicit content read. See
[ADR 058](adr/058_writable_chat_ownership.md) for state and recovery semantics.

Schema 11 stores the owner binding separately from historical read-only turns.
Legacy discovery excludes owner chats before pagination. Legacy owner access
returns `chat_owner_control_required`. Existing v11 binaries reject the v12
descriptor, so clients and daemon require coordinated upgrade. This foundation
admits an empty tool set and exposes provisioning and lifecycle operations.

The current store schema is 12. Owner attempts and their record references have
separate tables and share the authoritative writer's private-body channel.
The owner-runtime service uses this ledger with the existing read-only profiles.
Control v13 adds the separate closed `chat_execution` operation and payload,
consumed by native `chat_execution_request`. Start requires execution consent and
binds an operation UUID, chat, generation and owner version; Send binds one request
UUID and prompt to that execution context. LookupStart and Submission only read
retained receipts. Current runtime is returned separately from receipt sequence
and does not prove the old process remains live. Close uses `(chat_id, generation)`
and never closes a successor or finalizes the owner. Content/Live return bounded
private text; all other DTOs exclude it and private storage/resource bindings.
See [ADR 058](adr/058_writable_chat_ownership.md#control-v13-execution-contract).
Older schema-11 daemons refuse the migrated database. Build source-matched daemon/CLI artifacts for schema-12 qualification.

An expired chat acknowledgement remains an uncertain client observation. The
chat actor owns the accepted writer completion and its dependent process/input
step; the original Start/Send binding remains authoritative. Receipt lookup does
not re-dispatch an effect. This ownership repair changes no wire shape, schema or
production acknowledgement deadline.

## Harness adapter capability

`vibemux_harness::dispatch::adapter_capability` derives, per harness, which structured adapters this build has and the strongest thing any of them can do. The dashboard, the TUI (an `Adapters:` diagnostics line where the panel has room, and `adapter=` in its copied snapshot), the GUI, and `vibemuxctl probe` text all show this one answer. It describes the build, not the operator's configuration: a route still needs dispatch configuration, detection, and `allow_execution` before anything runs.

| Harness | Structured protocols | Support |
|---|---|---|
| Codex | `codex_exec`, `codex_app_server` | `execute` |
| Claude | `claude_stream_json` | `execute` |
| DSH | `dsh_sdk_json_rpc` | `execute` |
| OpenCode, Copilot, Grok | `acp` | `probe_only` |
| Qwen, iFlow, TRAE, CodeBuddy, Kimi | none | `terminal_only` |

`probe_only` for ACP means the daemon refuses to execute an ACP turn. No verified per-vendor deny posture exists yet, and OpenCode grants edit and bash by default. `codex_exec` has no initialize-only handshake, so probing Codex needs a `codex_app_server` route. `vibemuxctl --format text harness catalog` adds a per-route `Actions` column: `disabled`, `none`, `probe`, `execute`, or `execute,probe`. It combines these build facts with the route's `enabled` and `allow_execution` settings, and it is not a readiness check.

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

A native plugin's completion keeps its `error_code` only if the code is on a fixed allowlist: the thirteen protocol codes above plus the fixed transport and internal codes the plugin's runner records once the vendor process exists, such as `harness_cancelled`, `harness_deadline_exceeded`, `harness_eof_without_terminal`, `harness_read_failed`, `harness_session_internal`, and `harness_vendor_failed`. Any other code, including a case-changed, padded, or content-bearing one, is reported as `harness_vendor_failed`, so vendor text cannot reach the report through an error code, and a runner failure is not reported as the vendor's.

A plugin answers without a completion when it refuses the request or its runner fails before the vendor process yields one. The dispatcher then keeps the plugin's code only if it is on a second fixed list: the plugin's request checks, the runner's checks before launch, and its launch, containment, and reaping failures. For example, a missing vendor executable reports `harness_invalid_launch`, and a Windows `.cmd` or `.bat` shim reports `harness_native_executable_required`. Any other code is reported as `harness_plugin_rejected`. Either way the request reports `failed` with no exit code and no forced termination, as the daemon reports a vendor process that never started, and it returns at once rather than waiting out the shutdown grace period.

The plugin's own exit is folded in the way the daemon folds the vendor's exit. A plugin that does not shut down cleanly fails a completion whose exchange had not already failed (`harness_plugin_shutdown_failed`), so the completion never stands as success, a protocol terminal, or unverified. A failure the exchange already recorded, such as a protocol violation or a timeout, stays the reported cause.

Both paths answer vendor-to-client requests the same way. A Codex approval request gets `decline`. An ACP `session/request_permission` gets outcome `cancelled`. Every Claude `control_request` gets an error `control_response`. Every other JSON-RPC request gets `-32601` without echoing its parameters. That covers ACP filesystem and terminal requests, because the client advertises neither capability. Unknown or future vendor events stay `opaque` and are kept intact; they never complete a request.

Interactive approval authority is described in
[ADR 059](adr/059_interactive_chat_approvals.md). Schema 13 implements the
challenge/decision/outbox ledger, and Control v14 now consumes existing offers
through closed metadata/content/decision/receipt operations.
The protected private writer now returns a verified evidence snapshot with the
sole fresh claim; it exposes no vendor-frame constructor. A test-only diagnostic
observer grants no capability and has no native runner.
The separate guarded fixture runtime now connects the private writer capability
to tracked stdin delivery. Its fresh single-input Claude protocol mode is
available only through the `test_helpers` factory, which pins the actual fixture
vendor/trampoline, configuration and managed worktree identity. A private
challenge carries raw tool evidence outside serialized events and snapshots;
the runtime decision carries canonical IDs, expected version and AllowOnce/Deny,
without paths or widened grants. The fixture producer remains unavailable through
production configuration or renderer opt-in. Fixture delivery is covered by the
[native Windows evidence](evidence/native_harness_m1.md#guarded-fixture-approval-runtime).
The separate deny-only diagnostic report is test-only and content-free. Exact
initialization and generated input reuse the harness machine; manual-profile
records go to the observer. Its only permission response is a fixed Deny, with
no updated input or permissions. Reports distinguish raw correlation, actual
write/flush receipts, terminal observation and process cleanup; they never mark
an installed policy or production profile qualified. The explicit ignored native
case is a diagnostic invocation, not a public Control operation or certificate.
Report v2 also separates handshake acceptance and accepted input/denial queueing
from flush receipts. Its optional initialization comparison booleans omit raw
mode, path and tool values and preserve an unchecked state. This report change
does not change Control or relax initialization validation. The installed v1
probe's `init_mismatch` remains an unqualified observation; see its
[evidence and report repair](evidence/native_harness_m1.md#installed-claude-diagnostic-and-redacted-init-checks).
The separately authorized [installed v2 observation](evidence/native_harness_m1.md#installed-claude-report-v2-observation)
isolates a permission-mode mismatch while working directory and tools match;
strict validation and the absence of production qualification are unchanged.

Diagnostic report v3 corrects only the test-helper Manual-mode comparison:
canonical SDK `default` and the existing `manual` fixture alias are accepted as
exact strings; all other modes and malformed values remain rejected. The report
shape, root identity/CWD/tool checks, correlation and fixed Deny path are retained.
This does not modify the production read-only parser or identify the discarded
earlier live value. See the [v3 contract](adr/059_interactive_chat_approvals.md#canonical-manual-mode-in-diagnostic-report-v3).

The `native_qa` developer runner is a Rust-only test launch path. It reuses the
32-command production boundary and authenticated Control v17 client/daemon,
with store schema 14. Existing read and fixture operation minima are unchanged. Root/run stop nonces are
owned QA files, not a new public IPC or renderer capability. Fixture reports
cannot qualify an installed policy or production profile. Its trusted namespace,
lifecycle and report limits are in [ADR 062](adr/062_native_gui_acceptance_fixture.md).

The developer-only `prepare_cleanup` path selects a finalized-owner fixture and
one pinned first-party empty terminal peer through normal plugin startup. Private
plan schema 2 requires an explicit scenario; legacy schema-1 plans remain History.
Root/run/stop/report schema 1 and production Control/store/command versions are
unchanged. Stop does not parse the plan or plugin layout. No cleanup confirmation
or installed-provider qualification is seeded by preparation.

The native `chat_export_request` command is a separate desktop boundary. It
accepts only the registered project key, conversation kind/format, ChatId and
expected generation/version. It composes existing authenticated private reads,
with full-session consistency and exact UTF-8 cursor checks, and introduces no
Control operation or storage migration. Native selection owns the destination;
the renderer cannot provide a path, replacement body or record. Its receipt
contains only selection metadata, counts, saved/cancelled status and `partial`.
Schema-1 JSON/text files are explicit private copies, not native resume state or
execution capabilities. [ADR 060](adr/060_native_conversation_export.md) defines
the bounded artifact, destination and compatibility contract.

The native `chat_search_request` command also composes these authenticated reads
without a daemon operation or storage migration. Query/options and a versioned
read cursor are its only search data; arbitrary paths, databases, provider
endpoints and replacement records are refused. The query stays in the client
and is an escaped literal, never SQL, a shell string or model input. Private
previews are bounded, explicitly requested and absent from normal Debug/events.
Continuation checks canonical record coverage and source versions per page;
it is not an atomic project-wide snapshot. Read-only result navigation rechecks
chat/generation/version and exact record identity without replaying execution.
[ADR 061](adr/061_native_history_search.md) defines schema-1 cursors/results and
their compatibility limits. Current builds have 32 native commands, Control v17
and store schema 14. The new private progress read has incomplete acceptance;
existing operations retain their earlier minima.

Existing chat/owner/execution shapes and minima stay v11/v12/v13; `chat_approval`
requires v14. The default adapters remain read-only. Decisions contain only the
existing canonical binding, original expected version, operation ID and human
choice. The service/runtime retain accepted work after client observer loss;
receipt recovery reads no private body and returns no delivery capability. The
private Content operation returns JSON input only in at most 8192 UTF-8 bytes,
with exact offsets and total input size, excluding the evidence envelope.
The typed desktop command is `chat_approval_request`; no generic RPC, paths,
vendor IDs or grant changes are accepted. The daemon, CLI and desktop upgrade
together for the v14 descriptor. This increment leaves schema 13 unchanged; its
earlier migration/backup rules remain in force. See the
[surface evidence](evidence/native_harness_m1.md#control-v14-and-desktop-tool-approvals).

## Native Task Terminal boundary

The existing `supervisor_action` command accepts exactly a registered project
key, TaskId, RunId and a closed terminal request. Inspect optionally carries the
last received observation UUID; Link carries that view's UUID and an offered
pane/instance pair; Focus and Unlink carry that view's UUID. The renderer cannot
select a plugin, supply a cwd or command, or submit an arbitrary Core binding ID.
The actual native window label and generation bind the request to its task.

One current observation and one cleanup receipt per window bound provenance and
lost-reply recovery. Cleanup permits explicit idempotent Unlink of the exact
reviewed lease, while uncertain Link/Focus requires a new observation. Inspect
acknowledges only a matching token in the same caller/generation/scope. Closing
or revoking the view clears these receipts without terminal effects. Replies
contain the existing path-free terminal projection and canonical scope IDs.

Control terminal minima remain v4; the current descriptor is v17, store
schema 14, and the native command count 32. Task children gain only this closed
command alongside their two existing reads. Main assets and native permissions
must be upgraded together. See [ADR 064](adr/064_task_terminal_observation.md).
