# A2A implementation code-review guide

Status: **REVIEW GUIDANCE FOR PROPOSED WORK**, based on source inspection at `04733738ed68ee3d35b95dcf2a92dab71a482478` on 2026-10-26. This guide does not approve a design or assert that a future implementation passed review. The 2026-10-26 refresh re-anchors versions and module paths; the review criteria are unchanged.

Use with the [implementation task cards](a2a_implementation_plan.md), [roadmap](a2a_feature_roadmap.md), [AGENTS.md](../AGENTS.md), [PROGRESS.md](../PROGRESS.md), [architecture](architecture.md) and the accepted ADR for the actual change. Refresh source and status before reviewing. Source paths and test names below are baseline anchors, not promises that future refactors keep them unchanged.

## 1. Review the behavior, not the author's summary

Start with the precise task card, baseline/changed commits, diff, accepted contract, owned files, and commands actually executed. A review request authorizes inspection and findings; it does not authorize silently rewriting the implementation branch.

For each supported entry point, trace:

```text
input and authenticated identity
  -> validation and effective route/configuration
  -> durable admission and association
  -> single-use execution claim / fenced owner
  -> process or peer execution and bounded observations
  -> cancellation / failure / restart reconciliation
  -> artifact and independent acceptance checks
  -> canonical transition plus append-only event
  -> transport/control projection and frontend display
```

Do not accept a handler test as proof of the complete path. Inspect the actual backend used by the advertised server, the writer transaction, process lifecycle and response mapping. A mock `TaskBackend` can demonstrate wire continuation while the production runtime still rejects it.

For a small frontend-only slice, trace the existing dispatch backend (Control v5 operations on the v10 daemon) rather than requiring a new A2A service. For a bridge or persistence change, follow both parent and child ownership through admission, recovery and shutdown.

## 2. Evidence to request from the implementer

| Item | Required content |
| --- | --- |
| Scope | Selected task card, trigger/result, baseline and explicit owned files |
| Decision | Accepted ADR/amendment for changed mapping, schema, events, IPC, artifact or permission semantics |
| Version identity | Changed commit/worktree, relevant SDK/spec/control/store versions and actual toolchain |
| Verification | Exact command, exit/result, executed tests, fixture/binary identity and fresh report |
| Concurrency | Barrier/fault-injection scenario proving one accepted effect and fencing, not timing-based confidence |
| Failure behavior | Actual error/cancel/restart/unknown-outcome path and retained evidence |
| Platform | Native Windows result; Linux/WSL result or explicit unverified status; process descendants and resources joined |
| Privacy | Reviewed outgoing diff/events/logs with placeholder-only examples and sentinel leakage checks |
| Compatibility | Old/new examples, relevant fixtures, migration and downgrade/disable policy |

Use the repository's Section 21 handoff. Missing evidence remains missing. A pass count without the affected scenario, skipped-only invocation, old receipt, initialize-only probe or synthetic fixture does not prove live coding-harness execution.

## 3. Existing contracts that must survive

### 3.1 Single writer and exclusive state owners

Inspect [store/a2a.rs](../crates/vibemux_store/src/a2a.rs), [store/lib.rs](../crates/vibemux_store/src/lib.rs), [dispatch store](../crates/vibemux_store/src/harness_dispatch.rs) and [daemon writer](../crates/vibemuxd/src/lib.rs).

- Only the authoritative daemon writer performs canonical transitions.
- A2A transport, UI, model peers and plugins have no core SQLite connection or alternate writer. The accepted model-peer adapter may read an explicitly selected external CC Switch provider profile through its read-only configuration connection; that does not grant core state authority.
- A2A cannot bind a Run under a dispatch-owned Task. Generic projection updates cannot mutate either owner's entities or create bypass Runs.
- A bridge association has its own accepted semantics and does not erase these guards.
- Admission/state/version/event changes are transactional. Do not hold a transaction across provider, network or process awaits.
- Store sequence numbers are authoritative; adapters cannot allocate canonical event sequence numbers.

