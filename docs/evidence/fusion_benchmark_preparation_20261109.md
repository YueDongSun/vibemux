# Fusion benchmark preparation handoff — 2026-11-09

## Scope

- Issue/task: research Copilot Fusion, adapt its documented patterns into the
  native harness, and write real-model benchmarks without executing tests.
- Owned modules: the `vibemux_harness_native` benchmark example, its supporting
  modules/configuration templates, Cargo development metadata, and associated
  documentation. Integration branch: `codex/copilot-fusion-benchmarks`, based on
  `0619a3518a3d5a925199e2f0067172c788f8b150`.

## Implemented

- Explicit Single/Cascade/Critique orchestration and a strong Single baseline
  over existing read-only native dispatch. Judge/critic responses are strict
  bounded JSON; both parallel critics settle before same-model revision.
- Three coding-reasoning workloads with 38 deterministic scoring checks and
  14 written offline test contracts. No generated program execution.
- A plan-only default, disabled route templates, explicit live opt-in,
  call/deadline bounds, rotated mode order, and append-only content-free reports.
- Source provenance, configuration/run instructions, evidence semantics, and
  limits in [the runbook](../guides/fusion_benchmarks.md) and
  [ADR 057](../adr/057_fusion_benchmark_workflows.md).

## Behavior and invariants

- Model/protocol completion and judge acceptance do not establish correctness.
- Failed or unattempted samples remain in the accuracy denominator; unknown
  provider/model/usage/cost data remain unknown.
- No canonical state, database, daemon workflow, permission, or ownership gate
  changes. No automatic retry, commit, merge, push, or resource cleanup.
- Vendor text is data, never a shell command. Reports omit prompts, answers,
  raw diagnostics, credential values, and absolute paths.

## Files changed

- `crates/vibemux_harness_native/examples/fusion_benchmark.rs`
- `crates/vibemux_harness_native/examples/fusion_benchmark/{cases,config,evidence,workflow}.rs`
- `examples/harness/fusion/{benchmark,draft,strong,judge,critic_a,critic_b}.json`
- `crates/vibemux_harness_native/Cargo.toml` and `Cargo.lock`: reuse the already
  pinned `time` development dependency for RFC3339 report timestamps.
- `docs/guides/fusion_benchmarks.md`, `docs/adr/057_fusion_benchmark_workflows.md`,
  this handoff, `docs/architecture.md`, `PROGRESS.md`, and `CHANGELOG.md`.

## Validation executed

- `rustfmt --edition 2024 crates/vibemux_harness_native/examples/fusion_benchmark.rs`:
  successful; parsed and formatted the example and its modules.
- `rustfmt --edition 2024 --check crates/vibemux_harness_native/examples/fusion_benchmark.rs`:
  successful on the final source.
- `git diff --check`: no whitespace diagnostics for tracked changes.
- `git diff --no-index --check /dev/null <added-file>` for every added file:
  no whitespace diagnostics. A no-index difference exit status is not a test
  failure or an execution result.
- Independent static review checked protocol extraction, pattern order,
  deadline/budget paths, report/privacy semantics, and reference answers.
  Follow-up edits clarified context/alias/audit limits and tightened model-label
  distinctness. This review is not evidence of successful compilation.
- Public GitHub documentation and its pinned source were retrieved. No vendor
  CLI or provider was contacted.
- **Not executed:** cargo build/check/test/clippy/nextest, offline test contracts,
  benchmarks, live models, provider probes, performance measurements, and CI.
  These were deferred to honor the request not to test now.

## Platform coverage

- Windows: unverified; native `.exe` templates and future PowerShell commands.
- Linux/WSL: unverified.
- macOS: file editing, formatting, and static review only; no runtime coverage.
- Live terminal backend: not exercised or required by the example.

## Compatibility / migration

- Additive Cargo example and private version-1 experiment evidence only.
- No database, canonical event, plugin, A2A, IPC, or stable CLI migration.
- No new package versions, transitive packages, or production dependencies.

## Security impact

- Existing read-only native launch profiles and permission denials are reused.
- Strict bounded JSON rejects arbitrary judge actions and malformed reviews;
  observed tool/approval activity fails the sample.
- Output must be a new directory; existing evidence is never truncated.
- Separate contexts are not a sandbox. Vendor-loaded ancestor/global settings
  remain unmeasured operator-controlled inputs, and hashes cannot recover
  omitted answers for later independent regrading.

## Remaining work

- Later authorized compilation, execution of the written contracts and normal
  Rust gates, followed by configured real-provider experiments and platform
  qualification. None is required to have run during this preparation task.
- Prepared for draft review. Compilation, tests, provider execution and platform
  qualification remain deferred. No benchmark results were produced. Publication
  uses a commit-level `[skip ci]` instruction to preserve the requested testing
  deferral; repository workflows and test gates are unchanged.

## Known risks

- The source is uncompiled and untested. Runtime behavior and performance are
  not established.
- This is a Fusion-inspired workflow experiment, not GitHub's automatic router.
- Measures fixed public coding reasoning, not writable repository tasks or
  general coding superiority. Actual model identity/usage/cost are unavailable.
- Only Codex/Claude routes with explicit supported model selection participate;
  Copilot ACP, OpenCode/Grok ACP and DSH remain outside this experiment.
- Deadline cancellation is joined; arbitrary process interruption/Ctrl+C may
  leave incomplete evidence. No automatic resume or later regrading is provided.
