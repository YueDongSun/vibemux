# ADR 034: Decision port and advisory per-task harness routing

Status: Accepted for pre-alpha implementation (milestone 1). Fixture-verified
only: no live call to an external decision API has run, because that needs a
paid API key and the repository owner's explicit authorization.

Extends ADR 015 (single writer), ADR 017 (local control IPC), ADR 029
(daemon-owned harness dispatch), and ADR 031 (dual-track workflows). It does
not change the ADR 029 dispatch request, its gates, or its records.

## Context

Every orchestration choice in VibeMux is hard-coded or operator-configured
today. A `dispatch submit` caller names the harness. `switch` sets a project
default that dispatch never reads. Workflow slots, roles, and assignments come
from config, and candidate selection is a fixed baseline. Nothing scores
options, ranks them adaptively, or records why a choice was made.

Decision APIs such as TypeSafe Jev answer a typed question (one choice among
up to 255 operator-defined options) with a full probability distribution and
a confidence value. That shape fits orchestration well: a question has a
closed answer set, so a wrong answer can never name something outside it,
and the confidence supports a "act above a threshold, otherwise fall back"
rule. Research on 2026-10-12 found many open-source multi-agent tools that
route or judge, but none that treats each orchestration choice as a typed,
recorded, replayable decision with a threshold and a deterministic fallback
(see [decision orchestration roadmap](../decision_orchestration_roadmap.md)).

Jev facts relevant here (TypeSafe documentation, read 2026-10-12):

- `POST https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer`.
  The request carries `model`, `state` (string, object, or array), and named
  `questions`. A choice question is `{type: "choice", instructions,
  criteria: {option_id: description}}` with 1 to 255 options.
- The answer carries `choice` (the highest-probability option),
  `probabilities` (every option, summing to 1), and `confidence` (0 to 1,
  "computed from how probabilities is spread"; no formula is published).
- The request model is `jev-latest`; the response reports a concrete
  version such as `jev-1.13.0`. 429 and 529 are retryable; 401 and 422 are
  not.
- Independent evaluations report that confidence is not calibrated across
  domains: useful thresholds ranged from 0.62 to 0.96, and the mean
  confidence was about 0.54 when no option was correct.

## Decision

### 1. A decision is advisory and never grants execution

A decision is a content-free canonical record that names a chosen option, its
evidence, and how it was reached. It never authorizes anything. A decided
dispatch (`dispatch submit --decision`) builds a normal ADR 029
`DispatchRequest` and runs every existing gate again: route enabled,
`allow_execution`, probe detection, working-directory reservation, prompt
limits, and single-active-dispatch.

### 2. Pure decision logic lives in `vibemux_decision`

`crates/vibemux_decision` is pure: no I/O, no network, no clock reads. It owns
the question and option contract, the facts sent to a backend, answer
validation, local rules, the fallback resolver, the operator config schema,
the persisted record, the canonical event drafts, and the peer wire format.
It depends only on `vibemux_harness`, `vibemux_events`, and `vibemux_types`.
`vibemux_store`, `vibemuxd`, and the peer depend on it.

Milestone 1 implements one decision kind, `harness_route`. The roadmap
reserves other kinds; they are not implemented.

### 3. Options

The options of a `harness_route` question are the harnesses that have an
enabled route with `allow_execution` whose protocol executes, and that the
probe detected. Option ids are `AgentKind::command_name()` values (for
example `opencode`, not the serde form `open_code`). The question always adds
the option `escalate`.

- No eligible harness: `decision_no_eligible_harness`, no state change.
- Exactly one eligible harness: resolved locally, no external call.

### 4. Facts are enumerated; no free text leaves the machine

A backend receives only the option ids, operator-written option descriptions
from the decision config, labels the caller picked from the config's label
allowlist, a prompt size bucket (`tiny`, `small`, `medium`, `large`), and the
project default harness. The prompt, its digest, file paths, and the
environment are never sent. Option descriptions are operator config that is
sent to the provider; the README tells operators to keep them free of
secrets. Forwarding a redacted task summary is deferred to a later
milestone behind its own two flags.

### 5. Two gates for any external call

The daemon spawns a decision peer only when the decision config sets
`jev.enabled: true` and the request carries `allow_external`. With either
off, no peer is spawned and the decision uses the local chain.

### 6. The network lives in a one-shot out-of-process peer

`vibemuxd` performs no decision-API network I/O. For each decision it spawns
`vibemux_decision_peer` (an absolute path next to `vibemuxd`), writes one
request line of at most 16 KiB to stdin, reads one result line of at most
16 KiB from stdout, and waits under a deadline. The child is killed on drop
and on deadline. Its environment is cleared, apart from the platform's
system variables and the one credential variable named by the config. At
most one external exchange runs at a time; a request that finds one in
progress records `decision_peer_busy` and takes the local chain at once
instead of queueing, so one advice never spends more than one peer deadline.

Daemon shutdown cancels the exchange. A control `shutdown` request cancels
it on acceptance, so draining connections does not wait for the peer
deadline. The daemon's shutdown path then waits until the peer is killed
and reaped, before it aborts the remaining connections and releases the
writer. The cancelled advice records `decision_peer_cancelled` and takes
the local chain. After shutdown starts, no peer is spawned.