Retain `a2a_cannot_bind_a_dispatch_owned_task`, `generic_commits_cannot_touch_dispatch_owned_entities`, and `claim_is_single_use_and_the_fence_stays_out_of_records_and_events`.

**Reject these apparent shortcuts:** dispatch submit followed by direct `start_a2a_run` under the child Task; deleting an ownership check; opening SQLite from a GUI/adapter; fabricating generic projection updates; making a fence part of public output; registering a second process owner.

### 3.2 Three different meanings of completion

Inspect [ADR 024](adr/024_stateful_a2a_supervisor.md), [ADR 029](adr/029_daemon_harness_dispatch.md), [supervisor workflow](../crates/vibemuxd/src/supervisor_workflow.rs) and dispatch outcome mapping.

1. A vendor's structured final record is a protocol observation.
2. A dispatch attempt can finish under ADR 029 and map to its dispatch-owned Task/Run outcome.
3. A supervised parent task is promoted only under its accepted independent-review/local-verifier contract.

Do not impose the supervisor contract retroactively on the existing one-shot dispatch path, or treat dispatch completion as satisfying the supervisor contract automatically. The bridge must define which result it advertises and how promotion occurs.

A requested output schema is not proof of conforming output. Verify actual bytes/schema/hash and task-specific acceptance. The current integer-result verifier is not a generic code verifier. Reviewer identity, base commit, worktree and artifact bindings must remain independent and current. A negative review creates a new repair attempt; do not rewrite the failed attempt into success.

### 3.3 One-shot task UI and continuous coordinator chat

Inspect [ADR 028](adr/028_supervisor_chat_frontend.md), [supervisor state](../crates/vibemux_frontend/src/gui/supervisor_state.rs), [client](../crates/vibemux_client/src/supervisor_client.rs), and [Control dispatch methods](../crates/vibemuxd/src/control_harness_dispatch.rs). The client is GUI-neutral; the egui shell is implemented, and [ADR 043](adr/043_tauri_desktop_app.md) proposes a Tauri replacement that is not implemented.

For `a2a_01_dispatch_ui`, a separately labeled one-shot action uses the existing dispatch methods (Control v5 minimum). The coordinator composer remains disabled until a continuous coordinator contract exists. Setting `send_enabled()` to true, creating a fake reply, or relabeling a one-shot receipt as a chat conversation is not implementation. If ADR 043 is accepted, the GUI-neutral behavior must stay in `vibemux_client`; a second shell-specific client is a defect.

Verify explicit configured harness selection. Model selection currently lives in the route config, not the request. The dispatch catalog fields (`harness`, `protocol`, `enabled`, `allow_execution`, `probe_supported`) do not expose model identity, even whether a model is configured or vendor-default; display that limitation accurately. Model-label display needs a separate accepted safe catalog/API extension. Do not let the frontend read private configuration to fill the gap. Preserve selected profiles and do not add automatic fallback. The catalog also does not attest live readiness and excludes executable paths and private environment names.

Retain the disabled coordinator draft regression while adding one-shot action tests. Keep task-window closure separate from task cancellation and terminal focus separate from process execution.

### 3.4 Cancellation and late results

The current A2A runtime preserves an already completed result when cancellation arrives too late. Dispatch's `cancel_requested` phase maps a subsequent finish to canceled. These are different accepted owner contracts; a bridge must define their reconciliation without falsifying either observation.

Review every cancel edge: before claim; after claim/before launch; during the turn; with a final vendor record; with process exit; after terminal finish; during shutdown; after restart; with late output from an old fence. Intent, owner acknowledgment, process-tree termination and canonical projection are distinct observations.

Do not claim confirmed cancellation because a token was set, an IPC request succeeded, a pane closed, a parent PID vanished or a stream disconnected. A caller disconnect does not implicitly stop an owned task. Unknown/unowned processes must not be killed to make cleanup pass.

Retain `cancel_waiters_are_bounded_pruned_and_late_cancel_preserves_completion`, the dispatch forced/confirmed-cancel tests, descendant containment and joined-shutdown tests.

