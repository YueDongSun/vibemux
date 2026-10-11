# Decision orchestration roadmap

Status: planning document. Only milestone 1 (items 1 and 2) has an
implementation, and it is fixture-verified only; see
[ADR 034](adr/034_decision_port.md) and [PROGRESS.md](../PROGRESS.md).
Everything else below is planned, not implemented.

## Goal

VibeMux should act as an orchestrator that consults a decision API for the
choices it makes on the operator's behalf: which harness takes a task,
which candidate wins, whether a run continues, which approval needs a human.
Every such choice follows one pattern:

1. Ask a typed question with a closed option set (always including an escape
   option such as `escalate`).
2. Act only when the answer validates and its confidence is at least the
   threshold τ for that decision kind.
3. Otherwise apply a deterministic fallback, or hand the choice to a human.
4. Record the decision as a content-free canonical record that can be
   inspected and replayed.

Deterministic gates (tests, policy, budgets, detection) always run before and
after advice. A decision never grants authority by itself.

## Landscape (observed 2026-10-12)

The research below is dated observation of public repositories, not an
endorsement or a compatibility claim.

- Parallel agents in worktrees or sessions:
  [gastown](https://github.com/gastownhall/gastown),
  [agent-orchestrator](https://github.com/OrchestratorInc/agent-orchestrator),
  [omnigent](https://github.com/omnigent-ai/omnigent),
  [claudexor](https://github.com/razzant/claudexor),
  [claw-orchestrator](https://github.com/Enderfga/claw-orchestrator),
  [ccmanager](https://github.com/kbwo/ccmanager),
  [Maestro](https://github.com/RunMaestro/Maestro),
  [claude-squad](https://github.com/smtg-ai/claude-squad),
  [emdash](https://github.com/generalaction/emdash),
  [coder/mux](https://github.com/coder/mux),
  [agent-of-empires](https://github.com/agent-of-empires/agent-of-empires),
  [container-use](https://github.com/dagger/container-use). Most are
  tmux-based or macOS/Linux-first; a Windows-first local daemon is itself a
  differentiator. Curated list:
  [awesome-agent-orchestrators](https://github.com/andyrewlee/awesome-agent-orchestrators).
- Request routing:
  [Plano / Arch-Router](https://github.com/katanemo/plano),
  [vLLM semantic-router](https://github.com/vllm-project/semantic-router),
  [aurelio semantic-router](https://github.com/aurelio-labs/semantic-router),
  [RouteLLM](https://github.com/lm-sys/RouteLLM),
  [LiteLLM](https://github.com/BerriAI/litellm),
  [RoRF](https://github.com/Not-Diamond/RoRF),
  [RouterBench](https://github.com/withmartian/routerbench).
  These route model calls, not coding-harness tasks, and they do not keep a
  per-decision ledger with a confidence gate.
- Judging and leaderboards:
  [llm-council](https://github.com/karpathy/llm-council),
  [PRarena](https://github.com/aavetis/PRarena),
  [pr-arena](https://github.com/neulab/pr-arena).
- Approval and remote control:
  [octomux](https://github.com/ShreyPaharia/octomux),
  [happy](https://github.com/slopus/happy).
- Stop and budget governors:
  [martin-loop](https://github.com/Keesan12/martin-loop).
- Protocols and telemetry:
  [Agent Client Protocol](https://github.com/zed-industries/agent-client-protocol),
  [OpenTelemetry GenAI semantic conventions](https://github.com/open-telemetry/semantic-conventions-genai),
  [Langfuse](https://github.com/langfuse/langfuse).
- Jev ecosystem:
  [typesafe-sdk-python](https://github.com/typesafe-ai/typesafe-sdk-python),
  [typesafe-sdk-js](https://github.com/typesafe-ai/typesafe-sdk-js),
  [jev-usecases](https://github.com/kenhuangus/jev-usecases),
  [decision-model-benchmark](https://github.com/nibzard/decision-model-benchmark),
  [awesome-typesafe-jev](https://github.com/AbdelStark/awesome-typesafe-jev).
  There is no official Rust SDK, so VibeMux has its own thin adapter.

None of the orchestrators observed treats each orchestration choice as a
typed, recorded, replayable decision with a threshold and a deterministic
fallback. That is the distinctive capability this roadmap builds.

## Jev caveats that shape the design

- Confidence comes from the provider and is not calibrated across domains.
  Independent evaluations saw useful thresholds from 0.62 to 0.96 and about
  0.54 mean confidence when no option was correct. VibeMux therefore
  requires τ ≥ 0.5 and plans per-repository τ fitting (item 3).
- No formula for confidence is published, so VibeMux gates on the lower of the
  reported value and its own normalized-peak computation (ADR 034 §8).
- Responses report a concrete model version. VibeMux accepts only the pinned
  version, so a provider upgrade falls back to local rules until the operator
  re-pins.
- The API is paid and keyed. The default configuration never calls it; two
  gates must both be on (ADR 034 §5).

## Features

| # | Feature | Inspired by | Size | Status |
|---|---|---|---|---|
| 1 | Decision port and ledger | Jev, aurelio thresholds, AAG's advisory adapter | M | Milestone 1, fixture-verified |
| 2 | Per-task harness routing | Plano / Arch-Router, claudexor, OpenRouter auto | M | Milestone 1, fixture-verified |
| 3 | Local per-repository agent leaderboard feeding routing priors and τ fitting | PRarena, pr-arena | M | Planned |
| 4 | Best-of-N race with a judge among gate survivors | claudexor, claw-orchestrator, llm-council, omnigent | M–L | Planned |
| 5 | Review-verdict triage (a failed test forces `revise`) | omnigent, agent-orchestrator, gastown | S–M | Planned |
| 6 | Confidence-ranked approval inbox (allow-list AND τ) | octomux, ccmanager, happy | M | Planned |
| 7 | Stop, continue, or stall governor with hard budgets | martin-loop, gastown watchdogs | M | Planned |
| 8 | Cross-harness handoff with continuation packets | claudexor, claw-orchestrator, coder/mux | M–L | Planned |
| 9 | OpenTelemetry GenAI export and offline decision replay (τ what-if) | OTel GenAI conventions, Langfuse, decision-model-benchmark | M | Planned |
| 10 | VibeMux as an ACP agent or MCP `decide` tool | Agent Client Protocol, claw-orchestrator | M | Planned |

### Notes per feature

1. **Decision port and ledger.** A pure `vibemux_decision` crate with typed
   questions, answer validation, a fallback resolver, and content-free
   records; a one-shot `vibemux_decision_peer` holds the network.
2. **Harness routing.** `vibemuxctl decide route` picks among eligible
   harnesses from enumerated facts (operator labels, prompt size bucket,
   project default). `dispatch submit --decision` runs the choice through
   every ADR 029 gate.
3. **Leaderboard.** Count per-repository outcomes of decided dispatches and
   workflows (accepted, reverted, failed gate) to build routing priors and to
   fit τ per decision kind from recorded decisions.
4. **Best-of-N.** Run N workers in separate worktrees and let the
   deterministic gates (ADR 031) eliminate candidates first. Only the
   survivors go to a judge question whose options are candidate ids.
5. **Review triage.** Map a reviewer's verdict to `accept`, `revise`, or
   `escalate`, with a deterministic override: failing tests always force
   `revise`.
6. **Approval inbox.** Sort pending approvals by decision confidence.
   Auto-approve only when the action is on an operator allow-list and the
   confidence clears τ; everything else waits for a human.
7. **Governor.** Ask `continue`, `stop`, or `stalled` at bounded intervals,
   under hard token, time, and attempt budgets that the decision can never
   extend.
8. **Handoff.** When a harness stalls or a budget runs out, write a
   continuation packet (goal, evidence, remaining work) and let a routing
   decision pick the next harness.
9. **Replay.** Export decisions as OpenTelemetry GenAI spans, and replay
   recorded decisions offline under a different τ to see what would have
   changed.
10. **ACP or MCP surface.** Let other tools ask VibeMux for a decision through
    a standard protocol, with the same gates and ledger.
