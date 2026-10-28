# ADR 036: Execution authority and durable runtime effect boundaries

Status: Accepted for the offline increment. Live route certification, native
terminal integration, and active workflow checkpoint coverage remain pending.

Extends ADR 029, ADR 031, and ADR 032. This decision preserves their isolation,
single-writer, permission, and independent-acceptance boundaries.

## Context

The schema-5 workflow runtime widened admitted tool policy into two launch
booleans. Read and search were always granted, Edit also enabled Write, and
Codex workspace-write could not enforce the abstract development-command
restriction. Optimizer policy also accepted parameters without operational
effects. Model HTTP calls and native-controller effect accounting needed
separate ownership from deterministic workflow admission.

## Decision

- Derive five independent tool grants from the narrowed TaskSpec and the turn
  purpose. Answer and review retain only already-admitted ReadFiles/SearchFiles.
  The writer binds the complete spec digest to its tool summary and requires
  exact effective-tool equality at attempt admission.
- A vendor profile that cannot enforce the abstract grants refuses execution.
  Codex exec has no certified writable profile. Claude's independent tool
  mapping remains uncertified for live coding until installed-version evidence
  establishes the actual denial behavior. Tool flags are not an OS sandbox.
- Synthetic execution uses a distinct fixture profile and an opaque startup
  guard that registers exact copies of the built test worker. A serialized
  fixture label supplies no launch authority. Fixture class without that guard
  cannot certify a vendor route. The fixture enforces each granted effect and
  receives a pinned development-test runtime as data in typed argv.
- Persist the pinned effective dispatch route and purpose mask on attempt v2.
  Candidate/reviewer labels must match their attempts. Review checks compare
  persisted identities; unknown live endpoint identity cannot prove independence.
- Keep AAG HTTP effects in the daemon. The client permits numeric loopback
  origins, no redirects or ambient proxies, bounded bodies/streams/deadlines,
  cancellation, redacted credentials, and correlation identifiers. Responses
  remains unsupported. Local health/model fingerprints are observations rather
  than request-scoped upstream attestations. Live calls refuse before submission.
- Fake gateway calls require a nonserializable permit retaining the exact
  harness-owned listener. Model calls reserve accounting through the writer,
  persist submission before HTTP, retain terminal receipts before returning text,
  and never replay a submitted or uncertain request after restart. Interpretation
  is model output; canonical validation and rendering remain deterministic.
- The pre-admission compiler has one durable reservation under the exact workflow
  identity. Preparation atomically associates that reservation and debits the
  workflow allowance. Duplicate preparation neither refunds nor charges twice;
  the historical pre-admission call identity remains immutable. Rejected,
  blocked, and uncertain calls still consume their reserved allowance.
- Native control has a stable logical controller key and a separately fenced
  physical controller/session generation. Literal input reserves a digest and
  byte count, then atomically claims an outbox before sending. Only the claim
  winner can send. Unknown delivery is durable and non-replayable. Human control
  excludes automation; interruption acknowledgement is not terminal completion.
  Handoff requires terminal predecessor evidence and an explicit successor.
- Native/Human input reservations charge task-turn and model-request allowances
  atomically, without refunding failure or unknown delivery. Structured dispatch
  remains responsible for its existing turn charges. These are allowance charges,
  not invented observations of token usage or paid inference.
- Recovery checkpoints are separate from worker checkpoint claims. The writer
  verifies plan, contract, controller, receipt, and budget identities against
  committed state. Missing joins or context-setting observations fail closed.
  There is no automatic resume authority and no global context/compaction write.
- Reject non-template optimizer parameters as `optimizer_parameter_unsupported`
  before evaluation, promotion, and future admission. Preserve legacy policy
  readability and its immutable history.
- Windows daemon bootstrap uses the canonical executable, typed arguments and
  native process creation. Retain its Child handle until authenticated health;
  terminate that owned handle on startup failure. No shell or job breakaway is
  used. Captured outer-stdio survival remains a separately observed limitation.

## Versions and migration

SQLite advances from schema 5 to schema 6 in one forward migration. It adds
contract authority, model-call accounting, controller/outbox, plan binding, and
checkpoint tables without rewriting historical receipt bytes. Historical attempt
v1 records remain readable; missing authority is `legacy_unverified` and cannot
authorize a new execution. Workflow status/export writes schema 2. Existing
Control operations keep their v6 minimum; the descriptor remains v7 for DSH.
No new native/checkpoint mutation is advertised over Control in this increment.

Backup the database and its matching private workflow state before upgrading a
real project. A schema-5 binary cannot open a schema-6 database. Rollback requires
the matching pre-upgrade database/private-state backup and binaries; checking out
old source does not downgrade data. Never delete or recreate a database to make
rollback appear successful.

## Security and platform limits

User text stays on typed protocol/stdin paths. Events retain identifiers, counts,
digests, fixed codes and public metadata, not prompt bodies, credential values or
hidden reasoning. Worktree isolation remains separate from process containment.
Unknown resources are retained. Fixture evidence cannot promote a live workflow.

The Windows contained-browser diagnostic reaches the browser broker and target
attachment, but renderer initialization reports sandbox/access-denied markers.
The identical direct lifecycle succeeds. Cancellation observed owned Job members
returning to zero. The precise platform mechanism remains unproven; no breakaway,
browser sandbox bypass, or verifier weakening is accepted by this decision.

The native runtime supplies an owned-adapter boundary and writer fixtures; it
does not supply or certify a production native TUI backend. Approval decisions
can be recorded against exact challenges, but affirmative approval transport is
unavailable until its durable delivery/outcome contract exists. Active workflow
checkpoints remain refused where attempt/controller joins, participant cursors,
or artifact ownership cannot be verified. Live model supervisor, vendor route
isolation, standalone review, prompt editing and explicit recovery controls
remain subsequent increments. Full benchmark acceptance remains BLOCKED until
every required execution class has fresh evidence.

## Evidence

Fresh commands and outcomes for this increment are recorded in PROGRESS.md and
the accompanying evidence summary. Pure reducer tests, fake-server transport,
actual writer/control fixtures, platform diagnostics, and live acceptance are
reported separately. No live gateway/model/vendor, publication or deployment
evidence is inferred from an offline fixture.


## Bound model-admission extension, 2026-10-28

[ADR 054](054_gateway_bound_admission.md) adds explicit startup opt-in for AAG v2
snapshot-bound model calls. Legacy/default Live admission still refuses before
HTTP; opted-in calls require a matching complete gateway-observed receipt before
returning text. This supersedes only the blanket model-HTTP refusal above. Native
worker/reviewer profiles, tool grants, sandbox certification, attempt/lease gates
and independent model attestation remain separate and unchanged.
