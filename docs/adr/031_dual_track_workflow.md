# ADR 031: Daemon-owned dual-track coding workflows

Status: Accepted for pre-alpha implementation on `feat/dual_track_workflow`
(not merged). Offline only: every worker, reviewer, and compiler turn in the
evidence is a synthetic fixture process. No model, AAG gateway, or vendor
CLI was contacted, and the live execution classes of the dual-track
benchmark are `BLOCKED` (see "Verification" and
[the offline evidence](../evidence/dual_track_offline_acceptance.md)).

Extends ADR 015 (single writer), ADR 017 (local control IPC), ADR 024
(versioned run records and independent verifier gates), and ADR 029
(daemon-owned harness dispatch). ADR 029's read-only dispatch profiles,
its one-dispatch-per-project reservation, and its ownership rules for
dispatch-owned Task/Run records are unchanged.

## Context

The request was a supervised workflow in which two coding harnesses work on
one repository at the same time. In `cooperate` mode each worker owns a
disjoint part of one task. In `compare` mode two competitors implement the
same task, and one verified result is selected. A supervisor, a contract
compiler, the workers, and an independent reviewer use explicitly
configured AAG routes. Global default models and context windows are not
changed. The acceptance contract is a pinned 37-scenario benchmark
(`tests/fixtures/dual_track_benchmark/dual_track_benchmark_v1.json`).

### Baseline verification

The research baseline named `main` at `2445a01`. This branch starts from
that commit. Before implementation, the baseline's source-code claims were
checked against the checkout:

- Harness dispatch (ADR 029) keeps one project working directory per daemon.
  Its resource key is the project root, so it cannot run two writable
  workspaces at once. Confirmed.
- The Codex execution profile is read-only and ephemeral. The Claude profile
  disables tools and session persistence. Neither can produce a writable
  multi-round worker. Confirmed.
- Control v5 `harness_dispatch_submit` returns an admission receipt, not a
  completion or acceptance claim. Confirmed.
- Dispatch-owned and A2A-owned Task/Run records must not be rebound by
  generic projection paths. Confirmed; the workflow associates with them
  instead of owning them.
- The AAG proxy (read-only inspection of the development repository) applies
  a global `pinned_provider` to every request when set. It has no
  request-scoped routing receipt and no headless prompt service. Confirmed.
  Dual tracks therefore cannot switch the gateway's global provider, and a
  pinned gateway blocks strict routing.