### 3.5 Persistence and conservative restart recovery

The current inbound A2A runtime is ephemeral; canonical bindings and harness dispatch records are durable. Existing recovery marks unclaimed admitted dispatch attempts Failed/Interrupted, keeps claimed/running or cancel-requested attempts in `recovery_pending`, and quarantines defective records. New inbox persistence must complement that behavior rather than re-launch an interrupted admitted child.

Check durable unique identity by authenticated subject/project/key, canonical request fingerprint, selected route/configuration/model contract and owned association. A lost receipt retry returns the original accepted identity; conflicting reuse fails. Account for a configuration change between admission and retry, without silently resolving a new model.

Acceptance plus reservation/association/event must be atomic, or use an explicitly accepted reconcilable intent protocol. A uniqueness constraint alone does not prevent a second process launch if claims are not fenced. A claimed/lost executor must not be automatically re-executed after restart. Corruption/mismatch stays quarantined, with reservation retained where ownership is uncertain.

Content-free durable identity is not a durable prompt/transcript. If data needed to resume is intentionally absent, show unresumable/unknown output rather than inventing it. Preserve old event bytes and forward migrations; never delete/recreate a user database as a migration fix.

### 3.6 Input checkpoints do not approve tool effects

Inspect the production `TaskExecution`/`TaskExecutor` contract in [task_runtime.rs](../crates/vibemux_a2a/src/task_runtime.rs), server pause mappings, and [protocol_session.rs](../crates/vibemux_harness/src/dispatch/protocol_session.rs).

The existing executor is terminal-only. Deleting the `task_id` rejection does not make it resumable. Require an accepted paused-state/backend contract and a bounded checkpoint with authenticated subject, task/context, kind, expected version, expiry and one-use consumption identity.

Compare-and-set response admission must allow only one next turn, preserve task identity and reject stale/foreign/conflicting/terminal responses. A2A task/context identity is distinct from vendor thread/turn/session identity. A backend that remains one-shot must return unsupported, not silently restart the vendor.

Ordinary input and execution approval are separate contracts. Until a separate approval policy is accepted, keep Codex/Claude permission denial and Agent Client Protocol (ACP) probe-only behavior. This stdio ACP is different from the archived IBM/BeeAI Agent Communication Protocol. Do not auto-approve, synthesize authority from natural-language input, or treat a prior task's approval as standing permission.

### 3.7 Bounded transport, content and artifacts

Use current contract constants, not a new arbitrary limit: A2A application payload 16 KiB; at most 16 artifacts per snapshot; 1..16 parts per message/artifact; internal serialized snapshot/message/list validation ceiling 32 KiB; external transport bodies/protobuf/SSE ceilings 64 KiB; local workspace JSON artifacts 32 KiB; and dispatch prompt 32 KiB. Envelope overhead and escaped serialization still count. A transport ceiling does not override a nested object's smaller budget. Limits may change only under the selected contract, with allocation-before-parse defenses.

Keep SDK types at the adapter boundary. Check unknown enums/required fields, transport-independent errors, numeric equivalence and exact interoperable integer range. Test `1` versus `1.0`, fractional counters, overflow and large integers through JSON-RPC/HTTP+JSON/gRPC as relevant.

Artifact references are not path permission. Hash actual bytes, check owner receipt/Git inventory/base commit/Run, use create-new writes and preserve filename/media/size restrictions. Existing workspace writer allows specific JSON names; new arbitrary filenames require a contract. URL references are not automatically fetched.

Include Windows case/drive/path-prefix/space/Unicode/junction/reparse-point cases. Cleanup must remain plan-first, dry-run/fail-closed and restricted to freshly verified owned clean resources; independent Git worktrees are not an OS security sandbox.

### 3.8 Process containment, async ownership and shutdown

Preserve the daemon's executable validation, fixed protocol argv, environment allowlist, launch trampoline and owned process tree. User prompts enter stdin/typed protocol fields, never shell strings. Pure `vibemux_harness` logic must not gain I/O.

