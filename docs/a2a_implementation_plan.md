# A2A implementation plan

Status: **PROPOSED**, based on source inspection at `04733738ed68ee3d35b95dcf2a92dab71a482478` on 2026-10-26. No task below is implemented or authorized for execution merely by this document. [AGENTS.md](../AGENTS.md), [PROGRESS.md](../PROGRESS.md), accepted ADRs and actual source remain authoritative. This refresh re-anchors the baseline, module paths and versions; the proposed task cards are unchanged.

Read the [feature roadmap](a2a_feature_roadmap.md) for priorities and the [review guide](a2a_code_review_guide.md) before changing code. Use one task card per bounded implementation issue; split a card further when it spans an independently reviewable contract, migration or runtime change.

## 1. Contract and baseline capsule

Before each task, record the actual branch, HEAD, status, selected card, file ownership, relevant accepted ADR and nearby tests. Preserve unrelated changes. Use separate worktrees/branches for implementation agents as required by repository rules; designate one owner for schemas, migrations and generated code.

The baseline already has:

- A2A wire version `1.0`, with SDK pins `a2a-lf 0.3.0`, `a2a-client-lf 0.2.1`, `a2a-server-lf 0.4.1`, `a2a-grpc 0.3.1` and `a2a-pb 0.2.0`;
- store schema 9, one bounded daemon writer and exclusive lifecycle ownership; A2A bindings have been durable since schema 2 and harness dispatch since schema 4;
- authenticated local Control v10, with the dispatch operations keeping their v5 minimum and earlier operation versions retained;
- Codex/Claude one-shot execution profiles, Agent Client Protocol (ACP) initialize-only probes, and one active dispatch per project;
- explicitly configured routes, optional route-level models and startup-pinned configuration; there is no per-request model override;
- a separate structured-data supervisor with independent review/verifier promotion;
- read-only frontend queries through the shared `vibemux_client`, a disabled continuous coordinator composer, and a proposed but unimplemented Tauri desktop replacement ([ADR 043](adr/043_tauri_desktop_app.md));
- bounded, in-memory vendor transcripts and conservative dispatch restart recovery.

Keep these pins and constraints unless the selected accepted issue explicitly changes them. A newer upstream SDK is not an automatic dependency upgrade.

## 2. Source and ownership map

| Responsibility | Existing entry points | Keep outside this responsibility |
| --- | --- | --- |
| Wire validation and transport conversion | [task_contract.rs](../crates/vibemux_a2a/src/task_contract.rs), [task_server.rs](../crates/vibemux_a2a/src/task_server.rs), [task_runtime.rs](../crates/vibemux_a2a/src/task_runtime.rs) | SQLite connections, vendor process launch and canonical state authority |
| Pure harness protocol | [request.rs](../crates/vibemux_harness/src/dispatch/request.rs), [route_config.rs](../crates/vibemux_harness/src/dispatch/route_config.rs), [protocol_session.rs](../crates/vibemux_harness/src/dispatch/protocol_session.rs) | Process, network, filesystem and store I/O |
| Execution ownership | [dispatch service](../crates/vibemuxd/src/harness_dispatch/mod.rs), [executor.rs](../crates/vibemuxd/src/harness_dispatch/executor.rs), [supervisor_service.rs](../crates/vibemuxd/src/supervisor_service.rs) | A second owner, shell interpolation and unbounded detached jobs |
| Durable state and fences | [dispatch store](../crates/vibemux_store/src/harness_dispatch.rs), [A2A store](../crates/vibemux_store/src/a2a.rs), [writer](../crates/vibemuxd/src/lib.rs) | Client/plugin writers, network awaits inside transactions and generic ownership bypasses |
| Authenticated control | [control.rs](../crates/vibemuxd/src/control.rs), [control_harness_dispatch.rs](../crates/vibemuxd/src/control_harness_dispatch.rs) | New transports or operations when an existing typed operation suffices |
| Frontend actions and state | [supervisor_client.rs](../crates/vibemux_client/src/supervisor_client.rs), [supervisor_model.rs](../crates/vibemux_client/src/supervisor_model.rs), [supervisor_state.rs](../crates/vibemux_frontend/src/gui/supervisor_state.rs) | Vendor execution, direct store access and fabricated chat history |
| Artifacts and worktrees | [workspace manager](../crates/vibemux_workspace/src/lib.rs), [A2A domain references](../crates/vibemux_types/src/a2a.rs) | Arbitrary file paths, automatic URL fetching or unknown-resource cleanup |