The peer uses a fixed HTTPS endpoint, refuses redirects, bypasses proxies,
caps the response body at 16 KiB, retries 429 and 529 at most once, and
writes only fixed codes to stderr. Every attempt, including the retry, ends
when the peer's total time budget runs out. A loopback endpoint override
exists only behind the `test_endpoint` cargo feature.

### 7. Credential path

The config names an environment variable (`jev.credential_env`, default
`TYPESAFE_API_KEY`; `[A-Z0-9_]{1,128}`, no `VIBEMUX_` prefix, and not a
system variable such as `PATH`, `TEMP`, or `SYSTEMROOT`). The daemon's
environment is the one inherited by `vibemuxctl daemon start`. Only when both
gates are on does the daemon read that variable and place it in the peer's
cleared environment. The stdin request carries the variable's name, never its
value. The key never appears in argv, config files, logs, events, records, or
Control IPC. A missing or empty variable yields
`decision_credential_unavailable` and the local chain. Rotating the key needs a
daemon restart.

The decision config is therefore the control point for which daemon
variable is sent to the fixed endpoint as a bearer token. It shares the trust
level of `harness_dispatch.json`: anyone who can write the project's
`.vibemux/` directory can already choose the executables the daemon starts.
The name rules stop mistakes, not a hostile writer of that directory.

### 8. Answer validation and the confidence gate

A backend answer is accepted only if all of the following hold:

- its response model equals the configured `jev.pinned_model`;
- `choice` is one of the options;
- `probabilities` cover exactly the options, and each is finite and in
  [0, 1];
- the probabilities sum to 1 within 0.01;
- `choice` is the unique argmax;
- the reported `confidence` is finite and in [0, 1].

Jev does not publish its confidence formula, and its own documentation
example does not match the normalized-peak formula exactly. So VibeMux does
not reject a reported confidence that differs from its own computation.
Instead it computes the normalized peak `(p_max − 1/n) / (1 − 1/n)` and gates
on the lower of the two values. Both are recorded in integer millionths.

The threshold τ is per decision kind in the decision config. It must be in
[0.5, 1.0], because a lower τ would accept answers at the confidence level
observed when no option is correct. τ is policy, not calibration; fitting τ
per repository is roadmap item 3.

### 9. Fallback chain

The resolver is a pure function, applied in this order:

1. An accepted answer whose gated confidence is at least τ: apply its choice.
   If the choice is `escalate`, the decision is `escalated`.
2. Otherwise, the highest-priority local rule whose label is among the
   request labels and whose harness is eligible. Ties keep config order.
3. Otherwise, the project default harness if it is eligible.
4. Otherwise, `escalated`: the operator picks a harness manually.

The external evidence (status, model, choice, confidences, probabilities,
error code) is stored separately from the applied choice. A failed or
rejected external answer records only its fixed error code.

### 10. Persistence

Store schema 6 adds the `decision_records` projection with `updated_sequence
REFERENCES events(sequence)`. The `decision_recorded` and `decision_consumed`
events are appended in the same `IMMEDIATE` transaction as the projection
change, by the single `WriterWorker`. Records and events hold identities,
digests, byte counts, option ids, allowlisted labels, millionths, model ids,
config digests, and fixed codes. The prompt, the facts' free text, the
provider's `usage`, and the key are never stored. A decision expires 15
minutes after creation.

The events are sufficient to rebuild the projection. `decision_recorded`
carries the complete sanitized creation state: options, labels, prompt
digest, size, and size bucket, project default, threshold, config digests,
source, rule index, choice, status, the external evidence, and both
timestamps. `decision_consumed` carries the consuming request and its time.
`vibemux_decision::events::rebuild_record` rebuilds a record from the two
payloads. A test checks that the rebuilt records equal the projection.

### 11. Binding a decision to a dispatch

`DispatchRequest` is unchanged. Control protocol V8 adds
`decision_dispatch_submit {decision_id, request_id, prompt}`. The daemon
builds a `DispatchRequest` for the decided harness and runs every ADR 029
gate. In one store transaction it then checks that the decision is resolved,
not escalated, not consumed, and not expired, and that the harness, the
prompt digest, and the dispatch config digest match the decision. It calls the
same admission code as `dispatch submit` and marks the decision consumed.
Replaying the same `request_id` returns the stored dispatch; a different
`request_id` fails with `decision_already_consumed`.

### 12. Control IPC

Control protocol V8 adds `decision_advise`, `decision_list`, `decision_show`,
and `decision_dispatch_submit`. Errors use the `decision_` code prefix.

## Consequences

- Each orchestration choice becomes a typed, recorded, replayable decision.
  Later roadmap items (leaderboard priors, best-of-N judging, approval
  inbox, governor) reuse the same port and ledger.
- The decided path is longer than a plain `dispatch submit`: two commands and
  a short-lived record. The plain path is unchanged.
- The peer binary must ship next to `vibemuxd`. A missing peer fails with
  `decision_peer_unavailable` and falls back to the local chain; it never
  crashes the daemon.
- Jev's confidence is uncalibrated. Decisions stay advisory, and the default
  configuration never calls the provider.

## Open decisions

1. Redacted task-summary forwarding (behind two flags) for better routing.
2. Additional backends (local classifier, other decision APIs) behind the
   same peer wire.
3. Per-repository τ fitting from recorded outcomes (roadmap item 3).
