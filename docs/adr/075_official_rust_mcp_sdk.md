# ADR 075: Adopt the official Rust MCP SDK (rmcp) for vibemux_mcp

Status: Accepted, 2026-11-16. Renumbered to 075: 061-074 are claimed by
unpublished branches (`061_native_history_search.md` onward). This ADR
amends [ADR 035](035_native_mcp_read_host.md) for the protocol
implementation basis only; the read-only boundary, grant model, tool
set, projections, reader process, and `2025-06-18` wire contract are
unchanged. Operator decisions at acceptance: the pin is `=3.10.0` as
written; the hand-written server stays the default until parity is green
on native Windows and Ubuntu, and the default flip plus deletion are
separate later decisions. First implementation slice: the feature-gated
transport/model seam and its parity tests only — the hand-written session
loop and all protocol behavior are unchanged.

## Context

`vibemux_mcp` hand-rolls MCP revision `2025-06-18`: a newline JSON-RPC
2.0 loop, strict envelope and argument validation, five read-only tools,
one out-of-process reader, and crate-owned frame, deadline, restart and
containment budgets (ADR 035). Tracking a moving protocol revision by
hand costs a local change for every new method, notification and error
shape, and the surface is already larger than the five tools suggest.

Repository planning names the official Rust SDK, `rmcp`
(`modelcontextprotocol/rust-sdk`), as the presumptive candidate, subject
to the AGENTS §16 dependency review behind an accepted ADR (mobile
access design §9.2; [upstream adoption plan](../upstream_adoption_plan.md)
P0-②). The C10 cloud case splits in two: `C10-MCP` covers both read-only
metadata services, while full `C10` — a VibeMux worker calling Hub tools
through an admitted broker — has no implementation, admission contract
or evidence ([cloud harness integration](../guides/cloud_harness_integration.md)).