`vibemux_client` is the GUI-neutral daemon/probe/Ask/Git client shared by the egui frontend and intended for a future accepted desktop shell. The egui frontend remains the only implemented native desktop GUI; [ADR 043](adr/043_tauri_desktop_app.md) is still `Proposed`.

Proposed association, inbox, checkpoint, catalog and trace modules are intentionally not given fictitious existing paths or symbols. Their owner should select a cohesive location within this architecture and document new interfaces in the relevant ADR.

## 3. Task cards

### a2a_00_contract: choose and record the extension contract

**Deliverable:** one focused Proposed ADR or explicit amendment for the selected next slice, with acceptance decision recorded through normal project review. Check current ADR numbering rather than reserving a number from this document.

**Read:** ADRs [024](adr/024_stateful_a2a_supervisor.md), [028](adr/028_supervisor_chat_frontend.md), [029](adr/029_daemon_harness_dispatch.md), [030](adr/030_claude_style_gui.md), applicable source above and `PROGRESS.md` immediate execution order.

**Resolve before implementation: scope, state owner, completion meaning, cancellation races, selection policy, payload limits, version changes, storage/content policy, process lifecycle and rollback.** For every new public error choose its stable code in this ADR; do not present a proposed code as already accepted.

Document that the first one-shot UI can reuse the existing dispatch operations (Control v5 minimum). A novel submit/resume/recovery operation or changed wire semantics needs its own compatibility decision. Proposed mapping, schema, events and artifact changes require the repository's ADR/migration process.

**Completion:** a reviewer can identify the exact behavior and files for one bounded implementation issue. Milestone status stays unchanged until matching implementation evidence exists.

### a2a_01_dispatch_ui: a separately labeled one-shot task composer

**Dependencies:** accepted UI-scope amendment from `a2a_00_contract`. This is the recommended first implementation because it reuses the current backend. The only implemented landing surface is the egui shell; if [ADR 043](adr/043_tauri_desktop_app.md) is accepted, implement the GUI-neutral behavior once in `vibemux_client` and expose it through the accepted desktop shell rather than building a second client.

**Own:** frontend action/model/client/state/composer code and its tests (GUI-neutral behavior belongs in `vibemux_client` first); protocol documentation only if needed to explain new UI behavior. Coordinate any changes to accepted ADRs. Do not modify store schema, process executor, route permissions or dependencies for this slice.

**Existing calls:** `ControlClient::harness_dispatch_submit`, `harness_dispatch_status`, `harness_dispatch_output`, `harness_dispatch_cancel` in `control_harness_dispatch.rs`. The shared `vibemux_client` consumed by the egui frontend already depends on `vibemuxd`.

**Behavior:**

