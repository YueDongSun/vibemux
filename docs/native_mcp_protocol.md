# Native MCP read frontend

`vibemux_mcp` is an additive pre-alpha first-party frontend for an already-running
VibeMux daemon. It serves VibeMux metadata only. The Hub MCP companion continues
to aggregate its separate producers.

## Launch and permission

From the repository root, build with Rust 1.92 and launch over stdio:

```text
cargo run --locked -p vibemux_mcp --bin vibemux_mcp -- --descriptor PATH --allow_metadata_read
```

`PATH` explicitly names the existing daemon control descriptor. No environment
variable selects a descriptor, executable or permission. Missing/duplicate flags,
unknown flags and missing values fail with a fixed error code and exit 2.
Descriptor paths are bounded to 4096 UTF-8 bytes.
`--help` prints usage without descriptor access. Omitting `--allow_metadata_read`
keeps the host usable for initialization and `host_status`, with metadata denied.

MCP initialization may further reduce the launch grant:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"experimental":{"vibemux_metadata_read":false}},"clientInfo":{"name":"example","version":"1"}}}
```

The optional experimental boolean defaults to true; `false` removes metadata
access for this session. It cannot grant access absent the launch flag. Standard
MCP client capabilities do not confer daemon permissions. Tool schemas contain no
credential parameter. Authentication stays in the existing descriptor boundary,
and the descriptor is read only on an authorized metadata call.

## Transport and lifecycle

- UTF-8 JSON-RPC 2.0 objects, one JSON document per newline; no batches or HTTP.
- `initialize`, then `notifications/initialized`, then tool requests. `ping` is
  supported. Duplicate initialization is rejected.
- Standard request/notification `_meta` objects are accepted and ignored. They
  grant no permission and do not weaken a tool's argument schema.
- Only MCP `2025-06-18` is supported. For an unknown requested version the server
  returns `2025-06-18`; an incompatible client must disconnect. This negotiation
  never authorizes use of a future protocol.
- IDs are bounded strings or integer JSON numbers. Null and compound IDs fail.
- Maximum input is 64 KiB excluding the newline. Oversized or unterminated frames
  terminate the session with a fixed diagnostic. Invalid JSON or UTF-8 receives
  `parse_error`. Stdout contains protocol messages only; stderr never includes
  input, upstream errors, descriptor paths or credentials.
- Output JSON payloads are limited to 256 KiB, excluding their one newline
  delimiter, with a 2-second write deadline. The internal reader reply payload
  has a tighter 64-KiB ceiling. Closing output cleans up the reader.
- One producer call may be active; other requests receive `request_busy` while
  cancellation and EOF remain responsive. No unbounded request queue exists.
  `ping` can still receive its empty response; duplicate active IDs fail safely.
- `notifications/cancelled` with the active `requestId` ends the reader generation
  and suppresses the cancelled response. Unknown or malformed cancellations are
  ignored. Closing stdin cancels active work and terminates the owned reader.

Initialization, tool listing and host status never open the descriptor, spawn a
reader, probe a producer or start a daemon. The server advertises
`tools.listChanged: true`; unload/reload notifications are sent only after the
initialized notification, when the available tool set changes.

## Tools and projections

All input schemas reject additional properties. A tool's MCP annotations declare
read-only, nondestructive, idempotent operation and a closed local scope. These
annotations describe behavior; they are not authorization.

- `host_status`, `{}`: effective metadata grant, reader availability, worker
  launch/restart counters and fixed failure metadata. Evidence is local host
  configuration/lifecycle only, never inferred producer health. `worker_launches`
  counts launch attempts, including failed spawns; `launch_count_kind` is
  `attempts`. It is not a live child-process count.
- `health`, `{}`: daemon-declared healthy flag, store schema and bounded queue
  telemetry. Process identity and endpoint are omitted.
- `frontend_tasks`, optional `limit` (integer 1–32, default 32) and `after`
  (canonical non-nil UUID): task/project identities, title, truncation flag,
  declared status, latest sequence and next cursor.
- `frontend_task`, required `task_id` (canonical non-nil UUID): task summary,
  at most 12 run summaries, 24 event summaries and 24 artifact references, with
  continuation flags. A missing task is explicitly null.
- `harness_dispatch_status`, required `request_id` (canonical non-nil UUID):
  request/task/run identities, harness/protocol/phase, version, outcome, error
  code and timestamps. This uses existing dispatch status, never probe, submit,
  output, cancel or session operations.

Task titles are intentionally disclosed metadata, bounded to 240 UTF-8 bytes.
Title control characters are normalized, and `title_truncated` reports changed
text. Other textual metadata has fixed bounds; invalid labels/timestamps/digests
are represented as null rather than returned verbatim. Artifact media types must
have bounded, nonempty MIME type/subtype components; absolute or path-shaped
values are null.
Run paths and branches, event payloads, artifact contents, prompts, source bodies,
process/capture details, prompt size and base commits are not exposed. Results
carry a declared-evidence label: daemon state is not proof of model completion or
verification. Successful results include equivalent `structuredContent` and text
content. Tool execution failures use `isError: true` and a fixed safe code.

## Reader ownership and recovery

The foreground lazily launches one copy of its own executable in a private reader
mode. No MCP argument can choose an executable, argv or environment. The launch
clears the environment, retaining only the documented Windows runtime and temp
variables (`SystemRoot`, `WINDIR`, `TEMP`, `TMP`) when present. Descriptor paths
travel over the contained private pipe, never a shell or public response.

`ProcessTree::prepare_command` precedes spawn; `ProcessTree::contain` precedes
all input. An owned synchronous launch thread performs these OS operations, so
they cannot block the protocol task's polling of ping, cancellation or the read
deadline. The host retains the thread handle until it is terminal. Cancellation
records intent and closes eventual reader input; it does not forcibly cancel
the OS launch. Any eventual child remains owned and goes directly to cleanup if
the request was cancelled or timed out.

During cleanup, host status reports `cleanup_pending`, ping and tool discovery
remain available, and daemon tools are absent. No replacement starts until the
same launch/child owner reaches terminal cleanup and the complete backoff ends.
The parent closes known-child input, terminates the tree, and then reaps the
leader. Its two-second child-cleanup deadline begins when termination starts
and is never extended by other ready requests. This is resource containment,
not an OS sandbox. Cleanup failure ends the session and authorizes no restart.

Final shutdown and owner drop join any pending launch before relinquishing
ownership. A permanently stalled OS launch can therefore delay final host exit;
the read deadline does not prove OS-launch termination. After the launch returns,
the owner-drop fallback terminates the tree and polls the retained child for at
most two seconds. An unconfirmed fallback emits only
`reader_cleanup_unconfirmed`. EOF or a session error during recovery waits for
that same cleanup and suppresses later recovery notifications.

Metadata requests have a 12-second deadline covering launch and the producer
read; response writes retain their separate two-second deadline. The child also
has a 60-second idle deadline and the existing control-client request bounds.
Failure, timeout,
malformed/oversized worker output and crash unload all daemon-backed tools. The
host announces a tool-list change and starts a 200-millisecond backoff only after
terminal cleanup, before restoring availability for a new, explicit call. It never replays
the failed request. At most three restart attempts follow the initial attempt
over the entire session; failed spawns consume this budget and successful calls
do not replenish it. An expired recovery timer precedes new buffered requests.
Idle exit/unsolicited output is observed without a new metadata call. Exhaustion
leaves only host status until a fresh session. EOF cancels pending backoff.

Private worker replies carry generation and request counters. Wrong-generation,
duplicate or unsolicited completions fail closed. No completion from a cancelled
or unloaded generation can satisfy a later request.

## Compatibility and migration

Rust `0.2.0-alpha.1` raises the MSRV and pinned Windows/Ubuntu CI compiler from
1.85 to 1.92 for the approved frontend dependency migration to egui/eframe 0.34.3
and ratatui 0.30.2. The lockfile also includes patched time and rustls dependencies.
The exact repository license allowlist is in
`deny.toml`; it does not disable advisory or source checks.

Control IPC stays v7, storage stays schema 5, and descriptor validation/version
matching stays in `ControlClient`. Supported older descriptors retain that
client's per-operation version floors; unsupported operations fail visibly.
Existing daemon clients and the Python reference require no state migration.
Stop this foreground launch and remove its client registration to disable MCP;
rollback of the additive host never edits daemon state. Live deployment and
native Linux behavior require their own acceptance evidence.

The adversarial reader binary and alternate launch constructors are gated by
the explicit `test_helpers` Cargo feature. Default builds and the normal MCP
launch path have no fixture executable or injection flag.
