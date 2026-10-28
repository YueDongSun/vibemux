# ADR 053: Observe a workflow start and condition cancellation on its identity

Date: 2026-10-28. Status: implemented candidate; native/live gates unchanged.

## Problem

An integrator persisted its start request UUID and reviewed contract before I/O,
but a lost start reply left it without the workflow UUID. Repeating start is not
a read: a stopped coordinator can admit another generation. An ID-only cancel
after a read can also race an operator's newer generation.

## Auto-approved decisions

1. Add Control v11 `workflow_start_observation` and `workflow_cancel_start`.
   All earlier operations keep their minimum versions. Existing authenticated
   loopback IPC, descriptor privacy and frame limits remain authoritative.
2. Observe only the latest durable supervisor execution. Resolve the existing
   exact start binding, read workflow/execution in one serialized writer callback,
   and check request UUID, plan digest and positive generation/version. Do not
   load the unbounded rounds/actions history to answer this compact query.
3. Return observed or not_observed, always with retry_allowed false. A mismatch
   cannot distinguish an old replaced execution from a request never admitted.
   Missing plans, binding errors and store errors remain errors. Observation is
   durable admission evidence, not a reconstruction of original launch response,
   execution readiness, successful completion or permission to retry.
4. Conditional cancellation requires contract, original start UUID and generation.
   Hold the existing lifecycle permit from identity validation through cancellation
   intent/signalling, excluding a newer start. Share the private held-permit cancel
   helper; do not reacquire the public method's single-permit gate.
5. Preserve the existing operator ID-only cancellation path. It is not the same
   generation-fenced contract. No store migration, new execution profile, Live
   slot, network listener or automatic restart is added.

## Protocol and compatibility

```text
vibemuxctl workflow start_observation --contract <digest> --request-id <uuid> --project-root <project>
vibemuxctl workflow cancel_start --contract <digest> --request-id <uuid> --generation <positive> --project-root <project>
```

The observation CLI envelope is `{ok:true,observation:...}`. Schema 1 contains
request_id, start_contract, status, retry_allowed and workflow. A not_observed
answer has workflow null. An observed workflow contains workflow_id, generation,
mode, execution_state, phase and version. It contains no prompt, path, token,
source text, environment or artifact bytes.

Conditional cancel returns `{ok:true,workflow:...}` with workflow_id, phase,
signalled, request_id and generation. A mismatched identity yields
workflow_start_identity_mismatch before any signal or intent. Signalled is an
acknowledgment, not proof that execution stopped. A matching later observation is
needed for terminal interpretation.

Deploy matching new CLI/daemon builds to consume v11. Earlier operations still
accept their existing request versions. Older clients may reject a new descriptor;
the Hub's explicit descriptor allowlist now includes 11 while its generic read
adapter retains request cap 5. Future unknown descriptors remain rejected.

## Consumer and research boundary

The Hub persists a learned start ID before its second fallible I/O. Missing-ID
recovery uses observation only, retains generation/version, and uses the conditional
cancel for that recovered identity. Legacy ID-only flows retain their old contract.
Late valid legacy snapshots cannot replace stronger generation-bound evidence;
wrong identities, malformed records and qualified regressions still fail closed.

The inspected [A2A task types](https://github.com/a2aproject/A2A/blob/89699392a324fb24ac19824041f3bbda131789ad/specification/a2a.proto)
separate task identity/state queries from cancellation. The inspected
[Temporal workflow options](https://github.com/temporalio/sdk-typescript/blob/8b53edd8c5ced2338ecd8b83739431308d650101/packages/common/src/workflow-options.ts)
make workflow-ID reuse an explicit policy. Neither interface makes an arbitrary
lost reply safe to resend. This change uses the current VibeMux owner contract,
not another orchestration runtime or a new A2A implementation.

The arXiv abstract of [Permission to Retry](https://arxiv.org/abs/2610.31916v1)
separates checkpoints, current dispatch authority, external effect evidence and
reconciliation authority in 336 controlled histories. It informed the refusal and
unknown-outcome cases; its results do not qualify this implementation or its Live
backend. Only the abstract was reviewed here.

## Verification and residual boundary

See [validation record](../evidence/workflow_start_recovery_validation.md) for
commands, platform coverage, actual fixture Control evidence and consumer checks.
Latest-only history is a deliberate limitation: old request association may be
unobservable after another generation. No negative receipt authorizes reexecution.
Remote actor/project grants, remote transport deployment, native Mxc qualification
and substantive external-chat delivery remain separate requirements.