1. Load the real route catalog. Require explicit selection of an enabled, execution-capable Codex/Claude route. The current catalog exposes harness, protocol, enabled, allow_execution and probe_supported only; it does not attest live readiness or expose the model. State that model selection is controlled by the daemon route and unavailable through this catalog. Do not infer a vendor default, read private route configuration from the frontend or invent a request-level model field. Safe model-label display is a separate accepted catalog/API extension.
2. Expose a distinct **Dispatch task** action, rather than turning one-shot dispatch into continuous coordinator Send. Keep unsupported coordinator/ACP targets disabled with an accurate explanation.
3. Generate one request UUID when a draft becomes a submission attempt. Keep it stable across uncertain network/IPC outcomes. Distinguish rejected admission from unknown acceptance.
4. Send a typed request through the bounded action queue. Validate UTF-8 byte limits, then let the existing codec enforce escaped frame size too. Do not block the UI thread or construct a shell command.
5. Clear the submitted draft only after a validated admission receipt is associated with the same attempt. Preserve newer edits separately; failure or queue-full must retain the draft.
6. Read status and paged output through the dispatch operations (Control v5 minimum; the daemon protocol is v10). Use the existing output-reassembly contract, bounded records and explicit unavailable-output state after restart or retention loss.
7. Show cancel intent separately from confirmed cancellation. Closing a task window only closes presentation; it does not stop the task.
8. Invalidate old replies when daemon generation/project changes. Do not automatically send an uncertain attempt to a new daemon.

**Existing tests to preserve:** `disabled_send_preserves_multiline_cjk_draft`, `multiple_task_windows_close_independently_without_changing_snapshot`, `action_queue_is_bounded_and_does_not_invoke_the_runtime`, `reads_real_daemon_tasks_and_disconnects_without_owning_execution`. The last two now live in `vibemux_client`; the first two remain in `vibemux_frontend`.

**Add behavior tests:** disabled/unconfigured/old-daemon targets; duplicate clicks; lost receipt followed by same-ID retry; changed payload under an uncertain ID; queue-full; multiline/Unicode byte limit; escaped JSON frame overflow; old-generation reply; output-page byte identity; close-window independence. A Control integration fixture must prove actual admission/status/output/cancel, beyond widget-state tests.

**Acceptance:** a fixture-backed one-shot task is durably admitted exactly once, rendered from real state and output, and cancelable through the existing owner. Coordinator Send remains accurately unavailable. Live vendor turn acceptance is separately authorized and recorded.

### a2a_02_dispatch_bridge: local A2A parent linked to a dispatch child

**Dependencies:** an accepted mapping/completion ADR, current ownership tests, and the relevant prerequisite milestones. UI completion is useful but not a technical prerequisite.

**Recommended proposed mapping:** create a separately owned A2A parent and link a dispatch-owned child through a writer-owned association. Preserve both canonical IDs and the dispatch request/attempt identity. Never bind an A2A Run directly under the dispatch-owned Task.

**Why:** `a2a_cannot_bind_a_dispatch_owned_task` and `generic_commits_cannot_touch_dispatch_owned_entities` deliberately reject dual ownership. The bridge must not delete these checks or use generic projection writes to evade them.

**Own:** bounded adapter/backend logic in `vibemux_a2a` and daemon integration; writer association/migration only under a designated owner; existing SDK conversion remains isolated. Preserve pure harness protocol and process-tree ownership.

**Behavior:**

1. Authorize the authenticated subject against the parent project/task and explicitly enabled route. Caller-supplied actor/owner labels do not grant permission.
2. Validate the versioned payload before any durable admission. Current A2A application payload is 16 KiB; internal serialized task snapshots, messages and lists each have a 32 KiB validation ceiling; the external transport ceiling is 64 KiB. There are at most 16 artifacts per snapshot and 1..16 parts per message/artifact. Local workspace JSON artifacts and dispatch prompts each have a 32 KiB ceiling. Use the most restrictive applicable limit including envelope overhead; do not treat a transport ceiling as every nested object's budget.
3. Define admission atomically with its association and audit event, or accept a tested two-step intent/reconciliation protocol in the ADR. Do not claim atomic admission by calling two unrelated writer methods sequentially.
4. Let the existing dispatch service claim and own the child process. The bridge observes through typed owner APIs; it neither launches another vendor process nor mutates dispatch-owned state through A2A helpers.
5. Preserve content-free progress summaries and explicit output access. Raw vendor records cannot be copied into default A2A events/logs.
6. Define parent promotion separately from vendor/dispatch completion. The current integer-result verifier does not establish arbitrary coding acceptance. Use a bounded task-specific artifact plus independent review/local verification, or keep the result explicitly unverified under an accepted execution-result contract.
7. Define parent/child cancellation races. A2A's late-completion rule and dispatch's `cancel_requested` finish rule differ; retain both observations and choose the parent outcome through the accepted contract.
8. Shutdown stops admission, cancels/drains owned work, joins tasks/process trees, and releases the writer last. Client disconnect alone is not cancellation.

