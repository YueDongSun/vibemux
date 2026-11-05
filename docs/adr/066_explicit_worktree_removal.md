# ADR 066: Explicit owner worktree removal

- Status: Accepted for the bounded Windows implementation; synthetic native review/Keep verified, native removal and broader platform acceptance remain open
- Date: 2026-11-03
- Related: [ADR 058](058_writable_chat_ownership.md), [ADR 064](064_task_terminal_observation.md), [ADR 065](065_native_links_and_worktree_access.md), [ADR 067](067_terminal_cleanup_inventory.md)

## Context

Stop finalizes a Workspace owner and retains its files, branch and history.
`WorkspaceManager::plan_cleanup` already checks managed ownership and refuses
tracked, untracked and ignored changes; `cleanup` repeats checks and uses
`git worktree remove --` without force or branch deletion. Before this change,
the desktop lacked a reviewed removal plan, durable effect ownership and
unknown-outcome recovery.

Current terminal `inventory_v1` matches only the exact directory and can omit a
pane in a child directory. Its ephemeral observation leases, expiry and Unlink
do not prove absence. The existing Open-only terminal wrapper also cannot grant
removal authority for a Finalized owner.

## Accepted private store foundation

Schema 14 adds `workspace_cleanups` and its command index in one migration
transaction. It retains the historical owner/session/Task/Run unchanged. The
private Rust boundary records inert, five-minute previews and atomically claims
their exact versions, live incarnation and canonical settled Finalized binding.
Confirm reserves resource, normalized path and recorded branch. Fresh harness,
chat, owner, workflow-lease and A2A admission check the reciprocal reservation.
Cleanup also refuses an idle read-only chat process claim, Active or Quarantined
workflow lease, or any conflicting A2A binding. Replay precedes fresh admission
checks and returns status without another execution capability.

Only fresh Confirm returns a non-Clone, nonserializable execution claim. Consuming
it can retire an unused spawn path, or split registration and completion authority
for one executor nonce before any child-producing attempt. Registration returns
Arm authority; a fresh committed Arm alone returns the one-use release permit.
Completion survives lost registration/Arm replies and can atomically record the
same executor binding with joined evidence. Its local retirement fence revokes
late registration, Arm and release deliveries before evidence persistence begins.
A definite persistence error returns only the already-retired completion or
never-spawned proof for a safe retry; an uncertain reply requires a status read
first. No retry, read or reconciliation recreates spawn or release authority.

Joined evidence remains private, trusted-producer input. All required termination,
root-reap, tree-empty and pipe-drain observations must be positive before terminal
settlement. Unknown retains its reservation, including when executor fields are
absent. Explicit reconciliation can use an already-durable positive receipt and
fresh postcondition verdict to settle `RemovedObserved` or `Refused`; it cannot
attribute an unobserved Git invocation. `RemovedExecuted` additionally requires
the matching successful fixed-profile result and durable Arm. Removed/Refused
receipts are immutable. Removed is a historical owner-Run tombstone. Unrelated
future bindings retain their ordinary admission policy. Current directory reads
refuse an active cleanup or that Run's Removed tombstone both before and after
physical inspection; branch metadata remains readable.

Private records are bounded to 64 KiB before SQL retrieval/JSON decoding, with
strict validation against indexed columns. Events contain IDs, phase, version
and fixed outcome/error metadata, excluding paths, tokens and file identities.
The identity format preserves all 128 file-ID bits and the full volume serial
from [ADR 068](068_windows_file_identity.md). Serializable metadata itself grants
no process or filesystem authority.

The store is now connected to the supervised Windows service, Control v16 and
Main-only native command 32. Local owned-repository tests exercise the real Git
effect, retained branch/history, refusal after review changes, dropped waiters
and evidence-only restart reconciliation. Independent source review covers the
runtime and native/UI boundary. Native desktop interaction and live WezTerm,
Windows 10, Linux/WSL and additional-filesystem qualification remain open.

## Service decision

### Separate plan, claim and outcome

Add a schema-14 cleanup ledger without changing the historical owner or its
Finalized phase. Persist a nonreserving preview with a non-nil plan ID/version,
canonical Project/Chat/Task/Run binding, owner/session versions, complete private
workspace receipt, resource key, repository/common-directory identity, recorded
branch and observed branch HEAD. A committed HEAD may differ from the original
base; the retained branch preserves those commits. Preview expiry is bounded
and server-owned; restart or unavailable expiry authority requires a fresh plan.

Only a fully loaded Finalized, verified, settled, quiescent owner is eligible.
Recovery uncertainty, process claims, active requests, conflicting reservations,
dirty/ignored files, missing ownership or mismatched physical Run slots refuse
planning. A preview never reserves or removes anything.

Confirm supplies only the exact plan/version and confirmation operation ID.
One writer transaction rechecks the complete canonical binding, staleness and
conflicting reservations, consumes the preview once and creates the durable
cleanup claim/resource reservation with its audit event. Reciprocal dispatch
and execution admission checks consult that reservation in their own writer
transactions. Duplicate Confirm returns the original receipt/status and never
starts another effect.