Candidate facts, verified against crates.io and docs.rs on 2026-11-16:
`rmcp` 3.10.0 was published 2026-11-12, is Apache-2.0, declares
`rust_version = "1.88"` and edition 2024; its README says it implements
the stable `2026-07-28` revision while negotiating earlier ones; default
features are `base64`, `macros`, `server`. Upstream CI has no Windows
job, oversized stdio lines are dropped without a JSON-RPC error (open
issue #1395), and a pending stdin read cannot be cancelled. The SDK
internals named in this ADR were read from 3.10.0 sources during review
and must be re-verified when implementation starts. Nothing was built,
run or dependency-reviewed for this draft.

## Decision

1. Adopt `rmcp` as the MCP protocol implementation **only inside
   `vibemux_mcp`**. SDK types never appear in `vibemux_types`,
   `vibemux_events`, `vibemux_harness` or any other domain crate, and
   never in a public Rust ABI; upgrades happen in this one crate
   (AGENTS §10.1). The pin is an `=` entry in
   `[workspace.dependencies]` chosen by the §16 review and locked in
   `Cargo.lock`; this draft nominates `=3.10.0`.

2. Split the work into two independently gated scopes:
   - **Scope A — MCP frontend/server.** Replace the hand-written protocol
     loop with an `rmcp`-based session while ADR 035's grant model, tool
     set, projections, reader process, budgets and wire contract stay
     unchanged. Scope A is the only scope this ADR asks the operator to
     accept.
   - **Scope B — C10 worker→Hub admitted tool-call boundary.** A VibeMux
     worker calling Hub tools through an admitted broker needs an
     admission contract, per-call grants, Hub-side negotiation and its
     own transport decision. It is **not** authorized, designed or
     claimed here and requires a separate accepted ADR before any code.

3. Feature surface: `default-features = false, features = ["server"]`.
   `macros` and `transport-io` are evaluated at implementation, not
   assumed; no `base64`, no `reqwest`/`oauth2`/`jsonwebtoken`, no
   `transport-child-process`, no HTTP transport. Scope A is a stdio
   server; it opens no listener, serves no HTTP, and spawns no vendor
   process through the SDK. New packages expected: `rmcp`, `rmcp-macros`,
   `pastey` — the lockfile already carries `schemars` 1.2.3, `uuid`
   1.25.0 and `base64` 0.23.1. The "roughly 17 direct crates" figure
   applies to a default-feature build, which is **not** the adopted
   configuration.

4. VibeMux owns the transport seam **unconditionally**. The shipping path
   frames and parses on VibeMux terms over the existing `LineReader`
   logic: raw parse and the `-32700 parse_error` / `-32600
   invalid_request` synthesis (with `error.data.code`) stay first-party,
   and the SDK's `AsyncRwTransport` is not used — its oversize
   silent-drop (issue #1395) is the disqualifier, not an option. The
   five limits are enumerated and enforced before any SDK decode or
   write: 64 KiB MCP input (rejected before allocation), 64 KiB reader
   reply (`reader_output_too_large`), 256 KiB MCP output (checked before
   write/flush — arbitrary JSON cannot be sized before serialization),
   2 s write deadline, 4096-byte descriptor path, plus the existing 12 s
   read deadline. An oversized unterminated input frame is fatal: a
   shared out-of-band status channel carries the fixed stderr code to
   `main`, which exits nonzero with empty stdout — never a clean EOF or
   a logged-and-continued send.

5. The handshake stays VibeMux-owned. The SDK's high-level
   `serve_server` entry is **not** used for the session handshake: it
   answers a pre-initialize request with `-32602` and never calls the
   handler, while ADR 035 promises `-32002 not_initialized` with
   `error.data.code` and a session that stays open for a retry; it also
   types `capabilities.experimental` values as JSON objects, while
   `vibemux_metadata_read` is a JSON **boolean** on the documented wire.
   The boolean is therefore parsed first-party, the grant reduction is
   applied before any tool visibility, and an unreadable reduction fails
   closed (no grant — never a silent default to granted). At
   implementation the crate drives the full `ServerHandler` method set
   (including `on_initialized`, `on_cancelled`,
   `supported_protocol_versions`) on its own terms; the behavior
   contract, not the SDK defaults, is authoritative. The server pins
   `serverInfo`, its capabilities object, `protocol_version =
   2025-06-18`, and `supported_protocol_versions = [2025-06-18]`; an
   unknown requested version is answered with ours, and the `2026-07-28`
   inline lifecycle is not accepted in Scope A.

6. Admission, cancellation and shutdown stay VibeMux-owned. The SDK
   spawns a task per request and enforces neither a busy gate nor a
   duplicate-id check, so this crate keeps a try-admit gate (one active
   producer call; `-32000 request_busy`; `duplicate_request_id` for the
   active id; no reply for a cancelled id). Cancellation runs a
   `select!` on the per-call cancellation token that unloads the
   supervisor, emits `list_changed` through the SDK peer, and never
   reuses a reader whose read was abandoned — a dropped read leaves an
   unread reply and `reader_generation_mismatch`, so the reader is
   terminated and replaced. One session owner outside the per-request
   handlers acts as finalizer and awaits the supervisor's final shutdown
   within a stated whole-session bound; the SDK does not join handler
   tasks, so "no handler task outlives cleanup" is a first-party
   obligation with tests. EOF during an in-flight read keeps
   suppressing replies, as today.

7. Idle and unsolicited-output supervision stay ours: a supervision task
   polls the supervisor lifecycle with explicit start/stop ownership,
   emits `list_changed` only after `initialized`, and restarts within
   the documented backoff and budget.

8. Read-only boundary unchanged: only the contained reader touches
   `ControlClient`, and only its four read methods. The foreground never
   reads the descriptor, opens SQLite, obtains a writer handle, launches
   a daemon, or accepts client input that selects an operation, process
   or environment variable. `vibemux_mcp` keeps `#![forbid(unsafe_code)]`;
   dependency `unsafe` is reviewed under §16 and never enters first-party
   code.

9. Rollout is additive behind a crate feature (`rmcp`) plus a parity
   suite; the hand-written implementation remains the default until every
   parity gate passes on native Windows and Ubuntu. Deleting it is a
   later, separate decision. The inbound framing/envelope seam gets a
   bounded fuzz campaign (AGENTS §7.6/§14.1) or an explicit `PROGRESS.md`
   waiver naming owner and expiry.

10. Dependency gates: §16 review, exact pin plus lockfile, `cargo deny
    check` and `cargo audit` clean without new exceptions, MSRV 1.92.0
    respected (candidate declares 1.88), measured compile time and
    binary size, and green CI on native Windows 10/11 and Ubuntu.

## Implementation notes

- 2026-11-16 — First slice landed: workspace pin `rmcp = "=3.10.0"`
  (default features off, `server`), an optional `rmcp` crate feature on
  `vibemux_mcp`, and the `sdk_seam` module: a bounded two-channel
  transport (16 slots each), typed read/write helpers over the existing
  `LineReader`/`write_json` (64 KiB in, 256 KiB out, 2 s write), the
  out-of-band `FatalStatus` code, and a value-identical
  `tools_as_rmcp` mapping of the five-tool contract. Seven seam tests
  pin these; the hand-written session remains the only default path and
  no protocol behavior changed.
- 2026-11-16 — Second slice landed: `vibemux_mcp::sdk_driver` implements
  the contract core — first-party initialize parsing (boolean grant,
  fail-closed on a non-boolean, capped by the operator grant; parity-tested
  against the hand-written `Protocol`), the pinned `ServerConfig`
  (2025-06-18 only), the single-flight `Admission` gate
  (`request_busy`/`duplicate_request_id`, cancel marking with the
  no-reply suppression record the driver will apply), a runtime-gated
  `ServerHandler` (`list_tools` reads the same availability predicate),
  and `dispatch_tool` mirroring the hand-written tool contract
  (`host_status` synthesis, `metadata_read_denied`, argument validation,
  reader unavailability as a tool-level error).
- 2026-11-16 — Third slice landed: `vibemux_mcp::sdk_session` is the live
  driver over `serve_directly` with a VibeMux-owned reader loop — raw
  parse and `-32700`/`-32600` synthesis, first-party initialize and the
  pre-initialize gate, single-flight busy/duplicate answers, cancelled-id
  reply suppression in the outbound filter, one bounded serialized writer
  task, EOF-then-close-then-join ordering with the finalizer after every
  task, and bounded cleanup via `close_with_timeout(SHUTDOWN_DEADLINE)`.
  Three end-to-end in-memory tests cover the full contract (initialize →
  list → call → parse error → tool error → clean shutdown), cancellation
  with no reply and a continuing session, and duplicate/invalid
  initialize refusals. Known deltas from the hand-written loop, both
  recorded here: a duplicate request id echoes the request id instead of
  the hand-written null-id quirk, and idle/unsolicited-output supervision
  arrives with the production executor adapter.
- 2026-11-16 — Fourth slice landed: `vibemux_mcp::sdk_wiring` owns the real
  `Supervisor` in one actor task (bounded command channel, no mutex across
  an await): reads run with the read budget, a watch signal cancels and
  unloads immediately, a periodic tick refreshes the backend snapshot and
  emits tool-change events, and the finalizer sends the single shutdown
  command and joins the actor. `run_session` now takes the cancel hook
  and the event channel (cancellation calls the hook; tool-change events
  become `notifications/tools_list_changed` only after initialize and a
  grant). The fixture-backed end-to-end adapter test and the Windows
  parity matrix remain.

## Alternatives considered

- Keep the hand-rolled loop indefinitely: retained as the fallback while
  parity is unproven; rejected as the end state, because per-revision
  maintenance cost is the problem.
- Use the `#[tool_router]` macro table: rejected on grounds that survive
  the facts — runtime gating is possible (`disable_route`,
  `enable_route`, `with_disabled`) — but the macro router does not give
  the exact JSON-RPC error mapping, the pre-initialize/duplicate-
  initialize/initialized ordering, or the transport integration this
  contract needs.
- Use `transport::stdio()` / `AsyncRwTransport`: rejected; the helper
  cannot express the 64 KiB limit or report oversize, and the transport
  silently drops oversized lines (#1395).
- Take only `rmcp`'s model types and keep the local JSON-RPC loop:
  rejected as the target; it leaves lifecycle and error mapping, which is
  most of the maintenance, in first-party code.
- Move the MCP surface into `vibemuxd`: rejected; client crashes and
  framing failures must stay independent of the single writer.
- Adopt Scope A and Scope B together: rejected; Scope B has no admission
  contract, and merging them would present unimplemented work as planned
  detail.
- Add an HTTP/streamable transport now: rejected; nothing requires it,
  and TLS/auth features add Windows build prerequisites.

## Consequences

Positive:

- Protocol behavior, revision negotiation, content-block typing and
  future MCP methods come from the official SDK and land as crate
  upgrades behind an unchanged wire contract.
- Envelope and argument typing move from hand-written matches to SDK
  types, shrinking the maintenance surface while VibeMux keeps the
  security- and contract-bearing layers (framing, errors, handshake,
  admission).

Negative and risks:

- Dual maintenance during parity: the hand-written path is the default
  until gates pass, so both implementations ship enabled in the crate.
- The SDK's task-per-request model and missing joins require the
  first-party admission/finalizer work in Decision 6; a mistake there
  reintroduces concurrency bugs the current single-flight loop cannot
  have.
- Windows throughput must be measured before any concurrency claim: the
  blocking stdio path has no fast-path equivalent to the Linux
  `stdio_pipes` path.
- An uncancellable pending stdin read remains contained by the 12 s
  deadline and the 100 ms runtime shutdown timeout; on Windows the
  blocking path makes that containment load-bearing.
- Upstream has no Windows CI; native Windows evidence is ours to produce.

## Threat model and failure boundary

- Untrusted MCP input is now typed by the SDK after VibeMux framing and
  parse; malformed JSON, invalid UTF-8 and batch shapes are rejected by
  first-party code before the SDK sees bytes it could mis-handle.
- Oversize input is rejected before allocation; oversize output aborts
  before write; both exit through the out-of-band fatal path.
  `rmcp`'s own `unsafe` (if any) is reviewed under §16 and never enters
  first-party code.
- Descriptor and credential validation stays in `ControlClient`; Scope A
  adds no listener, no HTTP surface, and no SDK-owned process spawn.
- A same-user executable, a compromised daemon, and the absence of an OS
  sandbox remain outside the claim; `ProcessTree` is process ownership,
  not a sandbox.

## Companion changes

- `docs/adr/035_native_mcp_read_host.md`: status pointer to this ADR and
  correction of its stale "control IPC v7 / store schema 5" text
  (current: v10 / schema 15).
- `docs/index.md`: this ADR; `PROGRESS.md`: dated entry and status row;
  `CLAUDE.md`: the status sentence and the `crates/vibemux_mcp/` layout
  line; `docs/architecture.md`: the Native MCP frontend paragraph and
  crate-list comment; `docs/native_mcp_protocol.md`: implementation and
  transport note; `docs/protocol_boundaries.md`: MCP bullet;
  `docs/upstream_adoption_plan.md`: P0-② status.
- `[workspace.dependencies]` pin plus `Cargo.lock`; `deny.toml`/`NOTICE`
  if the §16 review changes them; the `ci.yml` job that builds the new
  feature on Windows and Ubuntu.
- If the advertised revision ever moves beyond `2025-06-18`, AGENTS §17
  applies: old/new examples, compatibility tests, version bump,
  changelog and `PROGRESS.md` update.

## Validation plan

Parity gates (all required before the feature is no longer opt-in):

- Feature-off default: `cargo test -p vibemux_mcp` (the hand-written
  fallback stays exercised) and feature-on: `cargo test -p vibemux_mcp
  --features rmcp`.
- Workspace truth: `cargo test --workspace --all-features` and
  `cargo nextest run --workspace --all-features`; clippy
  `-D warnings`; `cargo deny check`; `cargo audit`.
- Behavior parity cases: pre-initialize `tools/list` → `-32002
  not_initialized` with `data.code`; duplicate initialize;
  invalid `clientInfo`; `notifications/initialized` ordering and
  `list_changed` only after it; malformed input (invalid UTF-8,
  truncated JSON, batch/array, empty line, 129-byte string id, numeric
  id above `i64::MAX`); grant reduction `false`/`true`/absent/malformed
  (malformed fails closed); oversize unterminated inbound → nonzero
  exit, fixed stderr code, empty stdout; oversize outbound aborts before
  write; reader-reply 64 KiB ceiling; concurrent call flood →
  `request_busy`; duplicate id; cancel-during-read followed by another
  call; cancel during launch/cleanup; idle reader exit → unload plus a
  single `list_changed`; launch restart backoff; EOF during an in-flight
  read; whole-session shutdown joins every task under the bound.
- Native Windows 10/11 + PowerShell evidence (framing, oversize, cancel,
  shutdown), with green Ubuntu CI as the second gate; upstream macOS CI
  and Linux results are not substitutes.
- Cross-project frame: `python scripts/verify_fused_harness_cloud.py
  --cases C10-MCP,C10 --hub-root <hub-checkout> ...` reports `C10-MCP`
  green and `C10` `not_run` (selecting only `C10-MCP` leaves no `C10`
  row).

## Open questions

- Pin cadence: resolved at acceptance — `=3.10.0` stands; a patch bump is
  a normal §16-reviewed dependency change.
- Fallback lifetime: resolved at acceptance — the hand-written server is
  the default until parity holds; the default flip and any deletion are
  separate later decisions.
- Windows throughput: what baseline must the acceptance measurement
  record before any performance claim, given the blocking stdio path?
- Scope B shape (client role with `transport-child-process` vs a
  first-party broker) is deliberately deferred to its own ADR.