**Tests:** independent local A2A client to real daemon/fixture child; same-ID concurrent admission; conflicting retry; cross-subject access; disabled/busy route; payload ceilings; failures; completion without review; cancel before claim/during run/at finish/after completion; shutdown; late output from an old fence; unchanged main repository. Exercise every advertised binding.

**Acceptance:** external task, parent, dispatch child, attempt, events and artifacts correlate without dual ownership; one actual fixture process executes; canonical parent success requires the accepted verifier path. No remote bind, writable execution, extra dispatch concurrency, ACP execution or approval execution is added.

### a2a_03_durable_inbox: persist authenticated request identity

**Dependencies:** accepted persistence/schema decision. Do not treat existing `a2a_runs` and `a2a_commands` as a durable inbound inbox; they serve different contracts.

**Own:** one migration and store module owner, typed domain records, writer commands and a daemon-owned backend/persistence port. Model-peer ephemeral runtimes must remain usable without gaining store authority.

**Proposed record fields:** schema/version; authenticated subject; project; request ID; idempotency key; external task/context IDs; canonical parent/Run/dispatch association where applicable; request fingerprint; selected route and effective configuration/model identity; acceptance timestamp; current version; execution owner/fence reference; cancellation intent; last accepted state and artifact references. These are design fields, not an existing SQL schema.

**Admission rules:**

- A unique subject/project/key constraint and an immediate writer transaction make concurrent acceptance deterministic. Commit identity/association/audit before response or launch.
- Fingerprints use canonical validated content, including permitted numeric identity rules, and the resolved execution contract. Do not hash only a UI title or raw JSON spelling.
- The same key/content returns the original receipt. Changed content is a conflict. Specify retry behavior after route configuration changes: either original-identity return or conflict, never silent execution with a new model.
- Keep executable paths, private environment names, credentials and full prompts out of public identity projections and default events. Persist only approved metadata/digests.
- Persisting identity does not authorize storing prompts or transcripts. If restart execution would need missing content, report that limitation rather than fabricate or re-fetch content.
- Migration preserves historical event bytes, foreign keys and owner guards. Rollback uses the accepted forward-migration/backup policy; never recreate a user database.

**Tests:** duplicate/conflicting/concurrent requests; same key under different subjects/projects; `1` versus `1.0`; exact interoperable integer boundaries; failure between individual SQL statements; loss after commit before receipt; migration fixtures; no plaintext prompt/secret in state/events.

**Acceptance:** a post-restart retry recovers the original external/canonical association without creating a second semantic execution. Every claimed guarantee has a transaction or concurrency test.

### a2a_04_restart_reconciliation: recover state without replaying effects

**Dependencies:** durable inbox and accepted recovery/cursor contract.

**Own:** daemon startup/backend reconciliation, writer recovery operations and bounded client resume/projection behavior. Reuse existing `recover_harness_dispatches` rules and fenced claims.

**Behavior:** reconstruct authenticated inbound task status from durable records; preserve terminal outcomes, cancellation intent and artifact metadata. Existing dispatch recovery marks unclaimed admitted attempts Failed/Interrupted, moves claimed/running or cancel-requested attempts to `recovery_pending`, and quarantines defective records. Do not turn an interrupted admitted child into a resumable launch. Treat a lost executor/claimed child as reconciliation-required rather than re-launchable. Retain per-project reservation when an orphaned effect cannot be excluded. Defective records remain quarantined.

Clients Get a coherent current snapshot then subscribe to future events. If adding persistent replay, define a bounded cursor keyed by project/subject/task/generation, snapshot watermark, retention and stale-cursor errors. Do not imply the current in-memory broadcast already provides cross-restart history.

