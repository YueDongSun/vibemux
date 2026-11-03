# ADR 059: Interactive chat tool approvals

Status: Proposed, 2026-11-02. This contract guides remaining profile and native qualification work.
The schema-13 store ledger, protected private-evidence writer, private
write/flush receipts, guarded fixture runtime, Control v14 approval consumer and
mounted desktop review surface are implemented. Production writable profiles,
installed-vendor affirmative delivery and native qualification remain pending.
Existing read-only conversations remain the supported production profile.

Extends [ADR 058](058_writable_chat_ownership.md) and preserves the authority
requirements in [ADR 036](036_runtime_execution_authority.md).

## Context

Lifetime owners and attempt admission now retain actual writer completion through
runtime dispatch. Their grants remain empty. Both persistent adapters deny vendor
permission requests. The legacy input writer proves queue acceptance; its new
private tracked receipt separately proves per-frame write/flush completion.

The first interactive file profile will target Claude Code. Its explicit tool
list can represent independent ReadFiles, SearchFiles, EditFiles and WriteFiles
grants. RunDevTests stays unsupported until a separate exact command policy is
enforceable. Codex native workspace-write remains refused under ADR 036: an
approval-response frame does not establish the five abstract tool restrictions.

## Profile and admitted authority

A new owner explicitly admits a sorted tool set and a pinned interactive profile.
It cannot widen an existing owner's immutable Open binding. Each submission may
narrow the owner's grants; every permission decision checks that attempt's set.
The legacy owner Open and Send retain their empty-grant behavior.

The candidate Claude 2.1.304 profile uses the installed CLI's advertised `manual`
permission mode and `host` permission prompts. It retains restricted mode, strict
MCP isolation and the existing customization exclusions, names only the admitted
file tools, and supplies ASK rules for those tools. It supplies no pre-allowed
mutation, bypass, auto, acceptEdits, shell, network, delegation or policy-amendment
capability. A positive reply never contains updatedPermissions or changes input.
The first file effect uses Write; Edit and the independent read/search mappings
use the same authority boundary when their qualification passes.

Qualification binds the exact executable/version, launch arguments, supplied
permission settings, admitted tool mapping and all surviving effective policy
sources. Restricted/safe mode does not imply that managed settings disappear.
The profile must exclude or explicitly qualify surviving rules, hooks, plugins
and capabilities; unknown effective sources refuse launch. A relevant policy
change invalidates the qualification and process generation. Provider credentials
are neither qualification evidence nor exported policy content.

This is a candidate profile, not a certification. Production launch must refuse it
until installed-version evidence establishes denial, correlation, ordering and
cancellation behavior. A nonserializable test permit may authorize exact copies
of the contained fixture. A JSON fixture label or a passing synthetic score cannot
authorize a real vendor. CLI flags are not an OS sandbox.

## Exact challenge and private evidence

An approval binds project, owner Task/Run, chat, attempt/request, physical process
generation and fence, admitted profile, effective tools, vendor request ID,
tool-use ID, tool name, exact input fingerprint and expiry. Only one pending
challenge is admitted per attempt initially; additional requests are denied.

Request ID identifies an RPC, not a user turn. For persistent or resumed native
sessions, before offering an approval, the adapter must associate the permission request with complete root tool-use
evidence for the active user input and bound session. Name, tool-use ID and parsed
input must match. Agent/subagent, parent-tool, foreign-session, retired and
conflicting identities are refusal cases. An early request may be buffered within
existing frame/byte limits, while output reading and cancellation continue.

Installed-version qualification must prove that this evidence is available before
the permission reply is needed. Waiting for a later assistant record would
deadlock if the CLI emits it only after approval. Complete streamed tool input
may be usable only with proven identity/order. If exact association cannot be
established, affirmative delivery remains disabled; the implementation must not
substitute the current active turn for missing evidence.

Full input, edit/file content and explanations use the immutable private-body
channel. Default DTOs, events and Debug expose canonical IDs, tool kind, byte
counts, state, version and expiry. Explicit content reads use bounded UTF-8 chunks.
The renderer cannot supply paths, vendor IDs, input, grants, profiles or wire bytes
when deciding an existing challenge. Input and paths remain data; validation also
retains Windows containment, reparse, reserved-metadata and workspace checks.

