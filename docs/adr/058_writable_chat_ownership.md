# ADR 058: Lifetime writable chat ownership

Status: Accepted, 2026-11-01. Lifetime ownership, provisioning, attempt storage
and private-writer persistence are implemented. The internal owner-runtime
service is fixture-verified. Public Control v13 and native desktop conversation
controls are implemented and native synthetic-vendor fixtures pass. Writable
profiles, approval transport and success finalization remain open.

Extends [ADR 057](057_persistent_harness_sessions.md) and preserves the
execution-authority requirements in [ADR 036](036_runtime_execution_authority.md).

## Context

Persistent read-only chats allocate a canonical Task and Run for each turn.
A writable conversation needs a stable workspace across follow-up turns and
vendor process restarts. The repository requires every writable Run to own an
independent Git worktree and branch.

The existing workflow workspace ledger reuses a worker directory while later
dispatch admissions allocate new Runs. Its workspace-generating Run ID and
the dispatch Run ID are distinct. That mapping cannot establish the ownership
required for a persistent writable chat.

## Ownership mapping

Allocate one owner Task and Run for the writable chat's work lifetime. Bind
that owner to one independently owned worktree and branch. Record each user
submission as a correlated writable attempt with its own immutable request ID,
attempt ID, process generation, effective grants, and result.

Keep the existing read-only `chat_turns` mapping and validators. Add a separate
owner binding and writable-attempt representation. Existing validators require
unique Task/Run IDs per read-only turn and compare historical turn outcomes to
canonical Run status. They cannot interpret several attempts sharing a live
owner Run.

The owner binding records project/chat identity, canonical Task/Run IDs,
fixed base commit, verified worktree/branch identity, resource reservation,
admitted profile/authority revision, and provisioning/finalization state.
Generic projection and A2A guards must recognize the binding before the first
attempt exists.

## Provisioning and lifetime

The single writer allocates the owner Task, Run, chat, event and Open receipt
atomically. The daemon persists the planned path, branch and ownership token,
then claims creation durably before invoking Git. `WorkspaceManager::create_planned`
uses that exact plan. Receipt inspection and fixed-base verification precede
the writer's transition to `Open`. The canonical Run remains `Preparing`.

Duplicate Open returns the existing owner. Restart preserves pending plans
and claims for explicit `Reconcile`. A claimed effect can recover through its
matching receipt. An unresolved claim prevents another creation or Stop.
Known unverified process cleanup also prevents reconciliation effects and
requires operator investigation. This stage has no cleanup-acknowledgment API.
Exact operation replays return the retained receipt before new-work admission
checks. Changed reuse of an operation ID conflicts.

Launch cwd, protocol cwd, resource key and resume identity must all come from
the verified binding. Reserve the workspace throughout idle and suspended
periods as well as during active attempts. Revalidate ownership before each
launch and send. A changed branch, moved HEAD, changed authority, or mismatched
resource requires explicit reconciliation. Preserve the original base commit
and all candidate files during that process.

The Run enters `Running` on its first admitted execution. Individual attempt
completion, cancellation and failure update that attempt. Transport Close,
suspend and verified process shutdown leave the owner logically open. Existing
`Closed` chat state remains resumable while its owner is nonterminal.

Uncertain delivery or ownership fences further execution through the session
and attempt states. Avoid mapping resumable uncertainty to canonical `Stale`,
whose existing transitions cannot return to `Running`. An explicit recovery
operation must reconcile the exact attempt, generation, process cleanup and
worktree before permitting another send.

Add a distinct owner-finalization operation. Explicit stop/failure terminalizes
the Run and ends its ability to resume. Success finalization requires verified
acceptance. Archive and private-content purge retain their content/listing
responsibilities. Subsequent writable work after finalization creates a new
owner and independent worktree with an explicit context handoff.

## Permissions and acceptance

Pin the owner's admitted authority. A turn can narrow its effective grants.
Prompt text, attachments and permission replies cannot expand those grants.
Affirmative approval transport needs its own exact challenge, operation,
generation, expiry, one-use delivery and unknown-outcome contract. The existing
controller records decisions but has no affirmative transport that can satisfy
this requirement.

Freeze new sends before success finalization. Collect the candidate, run
independent review and verification in separately owned resources, then
revalidate the candidate, owner and generation before committing success.
Reuse existing pure collection and acceptance logic through a chat-bound
adapter. Workflow receipts require workflow contracts, leases and attempts,
so they cannot certify a chat owner by substituting identifiers.