Channels, tasks, subscribers, records, retries and capture are bounded. Do not hold a mutex across `.await`, detach an unsupervised job or inherit the whole daemon environment. Check queue-full, dropped client, hung stdin reader, excess output, deadline, panic isolation and shutdown.

On Windows, verify assigned Job Object/trampoline ownership before releasing the vendor. On POSIX, verify the owned process-group lifecycle. Parent process exit does not prove descendants stopped. Writer shutdown follows execution drain/join; `Drop` is not a verified cleanup receipt.

### 3.9 Visibility without leaking content or changing authority

Audit state events and diagnostic traces have different purposes. Default audit/log records contain bounded identifiers, hashes, sizes and summaries; raw prompts, vendor records, stderr, credential values, full environments and signed/private URLs are excluded.

Use sentinel values in failure-path tests and scan every emitted receipt/event/log/export. Catalog claims and observed compatibility remain separate. Unknown token/cost usage stays unknown. Budget exhaustion cannot silently switch provider/model, increase retries or grant tools. Export errors must not corrupt canonical state.

Remote profiles remain deferred. Reject incidental removal of loopback/allowlist/redirect/proxy controls or new non-loopback bind. Card signatures establish integrity under configured trust, not execution authority or competence.

## 4. Failure-injection review matrix

Choose affected rows for a narrow change. Bridge/inbox/checkpoint work requires the relevant concurrency, crash and authorization rows together. Use deterministic barriers/fault injection instead of sleeps that merely make a race unlikely.

| Scenario | Required observable result | Typical defect to find |
| --- | --- | --- |
| Two simultaneous identical submissions | One accepted identity, reservation and launch; both receipts correlate | Check-then-insert or duplicate spawn after a unique row |
| Same key, changed content/route/model contract | Explicit conflict; original attempt unchanged | Fingerprint omits selection or canonical input |
| Receipt lost after commit | Status/retry recovers original identity without launch | UI generates a new request ID after timeout |
| Same ID guessed by another subject | No task, output, artifact or cancel authority | Filtering list only while Get/Cancel trusts task ID |
| Crash after admission before claim | Existing unclaimed dispatch child fails interrupted; parent reconciles without automatic replay | Treating an interrupted admission as resumable or losing its association |
| Crash after claim or vendor start | Pending/reconciliation, retained ownership; no blind rerun | Treating absence of PID as safe retry |
| Old executor writes after restart | Fence/version rejects result; current record unchanged | Late callback commits stale success |
| Cancel versus completion | Accepted owner-specific result, original observations retained | Unconditionally relabeling completed output |
| Cancel while child ignores protocol | Owned process tree stops or precise failure retained | Only killing the leader process |
| Input checkpoint answered twice | One consumption/next turn; matching replay is idempotent | Two turns share one task/version |
| Foreign, expired or stale checkpoint | Stable rejection with no effect | UI or A2A actor string bypasses authorization |
| Window closed or subscriber disconnected | Execution owner continues unless explicitly canceled | Lifetime tied to presentation/stream |
| Daemon generation changes mid-submit | Stale reply rejected; unknown admission remains visible | Automatic resend to a fresh daemon |
| Oversized or heavily escaped prompt/frame | Bound enforced before unsafe allocation/admission | Character count mistaken for UTF-8/wire bytes |
| Queue/subscriber lag/output cap | Explicit bounded failure/lag; no silent data loss | Unbounded channels or truncated success |
| Raw-output paging | Byte-identical reconstruction or precise unavailable state | JSON reserialization changes records or tail skipped |
| Corrupt/mismatched durable record | Quarantine and retained reservation; other safe work reconciles | Deleting record or clearing busy to proceed |
| Migration interrupted | Transaction rollback and historical bytes preserved | `INSERT OR REPLACE`, recreated DB, rewritten events |
| Artifact hash/path/owner mismatch | Rejection before mutation/read; original remains | Trusting caller hash or string-prefix containment |
| Missing reviewer/verifier or reused receipt | No supervised parent promotion | Vendor Completed treated as verified work |
| Vendor approval or ACP prompt request | Current deny/probe-only contract preserved | Permission downgrade to get a happy-path demo |
| Sentinel secret in provider error/export | Redacted structured diagnostic; no default content leak | Logging request/response/environment on failure |
| SDK protocol/version mismatch | Explicit compatibility result; no silent downgrade | Assuming every A2A framework supports wire 1.0 |
| Trace exporter fails | Task state and writer remain valid | Reporter becomes an authoritative state owner |