The service owns the executor beyond a Control/Tauri request or window lifetime.
Under the cleanup reservation and exclusive daemon resource gate, it repeats
authority, physical, clean-status and terminal checks and compares the approved
HEAD/binding. Changed identity or proof scope requires a new plan. No writer
transaction or registry mutex spans filesystem, plugin or Git awaits. The gate
remains held through the owned Git operation, join and durable settlement.

### Complete terminal observation

The read-only observation foundation has its own accepted scope in ADR 067.
Its implementation does not establish the removal lifecycle described here.

Add the distinct negotiated capability `terminal:cleanup_inventory_v1`, reusing
the existing `TerminalInventoryRequest` and `TerminalInventory` payload types.
It validates the complete bounded source inventory before filtering and covers
the root and all real descendant directory identities. Unknown/unresolvable or
unsupported remote/UNC cwd, duplicate IDs, malformed data, capacity failure,
unavailable capability and plugin/backend session changes refuse a clear result.
The full source bound is separate from the existing four matched-pane response
bound; neither collection nor matching may silently truncate.

Confirm obtains a fresh complete result from the configured active observer
instance while holding the exclusive resource gate. Any matching pane or active
Run lease blocks removal. Ordinary inspect/link/focus use a shared gate through
their plugin work and retain their existing semantics. Recheck the exact backend
instance/plugin session immediately before Git admission. An older observer or
an unavailable plugin never falls back to ordinary inventory or an empty lease
map. This proof covers the configured supported terminal scope, not every
terminal or arbitrary external process on the host.

### Results and explicit reconciliation

Success requires the planned repository identity, absence of the target path
and its Git worktree inventory entry, and the retained branch at the approved
commit. A nonzero exit, timeout, partial effect or contradictory postcondition
cannot become success. Removed results remain bound to the historical workspace
even if something later appears at its former path.

Lost replies, daemon restart and unknown executor cleanup retain the durable
reservation. Status is read-only. Explicit Reconcile inspects filesystem/process
evidence without deletion and may commit a canonical result with its audit event;
it never replays deletion. Reconciliation may report verified removal observed only after executor
quiescence and all postconditions are proved; it does not invent a successful
unobserved Git invocation. An unchanged remaining path or missing parent PID
alone cannot prove an older executor is unable to act later. Known refusal
before an effect can settle without claiming removal. A fresh attempt requires
a fresh reviewed plan and operation ID.

The accepted Windows executor uses the held launch and evidence custodian below.
The existing workspace Git runner has a bounded child and reap path, but does
not provide the durable cross-restart process-tree receipt this service needs.
Existing platform containment is reused;
POSIX groups have no Windows kill-on-close guarantee. Unknown recovery must
remain fenced wherever the required proof is unavailable.

### Executor release barrier

