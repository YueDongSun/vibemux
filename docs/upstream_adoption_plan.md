# Upstream adoption plan — proposal

> **Status: proposal, not implemented.** This document records a researched
> set of upstream GitHub projects whose components could be forked,
> vendored, adopted as dependencies, or referenced into VibeMux, and the
> selection the operator made on 2026-11-15. Nothing here is implemented;
> each adoption is a future slice with its own validation and PROGRESS
> entry. Facts (license, stars, activity, module layout) were verified
> against the GitHub API on 2026-11-15.

VibeMux is Apache-2.0. Anything incorporated as source must be
Apache-2.0/MIT/BSD-compatible and carry attribution; AGPL, unlicensed, and
non-OSI-licensed projects are **reference-only** and may not have code
copied into this repository.

## Selection (operator, 2026-11-15)

| Direction | Selected |
|---|---|
| Orchestration/UI | vibe-kanban, opencode chat layouts, ccmanager, crystal |
| Planning templates | spec-kit, agent-os, Archon, BMAD/task-master (reference only) |
| Evaluation/fusion | promptfoo, inspect_ai, llm-council (reference only) |
| Tools/terminal | goose-mcp, tui-term (dependency), mcp-proxy |

## P0 — highest value, license-perfect

### 1. vibe-kanban relay stack (cloud phase foundation)

- Repo `BloopAI/vibe-kanban` — Apache-2.0, Rust workspace, 28.7k★, active
  daily (verified 2026-11-15).
- Verified modules: `crates/relay-protocol`, `relay-tunnel-core`,
  `relay-tunnel`, `relay-webrtc`, `relay-ws`, `relay-types`,
  `relay-client`, `relay-control`, `relay-hosts` — a complete relay stack
  including WebRTC; plus `crates/executors` (see P1) and `desktop-bridge`.
- First slice: read `relay-protocol`/`relay-types` and draft an ADR for a
  VibeMux remote-relay boundary (agent-side host ↔ operator-side client)
  that keeps the canonical writer daemon-local. Fork only after the ADR;
  candidate target crate `crates/vibemux_relay` (new, behind a feature).
- Risk: relay is out of today's loopback-only A2A contract; adopting it
  must not weaken the local-first invariants (AGENTS §2).

### 2. goose MCP server runner (C10 tool surface)

- Repo `aaif-goose/goose` — Apache-2.0, Rust, 56.6k★, active.
- Verified modules: `crates/goose-mcp/src/{mcp_server_runner.rs,
  subprocess.rs, memory, tutorial, computercontroller, peekaboo,
  autovisualiser}` and `crates/goose-acp-macros`.
- Verified finding (2026-11-15): `mcp_server_runner.rs` is a 1.2 KB thin
  wrapper over the official Rust SDK `rmcp` (`ServerHandler` + stdio
  transport); goose's value is the pattern, not code. Our `vibemux_mcp`
  hand-rolls JSON-RPC 2.0 today.
- First slice (**ADR drafted 2026-11-16**): adopt `rmcp` as the MCP basis
  for `vibemux_mcp` Scope A under [ADR 075](adr/075_official_rust_mcp_sdk.md)
  (proposed; VibeMux-owned transport, handshake, admission and shutdown
  seams specified; Scope B needs its own ADR). Goose's
  `serve<S: ServerHandler>` shape remains a reference;
  `goose-acp-macros` remains a reference for our ACP route macros. Never a
  core writer handle (AGENTS §3.2).

### 3. spec-kit template library (C02 planning profile templates)

- Repo `github/spec-kit` — MIT, Python, 149.9k★, active.
- Verified templates: `templates/{constitution,plan,spec,tasks,
  checklist}-template.md` plus `templates/commands`.
- First slice (**landed 2026-11-15**): `vibemux_workflow::planning_templates`
  (`builtin_v1` / `sdd_v1`, closed set), profile field `template_set`,
  catalog field `template_set`, the `compiler_template` passthrough on the
  workflow request, and NOTICE attribution pinning the adapted spec-kit
  commit. Prompt assets only; deterministic admission stays ours.

## P1 — adopted patterns and content

### 4. vibe-kanban executors (harness execution patterns)