Process cleanup and workspace cleanup have separate receipts. Finalization
retains candidate files. A later cleanup uses the workspace manager's existing
plan-first checks and retains dirty, unknown or mismatched resources and
branches.

## Compatibility and implementation order

Store schema 11 adds `chat_execution_owners` and its workspace reservation
index. Migration is transactional and forward-only. Historical read-only chat
rows keep their schema and meanings. Schema 12 adds `chat_execution_attempts`
and `chat_execution_records`, with a separate foreign-key chain and one active
attempt per owner. Shared content quotas and request-ID collision checks cover
both representations. Per-attempt outcomes never terminalize the lifetime Run.

| Observation | Owner Run | Attempt ledger |
| --- | --- | --- |
| Verified workspace, before a submission | `Preparing` | Empty |
| First admitted submission | Same Run becomes `Running` | New request/attempt identity |
| Two completed submissions | Same Run remains `Running` | Two distinct completed attempts |
| Unverified process cleanup | Run remains nonterminal | Claimed attempt retains recovery state and process fence |

The owner version advances with admission. Session and attempt versions track
runtime observations. A session in `RecoveryRequired` blocks its owner Task
while preserving the workspace binding. Process cleanup changes neither the
workspace-creation claim nor the lifetime owner identity. Verified cleanup may
settle an interrupted attempt and permit an exact-vendor resume.

Claude's control initialization precedes its root session/profile proof, including
exact resume. A new root permits one initial input with an unused pinned UUID
and no previous send claim. A retained session permits one first input in a new
generation when its expected UUID matches exactly and that generation has no
claimed input. Root confirmation remains required for acknowledgement and
correlated completion. Codex requires a confirmed thread before Ready. Startup
fences unfinished execution generations without granting a replacement claim.

The writer facade publishes immutable prompt/output bodies before their
references commit. Replay consults existing attempt/record receipts first.
Explicit history reads report missing bodies without republishing them. Runtime
sink uses these owner-specific methods and preserves the protocol/profile proof
ordering. The read-only per-turn sink remains separate. Both sinks share bounded
private output bookkeeping without sharing canonical turn settlement.

The internal `ChatService` execution methods serialize Start, Send, cancellation
and process closure. Fresh launch and input verify the stored workspace receipt,
inventory, branch, resource and HEAD. Start also validates the pinned profile.
Start and Send resolve retained immutable bindings before new-work checks, and
replays never grant another launch fence or enqueue input. Process release is
the sole sink teardown transaction. A timed-out claim response can leave local
capture absent after the writer committed; exact pending request/attempt identity
still permits cleanup settlement without asserting completion.

Control v12 adds the separate `chat_owner` operation with closed request and
response enums. It exposes `List`, `Open`, `LookupOpen`, `Status`, `Title`,
`Reconcile` and `Stop`. Open requires explicit private-storage and workspace
consent and admits an empty tool set. The legacy `chat` operation keeps its
v11 minimum and read-only aggregate semantics at every accepted envelope
version. It excludes owner chats from paginated discovery and refuses access
through `chat_owner_control_required`.

The daemon accepts historical v1–v11 request frames. Already-built v11 clients
reject its v12 descriptor, so the daemon and clients upgrade together. Existing
read-only sessions require an explicit new owner and context handoff for future
writable work. The desktop `chat_owner_request` command exposes this closed
contract through registered project handles. Its Workspaces view requires
explicit creation consent, retains uncertain request bindings through navigation
and provides receipt reconciliation and file-retaining Stop controls.

Implement ownership/provisioning and suspension first, with tools disabled.
Then implement bounded writable profiles and approval delivery, followed by
independent success finalization. Fixture checks must cover provisioning crashes,
duplicate admission, idle reservations, stale generations, restart recovery,
changed Git identity, rejected approvals and unchanged historical reads.
Native vendor qualification follows under its own bounded execution authority.

## Security boundary

### Control v13 execution contract

Control v13 allocates the separate closed `chat_execution` operation. It exposes
Status, Start, LookupStart, Send, Submission, Cancel, process Close, Attempts,
Content and Live. Start requires explicit execution consent. The native
`chat_execution_request` command resolves a registered project before and after
dispatch, guards foreground mutations, and requires the project's identity-bound
chat inference/storage consent for Start and Send. Workspace creation consent
continues to use the separate owner API.

An unchanged v12 owner status argument remains:

```json
{"operation":"status","chat_id":"00000000-0000-4000-8000-000000000001"}
```

The v13 execution Start argument binds a fresh operation to the displayed owner:

```json
{"operation":"start","operation_id":"00000000-0000-4000-8000-000000000002","chat_id":"00000000-0000-4000-8000-000000000001","generation":1,"owner_version":4,"consent_execution":true}
```

LookupStart uses the same operation/chat/generation/owner-version binding and
performs no launch. Its positive receipt can include the current runtime; it
does not grant another effect or prove that the old generation is running.
Null lookup remains observational. The renderer starts/resumes only on an
explicit action and sends separately after observing current readiness.

Process Close uses `(chat_id, generation)` as its natural idempotency key.
Duplicates serialize through the service, verified cleanup precedes success,
and a successor generation produces a stale-generation refusal. Close retains
the lifetime owner and its workspace reservation. Final Stop retains its existing
writer checks against active attempts, process claims and unknown cleanup.

Responses allowlist owner/session/attempt metadata and a durable process-claim
boolean. That boolean supplies neither a fence token nor liveness/readiness
evidence. Private Content and Live reads retain 8192-byte UTF-8 chunks, revision
reset rules and the 60-KiB response bound. Paths, tokens, blob references, profiles,
executables and grants are excluded from requests and normal status output.

Schema 12 is unchanged. The server accepts v1–v13 request frames; legacy chat and
owner operations retain their v11/v12 minimums. Existing v12 binaries reject a
v13 descriptor, so daemon and clients upgrade together. The profile stays
read-only with empty grants. Native transport/UI validation is recorded separately.

Authenticated local Control is the owner API boundary. The daemon derives Git
paths, executables, base commits and receipt tokens from its project and stored
plan. Closed requests prevent clients from supplying those values. Default
status exposes IDs, phase, branch and verification status. Title uses a separate
private-content read, and status/debug output excludes private paths and tokens.

Existing destinations, forged receipts, changed Git identity and outstanding
creation claims are refusal cases. Provisioning is supervised and joined during
shutdown. Unknown outcomes retain their resource reservation. Stop terminalizes
a preparing owner as Run `Failed` and Task `Cancelled`, retaining its candidate
files and branch. Workspace isolation is a Git ownership property. The empty
grant set and pinned read-only profile bound the runtime. Public owner Start/Send
use the closed Control v13 contract and native project-consent guard. The renderer
cannot choose a workspace path, profile or grants.

## Verification boundary

The foundation has independent scoped source review. Native Windows fixtures
exercise real Git creation, restart, claimed-effect recovery, foreign-directory
refusal, exact replay after a cleanup failure and retained files after Stop.
Store checks cover historical migration, rollback, resource reservations,
attempt replay, private records and canonical ownership guards. The attempt
ledger/private writer stage passed ten database regressions, six private-writer
tests and the broader store/daemon suites. Exact commands and results are recorded in
[the execution record](../evidence/native_harness_m1.md#lifetime-owner-workspace-foundation).
Vendor execution, affirmative approvals, success finalization and cross-platform
release qualification remain open.

The internal runtime stage passed 11 attempt-store regressions and three native
service tests covering both synthetic vendors through repeated turns, exact
resume, cancellation, crash and shutdown. Independent scoped source review
passed. The broader daemon run passed 203 library tests with two ignored, then
failed two workflow snapshot cases; the unchanged targeted probe passed later.
This is not a green full-package result. See the
[runtime execution record](../evidence/native_harness_m1.md#owner-runtime-service).

### Completion ownership after a client timeout

The accepted writer operation and its dependent runtime effect have one supervising
actor. A synchronous writer response timeout cannot relinquish that responsibility
while its callback may still commit. Chat writes therefore enqueue once on the
existing bounded writer and await an owned actual-completion channel. Start retains
the fresh launch fence; Send retains the admission result needed to queue input.

Client acknowledgement deadlines remain unchanged and can report uncertainty while
the actor continues. Shutdown checks after the commit release the fresh Start claim
or cancel the Send admission before any vendor effect. Queue rejection happens
before callback/private I/O. The original synchronous writer interface is retained
for its other consumers. No schema, protocol, retry or execution-authority change
is introduced.

A barrier held beyond the handle response deadline reproduced the abandoned result.
The corrected typed admission and fresh-Start-grant tests pass. The final daemon
library run passed 211 tests with two ignored, and the four native execution cases
plus the extended native desktop bridge case passed. These are contained synthetic
vendor fixtures, not live provider or native UI interaction qualification. Exact
commands and retained failures are in the
[public execution record](../evidence/native_harness_m1.md#public-owner-execution).