## Candidate fresh single-input origin

A separately qualified profile may establish canonical attempt origin by a
closed fresh process, rather than requiring an assistant echo that the installed
protocol might not provide before permission. This is a proposed alternative,
not a relaxation of the existing persistent-session parser or a certification.

The process has a fresh, never-resumed native identity and exact owned pipes,
generation and fence. It accepts exactly one typed, durably claimed user input
and permanently refuses another, including after a terminal result. Its write
and flush must be observed. Uncertain delivery is never retried. It imports no
native history, permission frames, fork, rewind, resume or other input channel.
One canonical input can cause several model/agent steps; this establishes
attempt origin, not a one-model-call budget.

Qualification must establish closure: no autonomous pre-input or recovered work,
background/root work, delegation, hooks, MCP, plugins or surviving unqualified
policy source. Fresh UUIDs and a root init alone do not establish that property.
The exact executable/version, policy sources and native root/nested semantics
remain bound to the profile. A complete root tool-use ID/name/input must still
match its permission request on the same owned pipe. Human AllowOnce, path
policy, immutable private evidence and the one-use delivery cutoff still apply.
An optional user replay remains an observation rather than standalone tool proof.

Canonical chat history, the lifetime owner and worktree can survive native
process rotation. This needs an explicit native-incarnation/history mapping and
profile contract; old native IDs and exact Resume semantics must stay intact.
Any prior context travels only as bounded literal text inside the next sole
input, with provenance and private-data handling. Serialized native tool or
permission history is never replayed as control. The mapping and product flow
are not implemented by this increment.

The test-only diagnostic observer distinguishes raw root tool/request matches
from current-input proof. A missing echo remains visible without acquiring
proof; an unrelated echoed message cannot bind that tool. A final assistant
snapshot may confirm its own streamed tool only when it includes the same tool
exactly once with unchanged input. Proof is frozen when the parent selects a
denial. The observer remains pure. A separate `test_helpers` diagnostic runner
now drives it over owned pipes and contained processes. Two separately authorized
installed-Claude 2.1.304 diagnostics rejected initialization mismatches; the v2
report isolated the mode comparison. Closed-origin and production-profile
qualification remain pending.

## Deny-only qualification diagnostic

`DiagnosticCandidate` prepares a fixed Claude 2.1.304 candidate without starting
an executable or reading credentials. It creates a fresh empty diagnostic
namespace, pins the vendor and built trampoline, and retains exact source and
workspace identity through launch. Native execution separately verifies bounded
`--version` output in a contained process before sending any model input.

The driver reuses the normal machine's exact initialization response handling
and generated user frame, then routes raw observations to the diagnostic
observer. It submits at most one fixed user input and constructs only a fixed
deny response. Independent write/flush receipts, output reading, cancellation,
root-liveness checks and absolute deadlines remain active together. A terminal
observation waits for the required input and denial receipts. All normal stops
join the input owner before contained process cleanup. An already-cancelled or
closed cancellation owner refuses before process acquisition.

The candidate requests manual/host permissions, Write only, ASK for Write,
restricted/safe mode, strict MCP, disabled Chrome and slash commands, and no
session persistence. It supplies neither pre-allowed tools nor permission
updates. Environment inheritance is limited to the four system names and the
explicitly selected provider-name allowlist. This is a diagnostic candidate;
the report always marks policy-source and production-profile qualification
false. `--max-turns 1` is a requested native limit, not proof of one model call.

The ordinary test target runs guarded synthetic processes. The separate
`installed_claude_deny_only_diagnostic` case is ignored and requires an explicit
live allowance, exact native executable, new recovery namespace, preserved
provider environment and review of surviving policy sources. It prints fixed
observations and cleanup evidence without raw frames, paths, input or provider
values. Unknown files or uncertain cleanup retain the namespace. No production
service, Control operation or renderer selects this test-only API.