Durable identity, durable progress metadata and durable output are three separate scopes. Existing vendor transcripts are lost on restart; display unavailable output accurately. Never reconstruct a successful transcript from a hash or copy content from another attempt.

**Tests:** crash before admission, after admission, after claim, after process start, after finish and before response; late old-fence finish; persisted cancel; multiple recovery calls; defective mapping/record quarantine; unauthorized restored access; subscribe races, lag and stale cursor; no orphan launch or writer overlap.

**Acceptance:** restart restores coherent identity/status without duplicate effects, forged completion or lost owner reservations. Report unresumable execution explicitly.

### a2a_05_input_continuation: resume an explicit pending input checkpoint

**Dependencies:** accepted runtime/checkpoint and backend API decision. Durable inbox/recovery is required before promising continuation across restart.

**Existing limitation:** `TaskExecution` has only `Completed` and `Canceled`; `TaskExecutor::execute` is one-shot. A mock-backend follow-up transport test is not production-runtime continuation.

**Own:** task execution/backend contract, runtime state machine, daemon checkpoint writer operations, conversion/tests and typed UI action only under the accepted contract. Initial scope is ordinary bounded input; tool permission execution and durable vendor sessions stay denied/unimplemented.

**Proposed checkpoint fields:** task/context, authenticated subject, checkpoint ID/kind, expected task version, parent attempt, bounded input schema, expiry, data policy, cancellation state, consumption identity and next-turn fence. Event metadata must remain content-free.

**Behavior:** validate task ownership/context/checkpoint/version/expiry before admission. Compare-and-set consumes one checkpoint once, creates one next-turn identity and commits its event atomically. Duplicate matching input returns the same continuation receipt; conflicting/stale/foreign input is rejected. Never allow overlapping turns, continuation after terminal cancellation, or an answer to an ordinary question to approve a tool operation.

Keep A2A task/context/checkpoint identity separate from a vendor thread/turn/session. A harness protocol change to true multi-turn sessions requires its own accepted decision and protocol fixtures. Until then, restrict continuation to a capable backend; report unsupported for the one-shot dispatch backend.

**Tests:** same task preserved; concurrent responses; duplicate versus conflicting decision; wrong subject/context; stale version; expired checkpoint; cancel/response race; terminal task; max-turn limit; malformed/oversized input; send-stream pause versus ongoing subscription; restart restoration if advertised; approvals still denied.

**Acceptance:** one authorized answer resumes one capable paused task exactly once under its original identity. Existing one-shot routes remain one-shot and all terminal/cancellation invariants survive.

### a2a_06_context_artifacts: explicit bounded context and provenance

**Dependencies:** accepted artifact format/content-retention decision. Expand filename allowlists only through the workspace contract; do not bypass them.

**Own:** SDK-independent manifest/domain validation, workspace artifact writer, authorized daemon selection and transport mapping. Existing artifact writer accepts `result.json`, `review.json` and `verification.json` with owned-worktree checks and create-new semantics.

**Proposed manifest:** schema version, source task/Run/peer, base commit, artifact ID/media type/byte size/hash, allowed consumer and explicitly selected relative content references. A manifest is not read permission and an advertised hash is not verification.

**Behavior:** verify actual bytes, worktree receipt/Git inventory and consumer authorization; enforce total and per-part byte ceilings before reading/serialization. Reject symlink/junction/path traversal, unselected/private/unknown content and existing artifact overwrite. Ignored paths are denied by default; explicitly approved immutable generated artifacts may be consumed only through validated references and ownership receipts under the accepted artifact contract. Use an explicit context builder with a selection list; no full repository/environment forwarding and no automatic artifact URL fetching. Secrets and full prompts stay out of default audit records.

**Tests:** actual hash mismatch; wrong base/Run/owner; read-only versus writable policy; size/UTF-8 limits; ignored/private files; path prefix traps, different drives, spaces/Unicode, symlinks/junctions; manifest roundtrip across advertised bindings; immutable file collision; consumer mismatch; cleanup of only owned clean resources.

