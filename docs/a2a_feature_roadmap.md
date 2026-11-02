# A2A feature roadmap

Status: **PROPOSED ENGINEERING PLAN**. This document does not enable features, accept an ADR, change milestone status, or authorize provider inference, writable execution, deployment, or publication.

Source inspection date: **2026-10-26**. Local baseline: `main`, commit `04733738ed68ee3d35b95dcf2a92dab71a482478`. This plan was first prepared on 2026-10-02 against `2445a0153b665b60f402cb5606106022a43d6775`; this refresh updates the local baseline, versions and module paths only. The upstream research snapshot in [Recent upstream work](#recent-upstream-work-research-snapshot-2026-10-02) was not re-verified. Refresh the checkout and relevant contracts before implementation.

## How to use this document

1. Read [AGENTS.md](../AGENTS.md), [PROGRESS.md](../PROGRESS.md), and [architecture.md](architecture.md) first. They remain authoritative.
2. Use this roadmap to choose one bounded issue. Scores express engineering judgment for the current pre-alpha stage, not measured performance or readiness.
3. Implement from the task cards in [a2a_implementation_plan.md](a2a_implementation_plan.md).
4. Review with [a2a_code_review_guide.md](a2a_code_review_guide.md). Required evidence belongs to the actual implementation attempt.

The order below is a dependency order for these proposals. It does not replace the execution order or prerequisites in `PROGRESS.md`. ADR 029 is a deliberately narrow exception to broader plugin/workspace milestones; this roadmap does not extend that exception automatically.

## Current capabilities and gaps

| Area | Inspected capability | Remaining gap |
| --- | --- | --- |
| A2A transport | Authenticated numeric-loopback HTTP+JSON, JSON-RPC, and gRPC through an isolated official SDK adapter | Remote/TLS, signed cards, push delivery, and full official ITK remain excluded |
| Structured-data supervisor | Separate planner/worker/reviewer peers, canonical bindings, immutable artifacts, independent review and local verifier gates | This recipe does not execute coding-harness turns or arbitrary model-generated code |
| Harness execution | Opt-in daemon-owned Codex/Claude one-shot dispatch, pure protocol adapters, contained processes, durable fenced admission, dispatch operations since Control v5 (daemon protocol v10); Agent Client Protocol (ACP) initialize-only probes | Fixtures exercise execution; documented live vendor evidence covers initialize-only probes |
| Recovery | Durable canonical Runs; unclaimed admitted dispatch attempts fail as interrupted, claimed/running attempts retain reservation in `recovery_pending`, and defective records quarantine | Inbound A2A runtime IDs, request deduplication and task index are ephemeral; durable transcripts are absent |
| Continuation | Wire mappings recognize input/auth-required states | Production `TaskExecutor` has terminal outcomes only and runtime Send rejects an existing `task_id` |
| Frontend | Shared `vibemux_client` supports authenticated task/run/event/artifact reads, bounded actions, terminal observation and local drafts; the egui shell is implemented, and a Tauri replacement is proposed (ADR 043) | The coordinator Send action is disabled; one-shot dispatch is not continuous coordinator chat; the desktop migration is not implemented |
| Artifact provenance | Create-new JSON artifacts, SHA-256 bindings, base commits, reviewer/verifier receipts | General content-addressed storage and remote artifact lifecycle remain incomplete |
| Interoperability evidence | Dated official TCK and custom pinned Python/Go bilateral checks | Custom bilateral tests are not full official ITK; historical results do not certify new code |

Sources: [stateful A2A protocol](a2a_supervisor_protocol.md), [dispatch ADR](adr/029_daemon_harness_dispatch.md), [frontend ADR](adr/028_supervisor_chat_frontend.md), [protocol boundaries](protocol_boundaries.md), and [conformance evidence](a2a_conformance_validation.md).

## Ranked proposals

| Capability | Priority / 10 | Relative effort | Specific proposal |
| --- | --- | --- | --- |
| Coding-harness execution through local A2A | 9.5 | High | Connect a separately owned A2A parent task to a daemon-owned dispatch child through an accepted association and verification contract |
| Task continuation and input checkpoints | 9.5 | High | Add versioned pending-input checkpoints and bounded continuation without confusing A2A identities with vendor sessions |
| Durable inbound A2A task identity | 9.0 | High | Persist authenticated request identity, conflict detection, external/internal associations and restart reconciliation |
| Usable one-shot task controls | 9.0 | Medium | Add a separately labeled task composer using existing dispatch operations (Control v5 minimum; daemon v10), with explicit route selection, output paging and cancel observation |
| Context and artifact handoff bundles | 9.0 | Medium | Package explicitly selected context with schemas, hashes, source Run and base commit; keep content retention a separate decision |
| Correlated task timeline and budgets | 8.5 | Medium | Show causal task/attempt/dispatch links and trustworthy usage; add opt-in deadline/turn/token budgets and redacted export |
| Explicit peer and compatibility catalog | 8.0 | Medium | Register enabled peers, advertised capabilities, observed compatibility and freshness without changing provider/model selection |
| Remote endpoint profiles | 7.5 now | High | Keep deferred behind M7/M9 gates; later define TLS, endpoint trust and task/artifact authorization |

The one-shot frontend task controls are the smallest useful first implementation. They reuse existing dispatch admission and process ownership. Their lower implementation cost does not reduce the strategic importance of the later A2A bridge. Land the GUI-neutral behavior in `vibemux_client` so the current egui shell and a future accepted Tauri shell (ADR 043, `Proposed`) do not grow duplicate clients.

## Recommended phases

### Phase 0: contract and scope

Task `a2a_00_contract` records the selected slice, accepted ADR changes, state ownership, compatibility and evidence requirements. New mapping, persistence, event, IPC, artifact or permission semantics require the repository's ADR and compatibility process. Suggested modules and contracts in the implementation guide are proposals, not existing APIs.

Do not bundle all proposals into one ADR or implementation. The one-shot UI, dispatch/A2A association, durable inbox, input continuation and permissions can require separate decisions.

### Phase 1: useful local one-shot dispatch

Task `a2a_01_dispatch_ui` uses the existing authenticated `ControlClient` dispatch methods, now reached through the GUI-neutral `vibemux_client`. Expose a distinct action such as **Dispatch task**, requiring a selected configured harness and preserving the read-only/deny posture. Keep coordinator Send unavailable until its continuous-chat contract exists. Implement the action once for the accepted shell (egui today; Tauri only if ADR 043 is accepted).

Fixture acceptance precedes live provider acceptance. A real inference turn is a separate authorized validation step with an exact provider, model, bounded input, cost/time budget and isolated project. Initialize-only success is not execution evidence.

### Phase 2: local A2A coding-harness bridge

Task `a2a_02_dispatch_bridge` follows an accepted mapping ADR. Recommended direction: a separately owned A2A parent references a dispatch-owned child; neither owner mutates the other owner's Task/Run. A2A observation, dispatch completion and verified parent success have distinct meanings.

The existing structured-data verifier is not a generic coding verifier. Before claiming verified coding-task completion, define the task-specific artifact, independent reviewer and local verifier contract. Otherwise expose a precisely named execution result and retain its unverified status.

### Phase 3: durable identity and restart reconciliation

Tasks `a2a_03_durable_inbox` and `a2a_04_restart_reconciliation` persist admission identity and owned associations through the writer. A retry after a lost receipt returns the original task association. A crash after launch does not trigger automatic re-execution. Retain `recovery_pending`/quarantine and reconcile from owned durable observations.

### Phase 4: interactive input

Task `a2a_05_input_continuation` adds explicit checkpoints, compare-and-set admission and one active turn per task. Answering ordinary input does not authorize a tool effect. Execution approvals and durable vendor sessions remain outside this first continuation slice until their separate policies are accepted.

### Phase 5: context, diagnostics and selected peers

Tasks `a2a_06_context_artifacts`, `a2a_07_timeline_budgets`, and `a2a_08_peer_catalog` extend existing artifacts and events. Keep selection explicit, data bounded, usage unknown when unavailable, and audit events separate from diagnostics.

### Phase 6: remote collaboration

Task `a2a_09_remote_profiles` remains deferred under M10. It must not remove numeric-loopback restrictions as an incidental change. Full ITK and selected remote security scenarios need their own pinned environment and evidence. Windows remains the product platform; optional Linux/WSL test infrastructure does not become an installation prerequisite.

## Recent upstream work (research snapshot, 2026-10-02)

This is a dated research snapshot. It was **not** re-verified during the 2026-10-26 local refresh. Recheck release source and compatibility before dependency changes; no dependency upgrade is included in this plan.

| Upstream | Verified work | Useful design reference |
| --- | --- | --- |
| [A2A specification v1.0.1](https://github.com/a2aproject/A2A/releases/tag/v1.0.1) | Published 2026-05-28; wire protocol and SDK package versions are distinct | Task states, version negotiation and security declarations |
| [JavaScript SDK v1.3.0](https://github.com/a2aproject/a2a-js/releases/tag/v1.3.0) | September release adds database-backed task and push stores | Durable storage boundaries; study [v1.2.0](https://github.com/a2aproject/a2a-js/releases/tag/v1.2.0) for cancellation and tenant/owner isolation |
| [Python SDK v1.2.1](https://github.com/a2aproject/a2a-python/releases/tag/v1.2.1) | September release fixes task-list keyset pagination | Stable cursors; cluster-mode versioned stores are not automatic job takeover |
| [Go SDK v2.6.0](https://github.com/a2aproject/a2a-go/releases/tag/v2.6.0) | September release adds Agent Card signing and shared interoperability scenarios | Independent test peers and configured card trust |
| [Official Rust SDK](https://github.com/a2aproject/a2a-rs) | A2A v1 workspace using Axum/Tonic; APIs are still 0.x | Adapter design and test peers; a CLI release is not a unified SDK release |
| [Google ADK v2.11.0](https://github.com/google/adk-python/releases/tag/v2.11.0) | 2026-10-02 fixes remote A2A resumption after human input | Explicit pause/resume; [v2.10.0](https://github.com/google/adk-python/releases/tag/v2.10.0) adds context construction and stream-end handling |
| [Microsoft Agent Framework Python 1.19.0](https://github.com/microsoft/agent-framework/releases/tag/python-1.19.0) | September work improves checkpoint restoration, concurrent storage and approval context | Checkpoint ownership and exact approval binding; [.NET 1.23.0](https://github.com/microsoft/agent-framework/releases/tag/dotnet-1.23.0) continues approval fixes |
| [BeeAI Framework](https://github.com/i-am-bee/beeai-framework/releases/tag/python_v0.1.85) and [Agent Stack](https://github.com/i-am-bee/agentstack/blob/main/docs/stable/deploy-agents/a2a-agents.mdx) | Recent BeeAI releases; Agent Stack registers running A2A agents and provides interaction UI | Input requests, task trajectory and files; framework replacement is outside this plan |
| [CrewAI 1.15.23 A2A documentation](https://github.com/crewAIInc/crewAI/blob/main/docs/v1.15.23/en/learn/a2a-agent-delegation.mdx) | Bounded delegation, output requests and remote completion review | Turn/time limits and independent acceptance; documented server default 0.3.0 needs a compatibility test |
| [AGNTCY Directory](https://github.com/agntcy/dir/releases/tag/v1.7.1), [Identity](https://github.com/agntcy/identity/releases/tag/v0.0.26), [Observe](https://github.com/agntcy/observe) | September discovery/identity work and OTel instrumentation | Start with a local catalog and redacted traces; federation adds operational dependencies |

[Agent Communication Protocol is archived](https://github.com/i-am-bee/acp). Follow its [ACP-to-A2A migration guide](https://github.com/i-am-bee/agentstack/blob/main/docs/stable/community/acp-a2a-migration-guide.mdx) for integrations with that IBM/BeeAI protocol. This is different from VibeMux's **Agent Client Protocol (ACP)** v1 over stdio, used for OpenCode/Copilot/Grok initialize-only probes. The archival note does not deprecate those adapters or propose migrating them to A2A.

## Dependency and evidence boundaries

- Keep current official Rust dependency pins unless the selected issue is explicitly an SDK upgrade. New upstream releases do not prove VibeMux compatibility.
- Preserve the single daemon writer and pure `vibemux_harness` protocol layer.
- Keep current per-project dispatch concurrency, provider/model selection, permission denial and process containment intact in the initial slices.
- Agent Card claims, requested JSON schemas, vendor final records and process exit codes are observations; each needs the relevant acceptance contract.
- Deduplication is a VibeMux guarantee only when durable admission, conflict detection and execution ownership prove it. A2A does not grant exactly-once side effects automatically.
- A cancel request is intent. Record process-tree and lifecycle observations before reporting confirmed cancellation.
- Historical TCK/interop receipts remain historical. New code needs fresh relevant mapping/transport/platform evidence.
- Full prompts, vendor output, credentials, private topology and machine paths stay out of default events and public examples.

## Documentation-task outcome

This roadmap and its companion guides were prepared from repository source inspection and the dated public research, then re-anchored to `0473373` on 2026-10-26. The refresh changed documentation only: no implementation, migration, provider inference, live execution validation, dependency change or release is represented as completed. `PROGRESS.md` milestone statuses remain unchanged.