Permission callbacks can be skipped by an earlier allow rule, and managed
settings survive the candidate's customization exclusions. The local file and
registry inventory alone does not qualify remote or runtime sources. See
[permission evaluation](https://code.claude.com/docs/en/agent-sdk/user-input)
and [managed source precedence](https://code.claude.com/docs/en/managed-settings).
The executed synthetic checks and remaining native evidence are recorded in the
[diagnostic execution record](../evidence/native_harness_m1.md#deny-only-approval-diagnostic).

Diagnostic report v2 adds `initialize_accepted`, `input_queued` and
`denial_queued`, separately from completed write/flush receipts. Optional
`init_checks` records only whether the exact manual mode, working directory and
single Write tool list matched. `None` means these checks were not reached.
Root identity and duplicate-initialization checks precede these comparisons;
all three comparisons are captured before a mismatch rejects the record. No
private field value is retained in the report, and acceptance remains strict.
A queued input may have reached the peer without a flush receipt, so a false
`input_written` does not authorize retry or replenish a live allowance. The
[installed observation](../evidence/native_harness_m1.md#installed-claude-diagnostic-and-redacted-init-checks)
used report v1 and cannot retrospectively identify the mismatched field.

### Canonical Manual mode in diagnostic report v3

The CLI spelling and SDK wire value are distinct. The official
[CLI reference](https://code.claude.com/docs/en/cli-reference) documents
`manual` as an alias for `default` from Claude Code 2.1.200 onward; help lists
the alias. The [permission-mode guide](https://code.claude.com/docs/en/permission-modes)
names `default` as the config/SDK value, and the
[SDK reference](https://code.claude.com/docs/en/agent-sdk/typescript#permissionmode)
types `SDKSystemMessage.permissionMode` with the SDK enum containing `default`.
These primary sources and the installed 2.1.304 help were checked on 2026-11-02.

The test-only observer now accepts exactly the JSON strings `default` and
`manual` for the effective Manual-mode comparison. The latter retains the
existing synthetic fixture spelling. Other modes, missing/non-string fields,
case variants and whitespace changes remain rejected. Identity and duplicate
checks still precede the exact working-directory and one-Write-tool checks.
Correlation and fixed Deny delivery are unchanged; no affirmative path is added.
The production `dontAsk` parser, guarded fixture state machine, launch arguments,
grants and production-profile admission are unchanged.

Report version 3 retains the same fields while distinguishing the corrected
meaning of `permission_mode_matches`. Historical v1/v2 output is not rewritten
or reinterpreted. The v2 live result established a rejected mode comparison,
but discarded the actual field value; it cannot prove that value was `default`.
The documented mapping identifies a source compatibility defect and a plausible
explanation for the live rejection, rather than a verified native replay.

This report-only compatibility change needs no Control/store migration. Its
threat boundary remains the test-helper deny-only observer: malformed or foreign
input cannot gain a response identity, and no mode spelling grants tool or
production authority. Offline regressions and remaining live limits are in the
[canonical-mode repair record](../evidence/native_harness_m1.md#canonical-manual-mode-diagnostic-repair).

## Decision and one-use delivery

The public choices are Allow once and Deny. Persist the exact user operation,
challenge version, decision and one-use delivery claim atomically before any
response can enter stdin. A replay returns its receipt and no new send claim.
Changed reuse conflicts. Cancellation, expiry, generation, profile, workspace and
effective grants are rechecked by the atomic claim. That claim is the irrevocable
delivery-authorization cutoff. Cancellation committed first prevents the claim;
cancellation committed later interrupts the runtime but cannot promise prevention
of that already-authorized file effect. Before writing, the runtime still checks
the exact owned process/generation and pending request binding. Queue entry alone
does not revoke or extend the authorization.

Decision and transport outcome are separate. Pending means no decision; Claimed
means a response may be sent; Written means all bytes were written and flushed to
the owned pipe. Neither queue acceptance nor Written proves vendor acceptance or
a successful tool effect. Partial write, dropped observation, timeout or crash
after claim retains Unknown and never creates another affirmative response.

The input writer therefore needs an owned per-frame completion receipt. Its
supervisor retains that receipt across client acknowledgement timeouts, with
cancellation/join ownership and explicit unknown outcomes. The actor never waits
for a person inside frame application or a database transaction.

A committed cancellation or expiry invalidates unclaimed affirmative delivery.
An already claimed response remains unknown until matching evidence resolves it;
it cannot be rewritten as a denial. Restart invalidates pending approvals and
fences claimed outcomes to the old generation. A terminal turn must settle its
approval obligations or enter recovery. Tool result, attempt completion and owner
success finalization remain distinct evidence.

The initial approval lifetime is 120 seconds, capped by the remaining turn
deadline; the existing turn clock keeps running while a person decides. Persist
UTC expiry and use a matching monotonic deadline in the live runtime. Expiry
before claim commits Expired, then interrupts and tears down the exact owned
process so the CLI cannot wait indefinitely. Unconfirmed interruption follows
the existing bounded containment cleanup and recovery path. Expiry after claim
uses the same interrupt/cleanup path but retains the decision and claimed/unknown
delivery state. It cannot relabel an authorized in-flight response as denied.
The ordinary output-idle timer is suspended only while a validated, persisted
challenge awaits a person; approval expiry and the total turn deadline remain
active. Early uncorrelated evidence does not suspend idle detection. A written
decision restarts ordinary idle observation without resetting the turn budget.
Written response completion retires the approval timer. A validated matching
terminal tool result may also settle that obligation if the write observer was
lost. Neither event resets the total turn clock. Tool execution after a written
decision remains bounded by ordinary idle/turn supervision, rather than a second
120-second approval deadline.

## Guarded fixture runtime

The test-only factory now connects the protocol, private writer and actor to an
actual contained fixture process over a real managed owner worktree. It admits
only a fresh Claude fixture session with WriteFiles, one user input and a fixed
create-new artifact. The normal constructor and service retain their read-only
profiles; no serialized flag, Control operation or renderer path selects this
fixture mode.

The actor waits for the exact input's write/flush receipt before publishing a
correlated challenge. It prepares a response once, obtains the canonical
one-use dispatch capability, and rechecks workspace HEAD, ownership, cancellation,
root process and absolute deadlines before enqueue. Owned futures retain callback
completion after the client observer disappears. Output reading and local
cancellation continue while database callbacks wait. A human wait pauses idle
expiry; the 120-second approval deadline is capped by the remaining turn budget.
Pipe Written retires approval expiry, and durable Written releases buffered
terminal evidence. Lost or refused delivery retains Unknown and is never resent.

The launch seam requires the fixture guard for this protocol mode. The factory
pins the actual built fixture vendor and trampoline, exact configuration and CWD,
and managed worktree receipt/base identity. The fixed effect uses `create_new`
on a relative filename after an exact matching allow response. These checks do
not establish a hostile same-user filesystem sandbox or qualify a native vendor.
The ten-case Windows matrix and its remaining limits are recorded in the
[execution evidence](../evidence/native_harness_m1.md#guarded-fixture-approval-runtime).

## Compatibility and implementation sequence

The schema-13 store increment now stores chat-bound challenges, private references,
decisions and delivery receipts under the existing owner/attempt foreign keys.
The migration is transactional and forward-only, preserves historical bytes and
adds request/operation collision and replay checks. Existing private quotas apply.
`chat_approvals` retains the challenge and one-use outbox metadata; the existing
`chat_operations` table retains the immutable user decision receipt. A fresh
committed decision alone returns a privately constructed, non-Clone and
nonserializable delivery capability. The store accepts only the four Claude file
grants, bounds challenges to 4096 per chat, and validates persisted bindings on
read. It records at most 120 seconds of UTC lifetime and a per-chat event-ordered
clock watermark. The guarded fixture runtime now applies the remaining-turn cap,
monotonic wait and adapter evidence validation. The private writer publishes
immutable evidence before metadata, checks exact replay before filesystem I/O,
and validates the owned body and raw vendor-ID bindings before a fresh decision.
It transfers a verified snapshot with the unique delivery claim to the actor.
Production profile integration remains pending. Control and the renderer use
explicit DTOs and the owned service; they never receive store rows, private
dispatch capabilities or direct database access.

Control v14 now adds the closed `chat_approval` operation for pending and metadata
queries, explicit private JSON-input chunks, a human decision and receipt lookup.
The decision carries canonical IDs, generation, request, expected version, one
operation ID and AllowOnce/Deny. It supplies no paths, vendor IDs, profile, fence,
grants or replacement input. Explicit owner/grant admission and narrowed writable
Send remain future work behind installed-profile qualification.
Existing chat, owner and execution operations retain v11/v12/v13 minima and shapes.
Clients built before v14 reject the newer descriptor, so daemon, CLI and desktop
upgrade together. This surface changes no storage schema; the prior schema-13
migration and its matching-backup rollback requirements retain their scope.

The native `chat_approval_request` command is the 29th fixed command. The mounted
Workspace approval controller binds every response to project/chat/generation/
request/approval/version. Review explicitly reads the input as bounded UTF-8 JSON
and renders plain text. Allow once requires complete current content and a live,
unexpired offer; Deny is an explicit choice. Opening, closing, polling, expiry and
navigation make no decision. The dialog initially focuses Deny.

The service obtains its owned runtime handle through the serialized registry,
then awaits the decision outside that queue. The native bridge also avoids the
project-exclusive daemon job while waiting, so cancellation can progress. The
runtime and writer remain the decision/delivery owners if a client stops waiting.
Read-only receipt lookup compares the original decision binding and returns its
original operation sequence with current delivery metadata, without body I/O,
clock changes, events or a delivery claim. An uncertain UI choice keeps its exact
payload until a matching receipt or newer final canonical noncommit evidence
resolves it. Recovery sends no replacement decision.

UI metadata applies against the current row, so a delayed response cannot roll
back newer delivery evidence. A detected local-clock rollback clears the private
review and blocks choices until fresh server calibration; it does not fabricate
canonical expiry. These source and contained-process checks are recorded in the
[surface execution record](../evidence/native_harness_m1.md#control-v14-and-desktop-tool-approvals).

The renderer is an untrusted caller of this boundary. Native project identity
and Control authentication precede dispatch; decisions accept only canonical
binding fields and a human choice. The writer and runtime enforce the existing
generation/fence, grants, source, cancellation, expiry and one-use delivery
rules. Tool input remains private display data, rendered as text and never
interpolated into commands. Malformed or mismatched content and stale replies
cannot acquire authority. The fixture path does not establish a native policy
boundary; production profiles remain refused until separately qualified.

Implementation proceeds through the actual effect boundary: a contained Claude
fixture waits on one exact permission response before writing its test artifact;
then pure protocol correlation and bounded pending state; durable writer delivery;
runtime write/flush acknowledgement; authenticated control; native approval UI;
and installed-version qualification. Read-only regression coverage stays intact.

Required cases include absent/denied approval, foreign and nested identities,
input changes, duplicate decision/response, late/cancelled/expired approval,
restart, partial write, lost receipt, workspace/profile change, private-content
limits, and retained lifetime identity. No effect occurs before the matching
one-use affirmative response. A fixture pass does not promote a live profile.

## Protocol grounding and limits

The installed Claude Code 2.1.304 help was inspected for manual/host/restricted
flags. The official SDK describes permission evaluation and the canUseTool
callback, and its source constructs allow replies from the original input.
These sources establish candidate shapes; they do not qualify this host/version's
event order. See [permission evaluation](https://code.claude.com/docs/en/agent-sdk/permissions),
[approval handling](https://code.claude.com/docs/en/agent-sdk/user-input), and
[the SDK control implementation](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/query.py).

The Codex 0.166.0 family schema generator and
[app-server approval documentation](https://learn.chatgpt.com/docs/app-server#approvals)
were also inspected. Their approval decisions remain denied by the current
persistent adapter. They supply no proof of the abstract grant enforcement
required by ADR 036.