## 5. Focused existing regressions

Test names below existed at the inspected baseline. Search them with `rg` and inspect the current assertion before modifying them.

| Area | Tests/anchors to retain |
| --- | --- |
| A2A owner guard | `a2a_cannot_bind_a_dispatch_owned_task` in store A2A unit tests |
| Dispatch ownership/fences | `generic_commits_cannot_touch_dispatch_owned_entities`; `claim_is_single_use_and_the_fence_stays_out_of_records_and_events` |
| Runtime concurrency | `concurrent_idempotency_executes_once_and_conflicting_payload_is_rejected`; retained-task/idempotency limits and numeric identity tests |
| Cancellation/shutdown | `cancel_waiters_are_bounded_pruned_and_late_cancel_preserves_completion`; `shutdown_deadline_aborts_joins_and_publishes_failure`; panic isolation |
| Actual dispatch execution | `concurrent_repeats_of_one_request_launch_one_attempt`; `a_second_request_is_busy_and_a_cancel_without_a_protocol_message_kills_the_tree`; `a_restart_leaves_a_running_attempt_pending_until_acknowledged` |
| Real Control path | `a_dispatch_round_trips_through_control_v5`; `a_running_attempt_reports_live_capture_and_cancels`; `control_shutdown_cancels_a_waiting_probe_promptly` |
| Frontend | `disabled_send_preserves_multiline_cjk_draft`; independent task-window close; bounded action queue and real-daemon reads without execution ownership (the last two moved to `vibemux_client` in the 2026-10-24 extraction) |
| Transactions/migration | `create_duplicate_conflict_and_transaction_rollback_are_atomic`; `v1_migration_preserves_historical_event_bytes_and_projection`; verification rollback; defective-row recovery |
| Workspace | `artifacts_are_immutable_bounded_and_ignored_artifacts_block_cleanup`; owner receipt failure; symlink/junction refusal; unchanged main repo |