**Acceptance:** an authorized consumer can verify selected immutable content and provenance. General remote blob transfer and content-addressed storage are separately scoped.

### a2a_07_timeline_budgets: causal diagnostics and opt-in bounds

**Dependencies:** accepted new event/export contract where applicable; bridge links only if displaying bridge relationships.

**Own:** projections/reporter/frontend timeline and small daemon budget checks. Reuse append-only correlation IDs, existing elapsed/token metadata and bounded outputs; avoid a second telemetry state authority.

**Behavior:** distinguish parent task, role Run, dispatch request/attempt, remote task, model observation and local acceptance. Show cancellation intent/confirmation, retry, recovery and verification explicitly. Summarize vendor output rather than creating one canonical event per raw record.

Deadline/turn/token budgets must be validated, bounded and explicitly enabled. Use monotonic elapsed time. Unknown token/cost usage remains unknown; an estimated cost cannot be represented as observed billing. No implicit cheaper-model fallback, provider switching or arbitrary retry follows budget exhaustion. Export redacted traces only through an explicit reporter/export boundary.

**Tests:** correlation reconstruction; bounded event paging/lag; out-of-order observation; missing usage; zero/negative/overflow budgets; deadline cancel races; redaction of sentinel credentials/prompts/URLs; export failure does not corrupt task state.

**Acceptance:** an operator can explain execution and acceptance from bounded actual observations without leaking prompt content or claiming unmeasured cost.

### a2a_08_peer_catalog: explicit discovery and compatibility

**Dependencies:** accepted configuration/catalog contract. Reuse a current manifest/config format where applicable; new independently editable deployment configuration uses validated JSON with snake_case keys unless an existing contract requires otherwise.

**Own:** a narrow catalog/config module, adapter discovery/probe logic and frontend presentation. Keep private endpoint/credential material separate from safe catalog projections.

**Behavior:** store an explicitly enabled peer identity, skills, protocol version, transports, MIME types, declared limits/auth requirements, last probe and observed results. Claims and probes stay separate. Signed card integrity is not competence or permission. Registration/probing does not change configured models, grant workspace access, enable routes or launch jobs.

Offer explicit selection first. Any later automatic matcher is an opt-in policy among already enabled peers, with visible tie-breaking and no silent fallback. Existing harness route configuration and CC Switch model-peer selection are distinct inventories.

**Tests:** disabled/stale/unknown peers; advertised A2A 0.3 versus supported 1.0; transport mismatch; card mutation/cache; credentials redacted; malformed/oversized metadata; explicit selection preserved; unavailable peer returns a precise error without routing elsewhere.

**Acceptance:** a user can inspect and select a compatible enabled peer. Full AGNTCY federation and identity infrastructure are not required.

### a2a_09_remote_profiles: deferred remote security contract

**Dependencies:** M10 entry gates from `PROGRESS.md`, accepted remote threat model and exact rollout authorization. This card is a design placeholder, not an instruction to deploy.

Require TLS verification, identity/credential references, per-task/artifact authorization, explicit scheme/host/port/address/redirect/DNS policy, body/stream limits, trusted cards where required and cross-language failure tests. Preserve remote-disabled defaults. Tailnet reachability does not replace application authorization.

Do not include real endpoints, machine identities, credentials or private paths in examples. Pin TCK/ITK source and test scenario/environment; native Windows remote behavior and Linux/WSL tooling have separate evidence. Do not describe push, full history or remote readiness as supported until their gates pass.

## 4. Existing validation commands

These command shapes and test targets exist at the inspected baseline. They were **not executed by this documentation task**. Run from the repository root using the actual pinned toolchain; inspect `rust-toolchain.toml`, active compiler and `CARGO_TARGET_DIR` before invoking Cargo. Preserve unrelated toolchain edits; do not silently install or switch a global toolchain.