The additive contained-launch seam is now implemented and locally verified:
`spawn_held` returns a private-pipe owner, and consuming `release_after` awaits a
trusted caller callback before its single go-byte attempt. Opaque failures retain
ownership for explicit joined cleanup and expose no release retry. Authorization
denial closes stdin to the unreleased trampoline; transport failure remains a
release attempt that may have started the program. Drop/future abort supplies no
joined receipt. The callback PID is not durable identity. Six explicitly selected
Windows synthetic cases verify this seam, including separate stdout completion.
This launch seam supplies no identity capture or removal permission. The private
store foundation above records Arm/quiescence. Its Windows runtime producer now
captures actual image, held-process and joined receipt evidence; the initial
launch-seam checks remain a separate earlier verification stage. See the
[execution record](../evidence/native_harness_m1.md#held-native-process-release-boundary).

Reuse the existing platform `ProcessTree` and launch trampoline, with a narrow
held-spawn/release seam. The existing `NativeProcess::spawn` auto-releases the go
byte and cannot be used unchanged for this barrier. Claim/fence first, spawn
held, contain, then durably record the exact executor nonce, daemon incarnation,
process creation identity and successful containment before sending go. A failed
or uncertain durable acknowledgement grants no release. An armed record means
Git may have run, including when a later pipe write reports failure.

The service retains the child/tree, bounded pipe drains, deadline/cancellation
and join independently of a client. Persist positive quiescence evidence before
or atomically with settlement: required termination/tree-empty, root-reap and
pipe-drain evidence must all be present. Reconciliation can then repeat only
postcondition reads if reply delivery or final settlement was interrupted.

| Durable evidence | Supported recovery |
|---|---|
| Unconsumed preview | Invalidate across restart; create a fresh plan. |
| Proven pre-release no-effect outcome | Return the refusal; no replay. |
| Positive quiescence receipt, result missing | Explicitly recheck postconditions and settle without execution. |
| Release may have occurred, no positive quiescence receipt | Keep Unknown and the reservation; no deletion or admission release. |
| Terminal result | Return its exact retained receipt; no replay. |

Current Windows Jobs are unnamed/non-inheritable and cannot be reopened through
the existing API after restart. POSIX groups lack its kill-on-close guarantee
and the current positive-empty receipt. Missing PIDs, elapsed time or daemon
death do not supply that evidence. The initial supported executor must be
qualified per platform before admitting an effect. Recovery from every crash
window would require an additional tested containment/quiescence authority;
this proposal does not claim it exists.

### Native/UI boundary and presentation

Add Control v16 `workspace_cleanup` with closed Plan/Confirm/Status/Reconcile
requests, preserving older operation minima. Add the main-only native command
`workspace_cleanup_request` as command 32. Renderer input contains IDs and
plan/version references, with no path, force flag or terminal-unavailable bypass.
The review shows the canonical target, recorded branch and retention consequence.
UI uncertainty retains the original request and uses read-only Status recovery;
any canonical reconciliation is a separate explicit operation.
Task children retain only their existing read/presentation capability.

The desktop serializes its own folder/path/link and cleanup jobs within one
AppState. Core reservations/gates block new conflicting admissions and directory
inspections. `workspace_read` v15 remains a fresh read without a directory-lifetime
lease: data already delivered to another client cannot be revoked. Previously
admitted native handoffs may overlap removal. Windows no-delete presentation
pins protect their platform-specific interval; POSIX pins do not prevent unlink.
`OpenRequested` acknowledges handoff, not directory survival until an external
application displays it. This retains ADR 065's trusted-operator/OS-resolution
limit and adds no cross-client presentation-lease promise. Removal must not hold
a leaf or Git administrative handle that denies the deletion it intends to make.

## Alternatives and consequences

The implementation uses one bounded daemon job owner for accepted Directory,
terminal and cleanup continuations. Capacity is acquired before waiting for a
resource gate; an RPC/window waiter cannot cancel the owned effect. Ordinary
observations use shared gates and cleanup retains the exclusive gate through
final checks, release, joined evidence and settlement. Shutdown stops admission,
joins these jobs while dependencies remain available, then flushes pending
evidence before the writer closes.

The fixed Git command uses retained executable and ancestor handles denying
WRITE and DELETE sharing. Root-process audit compares the live held trampoline
with that pinned object; start-time seconds remain audit metadata, never restart
or signalling authority. Repository/common/managed-directory pins and full-width
IDs bind physical reads. The worktree leaf's no-delete handle is released before
Go so it does not block the intended Git removal. No-force argv, cleared child
environment, disabled hooks/fsmonitor and ignored-file checks remain fixed.

A bounded per-claim evidence custodian owns retired proof before any persistence
read/enqueue. Rejected queue admission cannot consume it; the writer takes it
only when executing and returns it on a failed store operation. Bounded retries
retain pending custody for explicit reconciliation and shutdown flushing. Status
does not flush or write. The evidence flush applies one absolute deadline to
batch admission, response waits and retry delays; expiry retains uncertainty.
Accepted writer work still owns its Arc. Writer shutdown/join has its separate
ownership contract and is not bounded by this flush timer.

Verified stderr EOF is distinct from read failure or task completion. Terminal
results require actual termination, root reap, tree emptiness and both pipe EOF
observations. An unavailable proof retains Unknown. Complete terminal inventory
is always required; neither a lease expiry nor a possibly late Focus response
proves absence. Legacy Focus is not owned by the cleanup worker's busy slot, so
the implementation makes no claim that every earlier external CLI effect ended.

Reusing Stop would break its retention promise. Reusing a path-only cleanup call
or expired terminal lease would omit canonical authority and outcome recovery.
An ephemeral cleanup job would lose effect ownership on request cancellation.
The separate ledger preserves these responsibilities and requires a forward
migration, explicit compatibility checks and a new native capability boundary.

A durable cross-client presentation lease would add actor/job identity and
unresolved-lease reconciliation. It is unnecessary for the stated browsing
contract and still could not prove that a reused external app finished resolving
a pathname. It would be required for a future stronger exclusion guarantee.

## Verification and threat model

Test plan-only behavior, exact request replay/conflict, stale HEAD/binding/expiry,
concurrent claims and reciprocal admissions, root/subdirectory/alias terminals,
unknown or oversized inventories, dirty/ignored data, reparse/foreign/main
targets, response loss, interruption/restart and contradictory postconditions.
Verify branch and main-repository integrity after isolated synthetic removals.
Use fake terminal services and owned process fixtures before native acceptance.
No implementation test may delete a real user workspace or bypass an unavailable
observer. Native desktop input and installed-provider qualification retain their
separate authorization boundaries.

The authoritative Git interface is
[git-worktree](https://git-scm.com/docs/git-worktree). This design uses explicit
worktree removal and preserves Git's clean-worktree refusal; repository checks
also protect ignored artifacts. Public/native support remains unverified until
the required lifecycle, compatibility, platform and failure checks execute.

A bounded Windows 11 synthetic native check verified review/Keep/review for a
Finalized owner, a matching Task child without removal controls and joined
shutdown. It performed no removal. The exact earlier QA image and scope are in
[the observation record](../evidence/native_harness_m1.md#native-cleanup-review-observation);
they do not establish installed terminal or native deletion acceptance.