Use the [implementation validation commands](a2a_implementation_plan.md#4-existing-validation-commands). Confirm feature-gated daemon tests actually ran with `test_helpers`. A reviewer should inspect negative assertions and real ownership receipts, not merely test names or counts.

## 6. Diff-level review checklist

### Scope and contracts

- [ ] Exactly one selected task or an explicitly justified split is implemented.
- [ ] Accepted contract matches final behavior; proposed decisions are not claimed accepted.
- [ ] Existing SDK pins, permission defaults and provider/model policy are preserved unless explicitly in scope.
- [ ] Schemas/migrations/generated code have one owner; no unrelated lockfile/toolchain/UI-theme changes.
- [ ] Public examples use placeholders; commands match actual CLI/control methods.

### State and concurrency

- [ ] Each Task/Run/process has one lifecycle owner; bridge parent and dispatch child stay distinct.
- [ ] State, version checks, associations and audit events are atomic or covered by accepted reconciliation.
- [ ] Subject/project-scoped deduplication, conflicting retry, fences and lost receipt are tested.
- [ ] Cancellation, terminal preservation, restart and checkpoint races are explicit.
- [ ] No network/process awaits inside database transactions; no mutex held across `.await`.
- [ ] Queues, retry/deadline limits, subscribers and capture remain bounded and owned.

### User-visible behavior

- [ ] Supported one-shot action and unavailable continuous coordinator behavior are labeled accurately.
- [ ] Drafts survive failed/unknown admission; stale replies and duplicate clicks cannot create extra effects.
- [ ] Status, output, cancel confirmation and unavailable recovery/transcript states come from actual data.
- [ ] Independent review/verifier acceptance is shown separately from execution completion.
- [ ] Closing/focusing a window does not infer execution authority or completion.

### Security and platform

- [ ] Caller-supplied identities, cards, artifact URLs and paths cannot grant authority.
- [ ] Cross-subject access to every operation fails; credentials/content remain redacted on failure.
- [ ] Shell-free argv, environment allowlist, trampoline and descendants retain ownership and cleanup.
- [ ] Current read-only/deny, ACP probe-only and remote-disabled defaults survive.
- [ ] Windows path, process/control-runtime and Unicode behavior has relevant evidence.

### Evidence and documentation

- [ ] Changed behavior has decisive happy, boundary, failure and relevant race tests.
- [ ] Feature-gated tests executed; skips/ignored/unavailable tools are labeled accurately.
- [ ] Source/binary/version/platform identity matches the fresh test receipt.
- [ ] A2A changes satisfy `AGENTS.md` Section 10.5, including official TCK for every advertised binding and cross-language ITK scenarios. Custom bilateral checks are supplementary, not full ITK. Missing conformance gates remain explicit limitations and prevent expanded support or milestone-completion claims; this guide grants no exemption.
- [ ] Historical failures remain intact; no weakened tests, overwritten receipts or manufactured success.
- [ ] PROGRESS and compatibility docs reflect only the behavior actually implemented and verified.
- [ ] Final diff/status preserve unrelated work and pass `git diff --check`.

## 7. Finding format and severity

Report actionable defects with location, trigger, consequence and reproducible evidence. Separate a confirmed defect from a plausible design concern or missing runtime evidence.

```text
[P1] Short concrete defect title
Location: <file_and_current_line_or_symbol>
Trigger: <specific_input_interleaving_or_environment>
Observed: <source_trace_or_fresh_reproduction>
Expected contract: <accepted_ADR_or_test>
Consequence: <incorrect_state_effect_or_user_result>
Suggested repair boundary: <smallest_owned_module_contract_change>
Regression scenario: <decisive_success_and_failure_assertions>
Verification status: <inspected_reproduced_or_unverified>
```

- **P0:** immediate severe authority escape, destructive effect or pervasive state corruption in the reviewed supported path.
- **P1:** clear correctness/security/ownership/compatibility defect that blocks the selected behavior or its safe acceptance.
- **P2:** bounded regression, missing material failure handling or diagnostic limitation with a concrete trigger.
- **P3:** maintainability/documentation defect that materially impairs later work; avoid style-only noise.

Missing accepted contracts or decisive evidence can block acceptance without inventing a runtime bug. Do not use severity to compensate for uncertainty. Give a focused reproduction or state exactly which evidence is unavailable.

## 8. Bounded review assignment template

```text
Review only the implementation of task <task_card_id>.
Baseline/diff: <identities>. Accepted decision: <ADR_and_status>.
Use docs/a2a_code_review_guide.md and the selected card, then trace the actual
entrypoint through validation, writer admission, execution owner, failure,
verification and user-visible result. You are read-only; do not rewrite files.
Prioritize actionable correctness, authority, state ownership, concurrency,
compatibility, cancellation/recovery and Windows issues. Preserve other work.
Inspect nearby existing tests and the implementer's exact executed evidence.
Do not run paid inference, writable jobs, deploy, change settings or publish.
Return findings with location, trigger, consequence, accepted contract,
minimal repair boundary and regression scenario. Label uncertainty precisely.
List material evidence gaps and platform exclusions even with no findings.
Do not request unrelated refactors or claim production/full-ITK readiness.
```

After a fix, rerun the smallest decisive regression and affected required gates. Check both the original trigger and new boundary failures. Reuse evidence only when its tested version and inputs still match.

## 9. Acceptance disposition

The final review should state: selected scope; findings and their disposition; checks actually executed; platform/interop coverage; residual risks; unresolved decisions; and the next authorized step. An implementation can be code-complete with live vendor acceptance still unverified. That distinction must remain visible.

Do not merge, commit, push, deploy, enable remote access or run live providers merely because the review has no findings. Those stages retain their own authorization and evidence requirements.
