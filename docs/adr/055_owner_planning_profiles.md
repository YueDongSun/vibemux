# ADR 055: Owner-bound high-level workflow planning

Status: accepted for source and controlled fixture use. Live/native qualification remains separate.

## Context and decision

A bounded AAG compiler and generation-checked workflow execution already exist.
The remaining gap is the trusted information needed to turn a high-level request
into a complete workflow: repository policy, eligible workers, independent reviewer
and trusted verifier suites. Supplying those through model output would confuse
planning with authority. Requiring callers to invent TaskSpecs and assignments
would leave the automatic planning requirement incomplete.

Auto-approved decision: define startup-pinned planning profiles in the existing
workflow configuration. A profile owns policy, workflow key/mode, eligible worker
slots, reviewer, integration suites, shared-contract paths, maximum task count and
whether execution may follow preparation. The caller supplies only the exact goal,
request UUID, profile ID/digest and prepare-or-execute choice. No profile is enabled
by migration; an absent profile list grants no new planning entrypoint.

The profile digest includes its complete content and the pinned configuration.
Planning requires the explicit AAG bound-receipt-v2 compiler route. Neither the
model nor source text may install profiles, choose credentials, change verifiers
or expand tool/path permissions. Model output remains strict TaskSpec candidates.
The daemon derives task assignments deterministically from sorted unique task keys
and approved slots, then uses the existing source-coverage, path, policy and suite
admission. Cooperative mode uses one distinct slot per task; compare mode admits
one task across the approved competitors.

## Identity, effects and recovery

Before a compiler call, the owner claims the original UUID, exact source-text
SHA256, profile digest, execution choice and configuration digest in protected
private state. These digests are consumed by the identity and admission contract;
no unrelated checksum inventory is added. The immutable claim uses create-new
publication. Partial records are retained as uncertainty, never removed to retry.
The request key derives from the original UUID. Generic preparation cannot claim
the profile request namespace through another endpoint.

The existing compiler ledger retains its one-call namespace and accounting
association. A completed ledger does not contain replayable model text; a crash
before retaining the admitted plan stays unknown. A matching duplicate observes
existing state instead of compiling again. Changed source, profile or execution
choice under the same UUID conflicts.

Execution is optional and must be allowed by the profile. A separate immutable
start marker is retained before the first start. The existing deterministic start
uses the original request UUID and compiled contract; native qualification and
attempt/lease/reviewer/verifier gates stay intact. Unknown start or cancellation
outcomes are reconciled through generation-bound reads, never another mutation.
Preparation-only cannot be upgraded to execution by reusing its UUID.

Read-only observation returns profile/request metadata, a positively matched
prepared contract and, when available, the latest matching execution observation.
It never compiles, starts, resumes, cancels, edits ACLs or returns source text.
Missing evidence always has retry_allowed false. Prepared means contract admission,
not completed development; execution state remains a separate nested observation.

## Public interfaces and consumers

Control v12 adds workflow_planning_profiles, workflow_plan_request and
workflow_planning_observation. Existing operation minimum versions and response
version echo remain unchanged. The CLI accepts bounded closed request/query JSON
from a file or stdin; there are no policy, slot, command or arbitrary path overrides.

The Hub may bind a local project to one exact profile ID/digest. A nonempty reviewed
development goal uses that owner path; empty development commands retain the
existing immutable-contract behavior. Signed QQ/OpenClaw input retains its sender,
group and operation checks. The remote workflow protocol remains contract-only:
this increment does not implicitly grant a peer permission to send freeform goals.

## Research basis

GitHub MCP inspection of [SWE-agent's architecture](https://github.com/SWE-agent/SWE-agent/blob/3ea751c087f32b16e039a2233dd6eefecef325d5/docs/background/architecture.md)
shows environment/tool provisioning separate from the agent loop. The existing
VibeMux owner is reused instead of installing another runtime. Inspection of
[LangGraph checkpoint identity](https://github.com/langchain-ai/langgraph/blob/eb8ec111b3e9d3108af5d7bdcac51e3e913637a0/libs/checkpoint/langgraph/checkpoint/base/__init__.py)
informed retaining a stable task identity, without treating a checkpoint as effect
replay permission.

The arXiv abstract of [Matched Capabilities, Different Behaviors](https://arxiv.org/abs/2610.34298v1)
reports interface-sensitive divergence across six models and three environments.
It motivates verifying CLI/Control and consumer realizations against the same
semantic contract, not inferring equivalence from similar names. Only the abstract
was reviewed; its benchmark results do not qualify this implementation.

## Acceptance requirements

Required checks include strict profile/schema validation, original-goal preservation,
profile/configuration changes, one compiler call across duplicate/restart/lost replies,
foreign-request namespace refusal, deterministic slot assignment, prepare-only with
zero native starts, execution permission before model calls, unchanged native gates,
read-only observation and adversarial model candidates. Hub tests must preserve
legacy prepared contracts, deny remote planning, and retain one UI request identity
when a response is lost. Source review, producer/consumer fixtures and browser checks
remain separate from installed-runtime/live-model acceptance.


See [validation record](../evidence/planning_profile_validation.md) and the
[profile example](../examples/planning_profile.json). The sample has execution
disabled and must be adapted to an already configured trusted owner before use.