```powershell
git status --short
git branch --show-current
git rev-parse HEAD
rustup show active-toolchain
rustc --version
cargo --version
```

Narrow fixture tests, selected by the changed contract:

```powershell
cargo test --locked -p vibemux_frontend --test supervisor_ui
cargo test --locked -p vibemux_client --lib supervisor_client
cargo test --locked -p vibemux_client --test daemon_feed --test gui_independence
cargo test --locked -p vibemuxd --features test_helpers --test harness_dispatch --test control_harness_dispatch -- --test-threads=1
cargo test --locked -p vibemuxd --features test_helpers --test supervisor_workflow --test supervisor_grpc -- --test-threads=1
cargo test --locked -p vibemux_a2a --test basic_information_share --test task_runtime --test task_gateway --test task_grpc
cargo test --locked -p vibemux_store --test harness_dispatch
cargo test --locked -p vibemux_store --lib a2a::
cargo test --locked -p vibemux_harness --test dispatch_protocols --test dispatch_attempt --test dispatch_config --test dispatch_framing
cargo test --locked -p vibemux_workspace --test workspace_safety
```

Daemon fixture integrations require `test_helpers`; a zero-test invocation is not evidence. New tests/modules will need their own actual target names. Assert that each requested test executed and that the fixture process was joined. Do not relabel fixtures as live vendor validation. The `vibemux_client` targets replace the former `vibemux_frontend --lib supervisor_client` invocation after the 2026-10-24 client extraction.

Relevant Rust gates after behavior changes:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features -j 2 -- --test-threads=1
git diff --check
```

The serial Windows command is a useful decisive run for current shared-runtime test interference; it does not prove parallel suite readiness. Required platform/CI and release gates remain those in `AGENTS.md`, including additional nextest/deny/audit and relevant Python checks. Missing tools, skips, historical baseline failures and untested platforms must be reported accurately; do not disable assertions or retry away a failure.

For transport/mapping changes apply `AGENTS.md` Section 10.5: official TCK for every advertised binding and cross-language ITK scenarios, together with the specified mapping, cancellation, resubscription, malformed-payload and timeout checks. Follow [a2a_conformance_validation.md](a2a_conformance_validation.md) using exact source pins and owned fixture lifecycle. The runner uses synthetic credentials for fixtures, not real providers. Custom Python/Go bilateral scenarios are supplementary and are not full official ITK. Unresolved ITK/push/history/remote gates remain documented limitations and block expanded support or milestone claims; a passing local fixture cannot remove them. Changed code requires fresh binary/source identities and precise exclusions.

## 5. Bounded implementation assignment template

Use this as a public engineering issue template. Fill in the chosen scope; it is not standing authorization for the whole roadmap.

```text
Implement only task <task_card_id> from docs/a2a_implementation_plan.md.
Baseline: <full_commit_and_branch>. Accepted decision: <ADR_and_status>.
Owned files/modules: <explicit_list>. Other work is present; preserve it.
Expected behavior: <concrete_trigger_and_result>.
Keep current single-writer, lifecycle owners, provider/model selection,
read-only/deny profiles, bounded queues, content-free events and remote defaults.
Read only the task card, its named authorities/source and nearby tests first.
If the accepted contract is missing, prepare the specific decision artifact;
do not weaken a guard or invent acceptance to continue.
Add focused success, failure, retry/concurrency and compatibility tests.
Use fixture providers by default. Live inference, writable jobs, deployment,
commit and push require their applicable explicit authorization.
Run the narrow decisive checks and required gates; report exact outcomes.
Update protocol/compatibility docs and PROGRESS only for evidenced changes.
Return the AGENTS.md Section 21 handoff plus the review findings disposition.
Do not implement adjacent cards or add a framework/router/dependency upgrade.
```

Give the implementation model one task, one owner and one acceptance matrix. When a task needs an unresolved architecture decision, finish its reviewable proposal before dependent code. Avoid splitting the same migration/state machine among parallel implementation agents.
