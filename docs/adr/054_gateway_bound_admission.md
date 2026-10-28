# ADR 054: Bound gateway admission for workflow model calls

Status: accepted for source implementation; verification is recorded separately.

## Problem and owner boundary

The previous model transport refused every Live-class call before HTTP because
health/model observations could not establish request-scoped routing. AAG's v1
receipts now provide gateway-owned attempt, selector, wire-model and provider-claim
observations, but they do not bind the request to a preflight generation or pin.
A global provider pin or runtime reload could occur between observation and send.

Auto-approved decision: add a producer-owned v2 admission precondition and an
explicit VibeMux policy that requires its matched receipt before releasing text.
Do not replace the workflow engine, share databases, change AAG global selection,
or reinterpret a provider's reported model as independent model attestation.

AAG owns the immutable routing snapshot and its per-request observations. VibeMux
owns configured role aliases, request identity, bounded single-send transport,
durable model-call admission and the text-release decision. Native execution,
worker/reviewer profiles, leases, sandbox qualification and scientific promotion
remain separate owners and gates.

## Producer contract

Authenticated `GET /_aag/v2/receipt_capabilities` reads one live runtime snapshot.
It reports schema 2, server instance, positive native generation, pin presence,
Chat/Messages support and the existing receipt bounds. An absent live snapshot
cannot invent a generation and returns unavailable. Version 1 remains unchanged.

Inference pairs these exact headers:

```text
X-AAG-Receipt: v2.<nonce32hex>.<private-capability64hex>
X-AAG-Route-Precondition: v2.<instance32hex>.<canonical-generation>.<0-or-1-pin>
```

The gateway checks instance, generation and pin against the **same captured Arc**
used for dispatch. Later runtime reloads do not change that request's router,
pin or credential policy. The snapshot owns the planner offers, catalog evidence
and endpoint/provider mappings. Operational breaker, affinity and spread state
can still choose among eligible attempts; the receipt records the actual choice.
Malformed/duplicated/unpaired headers fail before
provider I/O; mismatched snapshots return 409. Both headers are removed before
provider handling. The separate Messages stored-key restriction still applies.

`GET /_aag/v2/receipts/<nonce>` requires gateway authentication and the private
capability header. Its closed wrapper contains schema 2, issuer, accepted admission
or null, and the unchanged nested v1 routing receipt. A matched admission is not
completion evidence by itself. Registry versions share nonce ownership, capacity
and retention, but cannot be read through each other's endpoint.

## Consumer policy and persistence

`GatewayConfig.live_admission` defaults to `disabled`. Omitted legacy settings
serialize unchanged. An operator may explicitly choose `bound_receipt_v2` in the
startup-pinned workflow gateway configuration. Only that policy installs the
production model factory. Construction/startup performs no inference, credential
lookup or socket request; the configured credential reference is resolved at the
model-call boundary.

The transport keeps its numeric loopback origin, disabled proxy/redirect behavior,
role aliases and strict-routing check. It reads authenticated health/models and
v2 capabilities, binds the native snapshot into a separately versioned observation
fingerprint, and sends one inference POST. The original monotonic deadline covers
all reads, response consumption and capability-protected receipt retrieval.

Text is released only after unique response headers and the closed receipt match
nonce, server, request ID, selector, protocol surface, generation, pin, HTTP status,
selected attempt and complete terminal evidence. Missing, partial, expired,
malformed or mismatched post-send evidence discards completion and remains
uncertain; it never falls back to v1 or repeats inference. Existing durable model
admission still reserves before submission and stores the result before text is
returned. A duplicate/uncertain compiler request cannot make another model call.

`ProviderAttribution::GatewayObserved` retains validated content-free evidence.
Requested alias, final wire model, canonical response model and raw provider model
claim remain distinct. Upstream model attestation is explicitly unavailable.
A health fingerprint is not AAG's native integer generation; the v2 fingerprint
includes both the prior observation and the admission snapshot.

## Compatibility and rollback

- Existing stored `unknown` attribution remains parseable.
- Existing configurations omit the new disabled policy when serialized.
- AAG v1 endpoints/headers/receipt JSON remain unchanged for the Hub consumer.
- New gateway-observed model-call rows contain a new typed enum variant. Older
  VibeMux binaries cannot deserialize those rows and fail closed. Disabling new
  admissions does not convert or delete those records. Do not downgrade an owner
  that must read them without a separately reviewed migration or compatible reader.
- Model-call rows are not exposed by the current generic Control/export snapshot
  receipt path. No unrelated Control protocol version is bumped.

Roll out the v2 producer first, install the compatible consumer second, and enable
only the exact reviewed gateway policy last. No rollout is performed by this source
change. Reverting the opt-in prevents new calls; it cannot retract a submitted
request or authorize a retry after a missing receipt.

## Research and implementation choice

GitHub MCP inspection of [Temporal request identity](https://github.com/temporalio/sdk-typescript/blob/8b53edd8c5ced2338ecd8b83739431308d650101/packages/client/src/workflow-client.ts)
informed separate durable call and operation identity. Inspection of
[PaperQA's bounded evidence settings](https://github.com/Future-House/paper-qa/blob/c692d0dfeac4a0296599f7fe21a253a58b7ac12a/docs/tutorials/settings_tutorial.md)
reinforced keeping source evidence and generated interpretation separate. Neither
library defines this AAG contract or grants execution authority; a thin typed
consumer avoids introducing another owner framework.

The arXiv abstract of [Permission to Retry](https://arxiv.org/abs/2610.31916v1)
informed the separation of checkpoints, present authority and effect evidence.
The newly inspected abstract of [Quantifying Claim Smoothing by LLM Research Agents](https://arxiv.org/abs/2610.35793v1)
reports stronger claims than underlying evidence warrants across seven systems
and four datasets. Here that motivates explicit gateway-observed versus
independently attested labels. Abstracts were inspected; no result from either
paper qualifies this implementation or a proprietary model.

## Validation and remaining work

See [validation record](../evidence/gateway_bound_admission_validation.md). Required
checks cover v1 compatibility, same-snapshot races, strict receipt parsing,
partial/unknown outcomes, owned cleanup, production-factory startup without model
injection, and actual AAG-source HTTP to the VibeMux consumer using synthetic
upstream data. Live provider execution and native writable-workflow qualification
remain unverified. High-level planning profiles still need to bind assignments,
reviewers, suites and repository policy before a freeform goal can become a full
workflow automatically.
