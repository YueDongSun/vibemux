# Supervisor context and operator contract editing

## Scope

- Task: approved continuation of the VibeMux runtime enhancement.
- Base: `c8ad7e09c582370943754b87dcc342be4c690d09`, owned branch
  `codex/remaining_runtime`. Main and pre-existing `ui_check/` remain untouched.
- Owned modules: supervisor context contracts, store commands, daemon effects,
  broker/private-state glue, operator contract editing, Control/CLI, tests and
  content-free browser diagnostics.

The original 37-case benchmark is unchanged. This receipt supplements the
[previous supervisor increment](model_supervisor_increment.md). Focused test
results and diagnostic runs are not fresh benchmark scenario PASS or live evidence.

## Implemented

- `ReadArtifact` retrieves admitted candidate excerpts or retained bundle content
  through typed identities. `QueryContext` retrieves permitted task context and
  live public messages. Both return bounded, explicitly untrusted data to the
  next model request, without model-supplied filesystem paths.
- `SendMessage` admits one fixed-ID question through the writer; `ShareContext`
  binds a question/context request to source/recipient sessions and bilateral
  scopes. Admission, delivery and acknowledgement remain separate observations.
- A workflow-history guard refuses arbitrary sends after any applied private
  read/query. The supervisor cannot paraphrase retrieved source into an unscoped
  message. Scoped sharing remains available.
- Applied reads require exact post-claim, content-free retrieval evidence and
  current source session, contract, audience, message state, expiry and scope.
  Existing or expired messages alone cannot prove retrieval.
- Explicit operator edits create immutable successor workflows, preserving exact
  source, original requirements/checks, policy/profile identity, spent counters
  and remaining allowances. One writer transaction retires a quiescent parent,
  admits one successor and records provenance. Active/uncertain parents refuse.
  Exact replay binds the complete immutable payload; it never creates a second
  successor or restores a budget. Broader scope uses ordinary new admission.
- Schema 8 and Control v9 provide this forward boundary. The CLI adds
  `vibemuxctl prompt edit --request-file <file>`.

## Behavior and invariants

Only the daemon writer commits state and audit events. Effects follow claimed
intents and retain exact generation, plan and contract identity. Private bodies
and source text stay outside ordinary action/status/audit rows. Model output
cannot grant permissions, weaken gates, select arbitrary routes or mark success.
Worktree isolation remains distinct from a security sandbox.

Message text is transient in the proposal; a typed digest/byte-count binding is
persisted, and the actual body stays in the existing private outbox. Unknown
delivery remains fenced. Deterministic broker settlement retains its old behavior;
pending/queryable supervisor replies apply only to a running model driver.

## Files changed

Pure contracts live in `vibemux_workflow/src/supervisor/context_actions.rs` and
`contract_edit.rs`. Store effects live in supervisor `context_actions.rs`,
`context_reads.rs` and workflow `contract_edit.rs`. Daemon `supervisor_context.rs`,
model execution, broker and private-state code integrate these effects. Operator
editing has a focused `workflow/service/contract_edit.rs` module and thin writer,
Control and CLI adapters. Focused production-store and guarded daemon cases cover
the boundaries. See [ADR 035](../adr/035_supervisor_context_actions.md) and
[ADR 036](../adr/036_operator_contract_edits.md).

## Validation executed

Exact commands, source stages, failed intermediate checks and final outcomes are
recorded in the [JSON receipt](context_edit_increment.json).

- Semantic-edit pure and store unit checks passed; four production-store edit
  tests and two real Control edit tests passed.
- Repaired context suites passed 23 production-store tests and seven guarded
  model-driver tests. The negative lifecycle reads source-only bytes, attempts
  an unscoped send and requires a typed refusal with zero admitted messages.
- A read outside the recent model observation remains fenced by full workflow
  history. Same-batch timing, wrong sessions/contracts/tasks, expired/dead
  messages and missing/conflicting retrieval evidence have executed coverage.
- Final affected regressions passed: 274 store/domain tests, 139 daemon/CLI unit
  tests and 13 service tests. Workspace formatting and all-target/all-feature
  Clippy with `-D warnings` passed. These overlapping suites are not added into
  an invented unique total. The default-feature workspace build also passed,
  with fixture-only capabilities disabled.

The first compile was interrupted without a terminal result. Later wrapped
commands completed with actual exit receipts. A test fixture borrow error, an old
schema-7 assertion and a test querying full status instead of the bounded model
observation were corrected without removing their behavioral assertions.

## Platform coverage

- Windows: native Rust tests and owned headless browser diagnostics.
- Linux/WSL: not executed for this increment.
- Live terminal backend: unverified; not supplied by this context/edit increment.
- Live model/vendor execution: not run. Model transport in these scenarios is
  synthetic; worker, broker, verifier and integration effects use owned fixtures.

The matched browser probe used Edge through NativeProcess and a direct Node
launcher, both inside owned Jobs, and Chrome through NativeProcess. Edge
154.0.4258.53 timed out at `Runtime.enable` with Windows access-denial markers
through both launchers. Chrome exited before DevTools. All tested Jobs reported
zero remaining members and observed browser cleanup. The direct entry path was
normalized to the same verified vendor-facing form before its final rerun.

This does not establish an uncontained baseline, the exact denied subsystem or
usable browser support. It does show that bypassing the VibeMux trampoline while
retaining Job containment did not resolve Edge's renderer failure. No sandbox
bypass, Job breakaway or host ACL/security mutation was introduced. The earlier
contained browser acceptance gate remains unresolved.

## Compatibility / migration

Schema 7 migrates atomically to 8 with an additive edit-provenance table. Old
records/events remain unchanged; a migration conflict leaves the marker at 7.
Historical schema fixtures also exercise forward migration. Older binaries refuse
8 before reading extended strict message-action JSON. Rollback uses matching
pre-upgrade database/private-state backups and binaries, never state deletion.

Control v9 adds contract edit. Prior operations retain their minimums: supervisor
v8, DSH v7 and deterministic workflow v6. A client that refuses a newer descriptor
needs a matching binary upgrade. The successor is initially non-executable until
immutable authority/recovery/private-plan bindings finish; exact replay restores
them after a crash. Changed HEAD or pinned configuration refuses recovery rather
than silently using new authority.

## Security impact

The independent Codex review identified and the implementation repaired cross-task
message disclosure and stale query-evidence gaps. The focused source recheck found
no remaining blocker in those two repairs; executed test results remain separate
acceptance evidence. No credentials, provider settings, global context-window or
compaction settings were changed. Unknown context identity remains unknown.

## Remaining work

Full Version 2 remains incomplete: standalone review mode, production native
backend/Control/UI, affirmative approval delivery, complete active-attempt/message/
controller recovery joins, native handoff and live gateway/harness certification.
Live execution still requires explicit route/harness/concurrency/time/call/cost/
retention policy. All 37 fresh acceptance scenario results remain NOT_RUN here.

## Known risks

This is an offline integration increment, not a release, main promotion or live
completion claim. Browser diagnostics collect evidence; a diagnostic test completing
does not make its observed browser failures pass. The older optimizer directory
with unknown ownership remains excluded from cleanup. Temporary current worktrees,
logs and build/test scratch are removed only after source preservation and terminal
execution evidence, with compact repository receipts retained.