- The two test fixes the baseline listed as unmerged (PR #14, shared ACL
  marker isolation; PR #15, deterministic startup-timeout test) were still
  unmerged. They are carried on this branch as `c944699` and `0588620`, so
  the full suite runs without touching shared per-user state.

## Decision

Pure contracts and reducers live in a new crate, `vibemux_workflow`, which
performs no I/O. `vibemuxd` performs every effect. `vibemux_store` persists
every state change through the single `WriterWorker` (store schema 5).
`vibemuxctl` is a thin Control v6 client. The dependency direction is
`vibemux_harness → vibemux_workflow → vibemux_store → vibemuxd → vibemux_cli`.

### 1. The workflow aggregate

A workflow is a versioned record with phases `prepared`, `running`,
`paused`, `integrating`, `accepted`, `failed`, `cancel_requested`,
`cancelled`, and `blocked`. It references its admitted contracts by
identity and its child attempts by dispatch request ID. It never takes
ownership of a dispatch-owned or A2A-owned Task/Run. Each worker turn is
admitted through the same dispatch admission (same gates, record,
projections, and event) inside the transaction that checks the workflow
phase and the lease fence. Acceptance is derived from verified child
receipts, never from a child's success label. Every update carries the
expected version, so a stale view cannot commit.

### 2. TaskSpec, compiler validation, policy, and contracts

- **TaskSpec** is the typed representation between the request and the
  English worker contract. It holds requirements with source spans, owned,
  forbidden, and protected paths, tools, checks, budgets, and the mode.
  Set-like fields are normalized, so equal meanings have equal canonical
  bytes.
- **Compiler validation** decides whether a candidate TaskSpec can be
  admitted. A model may propose the interpretation. The candidate is checked
  against the exact request bytes with stable rejection codes:
  - `invented_scope` and `missing_coverage`;
  - `dropped_prohibition` and `weakened_force`;
  - `changed_number` and `invented_number`;
  - `literal_not_preserved` and `literal_not_in_source`.
- **Operator policy** comes from a trusted file. Intersecting it with the
  TaskSpec can only narrow: tools are removed, budgets capped, and protected
  paths added, each recorded as a visible narrowing. A requested write scope
  outside the policy is rejected, not dropped.
- **Contract rendering** is deterministic English. Literal data is emitted
  inside fences longer than any tilde run in the data. A harness that may
  expand `@path` mentions, or whose behavior is unknown, fails closed.
- **Contract identity** is the canonical-JSON digest of the TaskSpec,
  policy, template, harness prompt profile, delivered context bundles, and
  base commit. A semantic edit makes a new contract generation. Each turn
  re-renders the admitted contract with a turn section, so the contract
  identity stays fixed and each attempt records its own prompt digest.

### 3. Supervisor, AAG routes, and prepare

- **Route bindings.** Each role (`supervisor`, `compiler`, `worker_a`,
  `worker_b`, `reviewer`, `optimizer`) binds to an immutable model alias on
  one loopback AAG endpoint. Requested aliases and reported models are
  recorded separately. Missing usage is unknown, never zero. Strict routing
  refuses to route while the gateway reports a global provider pin.
- **Live configs.** A live workflow config must name a gateway with a
  binding for every slot's role. A slot never falls back to a gateway
  default. Fixture configs may omit the gateway; their sessions record the
  route as `fixture:<role>`.
- **Supervisor.** The supervisor's surface is a typed proposal language:
  strict JSON naming one of a few tools with the expected workflow version
  and an idempotency key. It has no tool for SQL, shell, URLs, filesystem
  access, provider switching, secrets, permission grants, contract edits, or
  marking success. Loop budgets are persisted, so a restart does not reset
  them.
- **Prepare.** Prepare compiles an operator request into an admissible
  workflow. The request file holds the original text, the TaskSpec
  candidates, and the slot plan. Prepare checks the plan against the pinned
  config and renders each contract once.

**Not implemented:** there is no daemon AAG client. No supervisor or
compiler model is called: the coordinator is deterministic Rust, and the
TaskSpec candidates come from the operator's request file. `route_generation`
is null, and the `aag_only` policy flag is declarative. `review` mode is
refused at prepare.

### 4. Slots, leases, templates, service, and runtime

- **Slots** are configured in `<state dir>/workflow_config.json`. Each names
  a harness whose dispatch route may execute and a route role (`worker_a`,
  `worker_b`, or `reviewer`). A detected executable is not a coding worker.
  Eligibility needs capability evidence of the level the policy requires. A
  fixture config's executing slots are `write_mode_certified` as fixture
  evidence. A live config's slots stay `declared` and are never eligible to
  code, because no vendor CLI has write-mode certification yet.
- **Leases** reserve one slot and one worktree for one attempt under a
  monotonic generation. A missed heartbeat quarantines a lease; it never
  releases it. A completion with any other generation is stale and refused.
- **Templates** are versioned block lists. Immutable blocks carry ownership,
  protected files, acceptance authority, and the checkpoint format.
  Optimizable blocks carry wording only.
- **The service** pins the config with the digest of its exact bytes; a
  change takes effect only after a restart. It starts at most one
  coordinator per workflow. At startup, a workflow left running or
  integrating stops at `blocked` (daemon restart) until the operator starts
  it again. Nothing resumes on its own.
- **The coordinator** runs three stages:
  1. implementation, at most `width` units at a time;
  2. at most two broker rounds of question and answer;
  3. gating, with bounded repairs.

  Pause and cancel stop admission of new turns. An in-flight turn settles,
  then the step returns `workflow_stop_requested`.
- **Worker turns** go through the dispatch service with a writable profile
  (ADR 029's read-only profiles are unchanged). The resource key is the
  attempt's owned worktree, so two workers run concurrently in different
  worktrees. Request and session IDs are derived from the workflow, task,
  slot, purpose, and ordinal, so a retried step resolves to the same
  admission. The writable profile flags restrict what the vendor offers its
  model. They are not an OS sandbox.

### 5. Messages, bundles, checkpoints, and shares

- A worker ends its turn with one fenced `vibemux_checkpoint` block, and a
  reviewer with one `vibemux_review` block. Both are claims: they route
  messages and record what the session said, but never mark anything
  accepted. The final text comes from the captured structured records,
  never from terminal text.
- **The broker** binds each checkpoint message to the authenticated sender
  session and its post-turn snapshot. A worker cannot speak as the
  supervisor, grant context, or publish verifier receipts. Admission checks
  audience, policy, contract and source freshness, size, TTL, fan-out, and
  correlation depth. Delivery happens at the recipient's turn boundary,
  at least once, with deduplication by message ID and a per-recipient
  sequence.
  Worker delivery is an atomic batch after the recipient attempt is
  admitted and before its process launches. Delivery binds the workflow,
  task, session, and contract; acknowledgement binds the same completed
  attempt. A failed batch changes no message or audit event. The supervisor
  uses a separate deterministic inbox identity.
- **A context bundle** carries selected line ranges of the answering task's
  collected snapshot, never its live worktree. It also carries attributed
  claims, so a proposal is never relabeled as a confirmed decision.
  Secret-looking values are redacted. An over-budget selection is refused,
  not truncated.
- **`context share`** grants one recorded bundle to one worker session of
  the same cooperative workflow. Competitors in a comparison never receive
  each other's context.

### 6. Candidates, gates, selection, and integration

- **Collection.** The daemon decides what a candidate is. It asks Git for
  every changed path of the owned worktree relative to the base commit,
  including untracked and ignored files, and inspects each path without
  following links. A worker's list of changed files is not consulted. A
  candidate is admitted only if every change is a regular text file or a
  deletion inside the owned paths and outside forbidden and protected paths.
  Protected and forbidden paths compare case-insensitively, owned paths
  exactly. Any violation fails the run instead of being cleaned up.
- **Review and verification** run on collected bytes materialized into
  fresh owned worktrees, re-collected before and after. Every receipt names
  the exact candidate digest and contract.
  The store binds a passing review to a completed reviewer dispatch for the
  same task, contract, run, session, and harness. Attempts, candidate
  manifests, and integration receipts must name the admitted base commit;
  integration receipts must cover the accepted contracts exactly.
  `changes_requested` also requires a completed structured review;
  `blocked` diagnostics require a terminal dispatch. An active attempt
  cannot publish a review verdict.
- **The trusted verifier** runs with a cleared environment and the pinned
  argv template, behind the launch trampoline with a deadline. Its directory
  and executable must be absolute and outside the project root; the
  directory is hashed at startup. A missing or inconsistent result is
  `blocked`, never a pass.
- **Acceptance levels** are distinct, and only the last is workflow
  acceptance: `turn_completed`, `candidate_artifact_collected`,
  `independent_review_passed`, `local_verifier_passed`,
  `integration_candidate_verified`, `workflow_accepted`. A candidate passes
  its gate only with a passing review from an independent session and route
  plus a passing trusted verification. A cancelled or failed workflow is
  never accepted.
- **Selection in compare mode** excludes failed gates, other contracts, and
  labeled mutation artifacts. Predeclared metrics then decide in order; an
  unknown metric cannot decide. The lowest candidate digest breaks ties.
  With no valid candidate there is no winner.
- **Integration** applies accepted candidates from the blob store into a
  fresh owned worktree at the base commit, re-collects and verifies it, and
  only then offers it to the writer's acceptance gate. The operator's
  checkout is never written, and nothing is committed, merged, or pushed.
  Case aliases across candidate paths are conflicts before materialization,
  preserving Windows filesystem behavior.
  Cleanup removes only provably clean worktrees. Dirty or unknown ones are
  retained and reported (`workspace_unsafe_cleanup`).

### 7. Optimizer, configuration, and private state

- **The optimizer** may change only allowlisted fields: optimizable template
  wording, context retrieval order and summary budget, preapproved
  decomposition hints, and slot ranking weights. A diff naming any other
  field is rejected before evaluation.
- **Evaluation.** `prompt evaluate` runs one bounded cycle over a fixed
  operator suite (`<state dir>/prompt_suites/<suite_id>.json`) with fixed
  train, dev, and holdout cases. Each case is a real workflow. Candidates
  come from `<state dir>/prompt_candidates/<id>.json`.
- **Selection and promotion.** Candidates are selected on train and dev
  only: hard gates first, then verified success, then known cost. Only the
  final eligible candidate touches the holdout, and a holdout regression
  blocks promotion. "No improvement" is a valid result. Promotion affects
  future admissions only. `prompt rollback` returns the active version to
  its parent.
- **Configuration.** `<state dir>/workflow_config.json` (schema 1, unknown
  fields rejected) requires:
  - `evidence_class`, `git_executable`, `slots`, `heartbeat_ms`, and
    `candidate_limits`;
  - `verifier` (`directory`, `executable`, `suites`, `timeout_ms`,
    `environment_names`).

  `gateway` and `content_retention_days` are optional. Without
  `content_retention_days`, no content is retained.
- **Private state.** `<state dir>/workflow_state` holds content-addressed
  candidate blobs, the opt-in content store and its index, per-workflow
  plans, worktree ledgers, outboxes, and verifier scratch, and evaluation
  records. It is restricted to the daemon's user before use: on Windows
  with the control runtime's protected three-rule ACL, on POSIX with mode
  `0700`. A directory that cannot be restricted fails closed. Same-user
  processes are not isolated.
  Opt-in content has per-workflow ownership and retention. Purge releases
  only the selected workflow's references; bytes and the store tombstone
  change only after the final owner releases them.

### 8. Control v6, CLI, and evidence

- **Control v6** adds 15 operations:
  - `workflow_prepare`, `workflow_start`, `workflow_status`, `workflow_pause`,
    `workflow_cancel`, `workflow_export`, `workflow_slots`, `workflow_share`;
  - `workflow_session_inspect`, `workflow_session_prompt`,
    `workflow_session_attach`;
  - `workflow_policy_versions`, `workflow_policy_rollback`,
    `workflow_policy_evaluate`, `workflow_purge_content`.

  Each requires v6. Earlier operations keep their minimum versions.
  Answers are content-free, except that `workflow_session_prompt` returns
  the one prompt the operator asked for. `workflow_session_attach` is
  refused with `workflow_native_tui_unavailable`.
- **`vibemuxctl`** gains `workflow`, `slots`, `context`, `session`, and
  `prompt` verbs (README "Dual-track workflows"). Only
  `session inspect --prompts` prints prompt text. Only `workflow export`
  writes files, into a new directory it owns.
- **Status, export, and session views** are projections of the canonical
  snapshot. Messages appear as kinds, states, and digests; verifier commands
  as argv templates. No path outside the repository, environment, or
  database path is emitted.

## Verification

`scripts/verify_dual_track.py` is the acceptance runner. Python is only the
test driver.

1. It pins the benchmark by digest and creates an owned output directory
   plus a short private temporary root.
2. It runs `cargo test --workspace --all-features`, `pytest`, and the
   TaskBoard Lite calibration (good solution, nine mutants, browser suite).
3. The workflow integration tests drive the real daemon over Control v6
   and export evidence files.
4. It re-derives every class status and artifact summary from logs and
   evidence. Empty verifier suites, skipped-only repository checks, and a
   missing or relabeled calibration mutant cannot pass. Calibration checks
   source contents and paths, rather than modification times alone.
5. It validates the report (scenario E01). Timed-out commands terminate their
   owned process trees; unknown process completion retains the private root
   for diagnosis. Malformed validation metadata reports `INVALID`.

An offline run can at most be `OFFLINE_PASS`, never full acceptance. Live
classes are `BLOCKED` offline. `--validate <report>` re-derives a written
report from its artifacts.

## Consequences

- Two workers can run at once in separate owned worktrees, gated by
  independent review and trusted verification. Nothing reaches the
  operator's checkout.
- Store schema 5 is forward-only. A pre-v6 `vibemuxctl` refuses a v6
  daemon, so both binaries are upgraded together.
- Without native TUI sessions, the dual-TUI scenarios cannot pass. The
  structured-mode workflow is the only implemented interface.

## Not implemented (known limitations)

- No daemon AAG gateway client, live supervisor, or live compiler. Live
  configs cannot make a slot eligible to code. No live run has happened.
- No native TUI session is owned or attached. Native handoff (benchmark
  T02/T03) is not implemented.
- Prompt editing of a paused session is not implemented; inspection is
  read-only.
- The optimizer's improvement is measured on fixture cases only and implies
  no model gain. Only an optimizer candidate's `template_overrides` affect
  admitted runs.
- The negative-control verifier receipt of compare mode is not persisted.
  Worker owned paths come from the TaskSpec and are not traced to source
  requirements.
- The crash-restart test simulates the crash through the writer, not with a
  killed process.
- Reviewer route labels are assembled by the daemon and checked by the
  independence gate; the store does not independently bind them to a
  persisted dispatch route because that field is not persisted.
- Writable profiles and candidate scope checks are cooperative admission,
  not an OS sandbox. A same-user vendor process can write outside its
  worktree; the daemon rejects such a candidate when it collects it.

## Compatibility and rollback

Rolling back to a pre-031 build requires a store from before the schema 5
migration; schema 5 is not downgraded. Workflow state under
`<state dir>/workflow_state` can be removed after rollback. It is not read
by earlier builds. Without `<state dir>/workflow_config.json` (the
project's `.vibemux` directory), every workflow operation answers
`workflow_unconfigured`.

The private `workflow_state/content_index.json` format is version 2 after
the review repairs. New entries add `retain_until_ms` to the existing
`sha256`, `workflow_id`, and optional `bundle_id` fields. Version 1 entries
remain readable and retain their bytes until explicit purge when their
per-owner expiry is unknown. Subsequent index writes use version 2; no
SQLite or Control protocol version changes. An older binary cannot read
the version 2 index, so rollback also requires the matching pre-upgrade
private state. Do not recreate or discard a user's state to bypass that
restriction.

Version 1 example (digest and UUID values are placeholders):

```json
{"schema_version":1,"entries":[{"sha256":"<digest>","workflow_id":"<workflow_uuid>","bundle_id":null}]}
```

Version 2 example:

```json
{"schema_version":2,"entries":[{"sha256":"<digest>","workflow_id":"<workflow_uuid>","bundle_id":null,"retain_until_ms":4102444800000}]}
```
