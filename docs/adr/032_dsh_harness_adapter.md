# ADR 032: DSH harness adapter and the DSH session interaction substrate

Status: Accepted and implemented. Stage 1 (pure substrate and harness
registration in `vibemux_harness::dsh`), Stage 2 (the shared session driver in
`vibemux_dsh` and the `vibemux_dsh_tui` operator surface), and Stage 3
(`vibemuxd`'s session service, Control v7, and the `vibemuxctl dsh` client) are
implemented in this change. What remains unimplemented is listed under
`docs/dsh_session_protocol.md` §10 and `PROGRESS.md`; it is a canonical
persisted session record and live validation against a real vendor runtime.

Integration compatibility: workflow operations keep their established Control
v6 minimum (ADR 031); DSH operations require v7. The daemon accepts v1-v7
requests for their compatible operations and returns the requested version.
Older clients reject the v7 descriptor, so daemon and clients upgrade together.
This merge adds no persistence migration beyond workflow schema 5. DSH sessions
remain in memory. Rolling back the workflow code requires the database and
private workflow-state backup described in ADR 031; a schema-4 binary cannot
open a schema-5 database.

The generic daemon dispatch loader shares the session driver's DSH package
ownership check. Only this protocol may use Node, and both canonical launcher
paths must be regular files outside the project. An unowned script or a
repository-supplied launcher is refused before any process starts. This is
an operator configuration trust boundary, not an OS sandbox or workflow
write-mode certification.

Extends ADR 013 (detection-gated enablement), ADR 015 (single writer), ADR 017
(local control IPC), ADR 026 (theme system), and ADR 029 (daemon-owned harness
dispatch). Amends AGENTS.md §3.2 (first-party structured adapters) for a
long-lived session protocol, and `docs/architecture.md` for the DSH surface.

## Context

DeepSeek Harness ("DSH") is a local coding harness distributed as the `dsh`
launcher plus a set of Cordis plugin packages under the npm scope
`@deepseek-ai`. A "profile" is an ordered stack of plugin-bundle patch layers
composed at boot, so one launcher serves several applications (for example
`web`, `desktop`, `headless`, `sdk`).

VibeMux already models ten coding harnesses and can run one structured vendor
turn per request (ADR 029). DSH is not one of them, and it differs from the
existing five adapter targets in one decisive way: it publishes a
**first-party machine protocol** rather than only a human TUI.

### The protocol this adapter binds to

`@deepseek-ai/dsh-sdk-protocol` (0.1.2-rc.1) defines "the newline-delimited
JSON-RPC stdio transport and the named request, result, and notification types
spoken between the runtime server and SDK clients". Facts taken from the
installed package on the reference host:

- **Transport.** JSON-RPC 2.0, one message per `\n`-terminated line, over
  caller-owned byte streams. A frame with both `id` and `method` is a request,
  `id` alone is a response, `method` alone is a notification; malformed lines
  are ignored by the reference implementation.
- **Client-to-server requests.** `initialize` (`InitializeParams` →
  `InitializeResult`), `session/prompt` (`SessionPromptParams` →
  `SessionPromptResult`), `shutdown` (no params → `{}`).
- **Server-to-client notifications.** `session.event`, `session.status`,
  `subagent.started`, `subagent.finished`.
- **Server identity.** `serverInfo.name` is the wire-stable string
  `deepseek-harness-sdk-runtime`.
- **Session identity is client-chosen.** `SessionPromptParams.sessionId` is the
  SDK-side session id; "an unknown id lazily creates the agent+session pair".
- **`session.event` is unfiltered.** Every notification carries "every session
  in the runtime, not only SDK-created ones".
- **Terminal evidence is structured.** A `session.event` whose `event.type` is
  `turn/end` carries `data: { turn, reason }`, where `reason.kind` is one of
  `completed`, `aborted`, `blocked`, `error`, `max-tokens`, `interrupted`.
- **Initialization is a real handshake.** The server "rejects `session/prompt`
  until that handshake succeeds", so a missing adapter, unavailable model, or
  unsupported reasoning effort cannot silently fall back.
- **There is no cancel and no session-close method.** The protocol's own
  limitation list says a client abandons a turn "by closing the runtime
  process".
- **No protocol-version negotiation.** The handshake carries only
  `serverInfo.version`, unvalidated by clients, with no compatibility promise.
- **Server-to-client requests are a dead capability.** The transport supports
  them; the server never sends one.

### Capability that VibeMux lacks

| Capability DSH publishes | VibeMux today |
|---|---|
| A first-party, documented, versioned machine protocol | Native adapters only for Codex, Claude, and ACP vendors |
| A long-lived runtime that serves many turns on one process | One prompt per spawned process (ADR 029) |
| A server-side durable session id chosen by the client | No session identity above the dispatch request |
| A structured per-session event log with a monotonic `seq` | A flat per-attempt transcript with a daemon-assigned index |
| Structured turn termination (`turn/end.reason.kind`) | Per-vendor terminal correlation in `ProtocolSession` |

## Decision

### 1. Placement: pure logic in `vibemux_harness`, effects in `vibemux_dsh`

`vibemux_harness` gains a `dsh` module tree that is pure logic: no I/O, no
tokio, no filesystem, no clock, no `unsafe`. It contains the wire vocabulary,
the strict bounded decoders, the client-side session state machine, the record
model, and the outcome rule. It reuses the existing
`vibemux_harness::dispatch::json_line_framer` for framing rather than adding a
second framer.

A new crate `vibemux_dsh` owns every effect: resolving and verifying the
operator configuration, spawning the runtime through
`vibemux_platform::ProcessTree`, moving bytes over stdio, enforcing deadlines,
terminating the contained tree, and holding the bounded record buffer. It
depends only on `vibemux_harness` (pure) and `vibemux_platform` (containment).

Effects live in their own crate rather than in `vibemux_frontend` for two
reasons. The layering rule puts process and platform access below UI code, and
Stage 3 needs exactly this driver behind `vibemuxd`'s control operations:
putting it in the terminal UI would force the daemon to reimplement framing,
deadlines, and containment.

This mirrors ADR 029 Decision 1 and AGENTS.md §3.2's exception for first-party
structured adapters.

### 2. DSH is the eleventh registered harness

`AgentKind` gains a `Dsh` variant with the wire name `dsh`, the command name
`dsh`, and the display name `DSH`. This is a **wire contract change**: the
variant name appears in the probe report JSON, the persisted schema-3
projections, and the CLI row output, exactly like the existing ten.

Consequences accepted with this decision:

- the pinned `AgentKind` wire-name test changes from ten names to eleven;
- `vibemux_probe`'s detection table and its report fixture gain a DSH row;
- the Rust registry and probe fixtures gain the matching name; Python
  behaviour-reference parity remains pending and is not evidence for DSH;
- every document that says "ten harnesses" must say "eleven".

Detection stays `--version`-verified, as for every other harness: a bare PATH
hit is not detection (ADR 013).

### 3. `NativeProtocol::DshSdkJsonRpc`, and the Node launcher pair

A new `NativeProtocol` variant `DshSdkJsonRpc` describes the DSH SDK stdio
protocol. It accepts only `AgentKind::Dsh`, supports the initialize-only probe
(`initialize` is a real handshake that sends no prompt), and supports
execution.

**Launch posture.** On the reference host the `dsh` command resolves to a
`dsh.ps1`/`dsh.cmd` shim that runs `node.exe <npm root>/@deepseek-ai/dsh/lib/bin.js`.
ADR 029 requires a route executable to be a native image whose file stem equals
the harness command name, precisely so that a route cannot point at an
interpreter under another harness's name. DSH cannot satisfy that rule: its
vendor runtime *is* a Node interpreter.

Rather than weaken the rule for every route, this ADR adds a second, narrower
launch shape that only `DshSdkJsonRpc` may use:

- the route names `executable` = a native image whose file stem is `node`
  (ASCII case-insensitive on Windows);
- the route names a new required field `entry_script` = an absolute path to a
  `.js` file inside a package whose `package.json` declares
  `"name": "@deepseek-ai/dsh"` — the daemon canonicalizes the path and pins
  the SHA-256 of both the image and the script at load time;
- the route may name `profile` (default `sdk`), bounded to
  `[a-z0-9][a-z0-9-]{0,63}`;
- the resulting argv is exactly `[<entry_script>, "--profile", <profile>]`;
  no operator argv exists, and the prompt never reaches argv.

A `.ps1`, `.cmd`, `.bat`, or `.vbs` route path stays rejected for every
protocol, including this one.

### 4. A long-lived session substrate, not a second one-shot path

The DSH runtime serves many turns on one process, and its session identity is
client-chosen. Modelling that as a one-shot dispatch would throw away the
runtime's own session model and force a fresh process per turn.

`vibemux_harness::dsh` therefore implements a **session state machine**:

```text
Starting -> Initializing -> Ready -> Prompting -> Ready -> ... -> Closing -> Closed
                                  \-> Failed / Cancelled
```

- `initialize` is written once, after spawn, before any prompt.
- `session/prompt` requests are admitted only in `Ready`; a second prompt while
  `Prompting` is refused rather than queued.
- Each request carries a client-allocated `id`; a response with an unexpected
  `id`, a non-object `result`, or an `error` member is a protocol violation
  that ends the session, not a retryable condition. A late answer to a request
  the session abandoned by cancelling or closing is expected, and is dropped
  once (`docs/dsh_session_protocol.md` §4 rule 3).
- Only notifications whose `sessionId` equals the session's own id advance that
  session. Notifications for other sessions in the same runtime are counted and
  dropped, because the protocol is unfiltered by design.
- Unknown notification methods and unknown `session.event` types are tolerated:
  the protocol permits extension, and `SessionEvent.ignorable` is the
  documented compatibility mechanism.

### 5. Completion authority is `turn/end`, never `session.status`

`session.status: idle` means the whole-agent state went idle. AGENTS.md §2
invariant 5 forbids treating idle state as proof of task completion, so the
substrate never maps it to success.

The only completion authority is a correlated `session.event` whose
`event.type` is `turn/end`, mapped through an explicit table:

| `reason.kind` | Substrate outcome |
|---|---|
| `completed` | `Completed` |
| `aborted` | `Cancelled` |
| `blocked` | `Failed` |
| `error` | `Failed` |
| `max-tokens` | `Failed` |
| `interrupted` | `Failed` |
| absent or unknown | `Unverified` |

An unrecognized `reason.kind` is **not** an error: the vocabulary is
merge-extensible, so it degrades to `Unverified` rather than ending the
session. `ProcessExit` alone never yields `Completed`.

### 6. Cancellation terminates the contained process tree

DSH publishes no cancel method, and the protocol's own documentation says a
client abandons a turn by closing the runtime. Cancellation is therefore
implemented as an orderly teardown:

1. stop admitting new prompts;
2. write `shutdown` and wait for the grace period;
3. terminate and reap the process tree through `vibemux_platform::ProcessTree`.

The session outcome of a cancelled turn is `Cancelled`, and the canonical
record keeps the fact that termination was forced.

### 7. Operator configuration is a separate, versioned file

DSH session config lives in `.vibemux/dsh_session.json` (schema 1), read once
per session, and **absent by default** — an absent file means DSH is off and is
not an error, while an invalid one is refused with a fixed code before any
process starts.

The file carries no prompt text, no credentials, and no operator argv. It names
the launcher pair of Decision 3 (executable, entry script, profile), the
provider and model that `initialize` requires, an environment allowlist, and
the session-level bounds below. It is deserialize-only: paths never leave
through serialization, and only a content-free projection is printable. See
[`docs/dsh_session_protocol.md`](../dsh_session_protocol.md) for the schema.

### 8. Bounded records with a monotonic cursor

The driver keeps a bounded, in-memory, monotonically sequenced record buffer per
session. A consumer reads it by cursor, which fits the existing
one-request-per-connection control IPC and needs no server push channel, and
which the terminal surface polls today.

Bounds are explicit and validated at load time, with the same discipline as
`DispatchLimits`: a record ceiling, a byte ceiling, a frame ceiling, an
initialize deadline, a per-turn deadline, and a shutdown grace. A single record
larger than the byte ceiling ends the session with a fixed code rather than
being truncated in place; otherwise the oldest records are evicted and counted,
and a page whose cursor predates the retained window reports the gap instead of
silently claiming completeness.

Raw DSH records stay byte-exact inside the buffer and leave the driver only
through the poll operation. Session summaries, logs, and errors stay
content-free, following ADR 029 §5: a prompt contributes a hash and a byte
count, never its text, and a working directory contributes only its length.

### 9. Surfaces

- **Terminal UI (implemented).** A standalone `vibemux_dsh_tui` binary in
  `vibemux_frontend`. It is a new binary rather than a pane in
  `vibemux_frontend_tui` because the existing diagnostic TUI is deliberately
  read-only, and because the DSH surface needs an input loop and a teardown
  path. It reuses the audited `tui::Theme` system (ADR 026), never opens the
  core database, and writes no canonical state. It drives `vibemux_dsh`
  directly, so an operator needs no daemon; the control surface below reaches
  the same service for every other client.
- **Control IPC v7 (implemented).** Six operations:
  `dsh_session_open`, `dsh_session_prompt`, `dsh_session_poll`,
  `dsh_session_status`, `dsh_session_cancel`, `dsh_session_close`. A pre-v7
  client refuses a v7 descriptor, so `vibemuxctl` and `vibemuxd` upgrade
  together, exactly as for v5 (ADR 017, ADR 029). `vibemuxd` holds the session
  in `DshSessionService`, which joins it at shutdown so no vendor process
  outlives the daemon.
- **CLI (implemented).** `vibemuxctl dsh open|prompt|poll|status|cancel|close`,
  a thin Control v7 client in the shape of `vibemuxctl dispatch`. A prompt
  comes from a file or stdin, never from argv, and `dsh poll` is the only
  command that prints runtime content.

### 10. At most one DSH session per project

A project holds at most one live DSH session. The daemon refuses a second
`dsh_session_open` with `dsh_session_busy` rather than queueing it, bounding
process count, memory, and provider spend per project. The same rule is
enforced one level down: a single driver admits one turn at a time, because
the wire carries no correlation id between a prompt and a turn, so admitting
two concurrent prompts would make turn attribution impossible.

### 11. The runtime is not a sandbox

The DSH runtime runs in the project root with whatever tool permissions its own
profile grants. VibeMux's process-tree containment controls lifetime and
cancellation; it does not constrain what the runtime may read or write. Route
documentation and the TUI status line both say so.

## Consequences

- VibeMux gains a first-party structured adapter for a long-lived harness, with
  a session model that the existing one-prompt-per-process dispatch cannot
  express. ADR 029 gains `NativeProtocol::DshSdkJsonRpc` for initialize-only
  probes. Generic prompt-bearing dispatch is refused until a certified
  read-only DSH profile exists; protocol support and operator execution
  consent alone cannot establish that posture. The dedicated SDK driver and
  session surfaces remain available under their explicit operator configuration.
- Rust `AgentKind` grows to eleven, so its probe report fixtures, store
  projections, and CLI rows change together. The Python prototype still has
  its previous harness set; it does not provide DSH parity in this slice.
- The route config gains two fields (`entry_script`, `profile`) plus a
  `provider`, and they are protocol-conditional in both directions: a
  launcher-pair protocol must carry them, and no other protocol may. Both
  directions are tested, because a half-applied rule would let a route smuggle
  an unused interpreter or an unnamed provider through the config.
- A second crate joins the workspace. It sits beside `platform` in the
  dependency direction — below the UI, above the pure domain crates — so Stage
  3 can reuse it without a cycle.
- Because `session.event` is unfiltered, a client that owns one session still
  receives every other session's events. The substrate filters by id and counts
  the dropped records, so the behaviour is observable rather than implicit.
- Because the wire carries no prompt-to-turn correlation id, the substrate
  admits one turn at a time. A future concurrent-turn feature would have to
  attribute turns some other way, and this ADR does not authorize one.
- The runtime is Node, so startup latency and memory are larger than a native
  vendor CLI. The initialize deadline is a load-time setting rather than a
  constant, because a cold Node boot and profile resolution dominate.
- Booting the `sdk` profile for the first time **writes to the operator's DSH
  home**: DSH auto-initializes a templated profile on first use, and rewrites
  that profile's `cordis.yml` on every boot. Operators must expect that side
  effect; `docs/dsh_session_protocol.md` states it.
- No compatibility promise is made: DSH's protocol carries no version
  negotiation yet, so this adapter binds to the observed 0.1.2-rc.1 shapes and
  must be re-verified against later releases.

## Alternatives rejected

1. **One-shot dispatch only (a `DshSdkJsonRpc` route in the ADR 029 path, no
   session substrate).** Smallest change, and rejected because it discards the
   runtime's session model: every turn would pay a fresh Node boot, no
   multi-turn conversation would be expressible, and the terminal UI could not
   exist without a session to attach to.
2. **Driving the `web` profile's HTTP gateway.** `@deepseek-ai/dsh-api-gateway`
   and `@deepseek-ai/dsh-host-webserver` expose a richer API, but it is not the
   documented client contract, it binds a local network port, and it would make
   the adapter's compatibility depend on a browser-facing surface. The SDK
   stdio protocol is the surface DSH publishes for out-of-process clients.
3. **Using `dsh-acp` (Agent Client Protocol) instead.** DSH does ship an ACP
   server, and VibeMux already has an ACP profile. Rejected as the primary
   binding because ADR 029 keeps ACP initialize-only until a per-vendor
   permission posture exists, and because the SDK protocol is DSH's own
   contract rather than a third-party one.
4. **Embedding a terminal (PTY/ConPTY) and scraping the DSH TUI.** Rejected:
   AGENTS.md §2 invariant 5 forbids inferring completion from terminal text,
   and the project's standing position is that native-TUI attach is premature.
5. **Allowing `dsh.cmd`/`dsh.ps1` as the route executable.** Rejected: it routes
   argv through a command interpreter, which AGENTS.md §12.1 restricts to a
   narrow tested shim and which the dispatch trust rule exists to prevent. The
   Node image plus a pinned entry script achieves the same result with a real
   image and no interpreter.

## Open decisions

1. **Store schema.** Whether a DSH session earns a persisted record (like
   `harness_dispatches`) or stays in daemon memory (like the ADR 029
   transcript) is still open. Today it stays in memory: a daemon restart loses
   the session and its records, matching the transcript rule that a restart
   loses raw vendor content.
2. **Multi-session.** Decision 10 permits one session per project. Whether a
   later revision permits one per Run is deferred.
3. **Provider credentials.** The runtime resolves its own credentials from the
   operator's DSH home, so `DEEPSEEK_API_KEY` must be present there or in the
   forwarded environment. VibeMux never reads, forwards, or stores the
   credential file; surfacing a missing-credential failure as a specific code
   rather than a generic `-32603` is deferred.
4. **Version negotiation.** If DSH later adds protocol version negotiation,
   this adapter should require it rather than keep binding to observed shapes.
5. **Consolidating the two route loaders.** `vibemuxd::harness_dispatch::config_loader`
   and `vibemux_dsh::config` both verify a trusted launcher pair, for the
   dispatch route and the session configuration respectively. They share the
   pure shape rules in `vibemux_harness::dispatch::route_config` but not the
   filesystem walk. Unifying them is follow-up work; until then, a change to
   the trust rule must land in both.
6. **Richer per-record styling.** The terminal surface styles records through
   the audited theme's title and muted styles only. A per-record-kind palette
   is deferred to avoid inventing colours outside ADR 026's registry.

## References

- DSH SDK protocol: `@deepseek-ai/dsh-sdk-protocol` README and
  `lib/types/types.d.ts` (newline-delimited JSON-RPC 2.0 stdio transport).
- DSH SDK server: `@deepseek-ai/dsh-sdk-jsonrpc-server`.
- DSH SDK application bundle: `@deepseek-ai/dsh-sdk-app`
  (`dsh --profile sdk`).
- DSH session vocabulary: `@deepseek-ai/dsh-session`
  `lib/types/types.d.ts` and `lib/types/known-event-types.js`.
- Related decisions: ADR 013, ADR 015, ADR 017, ADR 026, ADR 029.
- Protocol contract for the VibeMux side: [`docs/dsh_session_protocol.md`](../dsh_session_protocol.md).