- Same repo as (1). Verified: `crates/executors/src/{executor_discovery.rs,
  executors/, approvals.rs, command.rs, env.rs, mcp_config.rs,
  model_selector.rs, profile.rs, stdout_dup.rs}`.
- First slice: compare `executor_discovery`/`approvals` against
  `vibemux_harness_native` + `vibemuxd` dispatch; absorb concrete patterns
  (stdout duplication for capture, env allowlisting, per-harness profiles)
  as patches, not a crate fork.

### 5. promptfoo evaluation contract (C14 live measurement)

- Repo `promptfoo/promptfoo` — MIT, TypeScript, 27.2k★, very active.
- First slice: port the assertion/matrix *contract* (assert types, provider
  matrix, red-team case format) onto the existing
  `prompt_suites`/`prompt_candidates` records, and wire an offline
  exporter so a C14 live run can be scored by an external promptfoo
  process (no vendor calls from the core).

### 6. opencode chat layouts (C15 shared UI)

- Repo `anomalyco/opencode` — MIT, TypeScript, 226.9k★, very active.
- Relevant packages: `packages/{tui,client,core,app}` (client/server + TUI
  chat layouts).
- First slice: pick the two chat layouts C15 names and prototype them in
  the browser chat workspace (ADR 056 surface) or as a Ratatui study
  spike; a direct fork stays in a separate frontend, never the core.

### 7. ccmanager session services (ten-harness session surface)

- Repo `kbwo/ccmanager` — MIT, TypeScript/Ink, 1.3k★, active.
- Verified: `src/{services,components,hooks,workers}` covering Claude
  Code, Codex, Gemini, Cursor, Copilot, Cline, OpenCode, Kimi CLIs.
- First slice: port per-CLI session detection/resume rules into the Hub's
  owner adapters (private repo) and mirror the findings as
  `vibemux_harness` profile notes for the diagnostics rows.

## P2 — smaller adoptions and references

| Item | Repo / license / verified fact | Mode |
|---|---|---|
| tui-term | `a-kenji/tui-term`, MIT, Rust, 234★; `src/{widget.rs, vt100_imp.rs, controller.rs}` | Dependency adoption (ratatui PTY widget) — gated on an accepted native-attach ADR; today attach stays premature per CLAUDE.md |
| inspect_ai scorers | `UKGovernmentBEIS/inspect_ai`, MIT, Python, 3.2k★ | Port scorer/solver semantics into Fusion offline contracts' scoring notes |
| crystal (Nimbalyst) | `stravu/crystal`, MIT, TS, 3.1k★ (slowing) | UX reference for the C11 two-track compare view |
| agent-os | `buildermethods/agent-os`, MIT, 5.6k★ | Standards/spec prompt templates; complements spec-kit |
| Archon | `coleam00/Archon`, MIT, TypeScript, 23.9k★ | Harness pattern-library content for `vibemux_harness` profiles |
| mcp-proxy | `punkpeye/mcp-proxy`, MIT, TS, 296★ | Standalone stdio↔SSE bridge for remote MCP access; independent process, never core |

## Reference-only (no code may be copied)

| Project | License | Why reference-only |
|---|---|---|
| claude-squad (8.8k★), claudecodeui (14.6k★) | AGPL-3.0 | Incompatible with Apache-2.0 core; independent-process integration only |
| crush (29.1k★) | non-OSI (FSL-family) | Cannot vendor; UX reference only |
| llm-council (26.6k★) | **no LICENSE file** | Legally unforkable; council debate/verdict formats documented as prose reference for Fusion critique |
| BMAD-METHOD (55.4k★), claude-task-master (28.4k★) | non-OSI / Commons-Clause-family | PRD→task decomposition ideas only |
| MoA (3.0k★) | Apache-2.0 but stale since 2025-01 | Fusion contracts already cover cascade/critique; cite as paper implementation |

## Adoption rules

1. Source forks vendor into `third_party/<project>` (new top level) with
   the upstream LICENSE and a NOTICE entry naming the exact commit; or a
   trimmed adaptation lands in the target crate with the NOTICE entry when
   the copy is substantially rewritten.
2. Dependency adoptions go through the normal dependency rules
   (AGENTS §16).
3. Reference-only projects contribute documentation text only, clearly
   attributed, never code.
4. Every adopted slice updates `PROGRESS.md` in the same change and names
   its evidence class (AGENTS §18/§19).
