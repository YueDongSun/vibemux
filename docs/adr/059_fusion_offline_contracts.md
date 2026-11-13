# ADR 059: Offline Fusion benchmark contracts

Status: Accepted, 2026-11-12. Ported from the independently developed
`codex/copilot-fusion-benchmarks` branch and renumbered; the earlier ADR 057
on that branch is now this repository's build-storage ADR 057.

## Context

The Fusion-inspired benchmark experiment compares single-draft,
single-strong, judge-controlled cascade, and two-critic parallel review over
the same coding task. The branch implementation was an opt-in live example
tied to the beta-era harness APIs. The cloud-integration round needs its
contracts available offline and deterministic, without a provider call, and
without presenting the live path as validated.

## Decision

Port the example and its four modules into `vibemux_harness_native`:

- `examples/fusion_benchmark.rs` plus `cases`, `config`, `evidence`, and
  `workflow` modules;
- the six disabled `examples/harness/fusion/*.json` templates;
- a `time` dev-dependency (already a workspace dependency).

The example keeps `--plan` as the default and requires `--execute-live
<config> <new-output-dir>` before any vendor launch. The ported tests assert
the mode contracts, budgets, scoring boundaries, cancellation and settlement,
append-only evidence, and hostile-candidate prompt handling with deterministic
fixtures. No test starts a model call or reads a provider token.

Evidence is a task-owned append-only JSON file, not a canonical event or
database row. Mode results, unknown usage, and unknown cost stay `null` when
not reported.

## Alternatives

- Integrate the benchmark into the daemon or store: rejected; it is an
  experiment harness, and its records are not canonical state.
- Ship only the live runner: rejected; it would make an unverified provider
  path the default and require credentials.
- Keep the branch separate: rejected after the conflict review; the contracts
  are useful to the cloud frame and the port is self-contained.

## Consequences

- `C14` can assert the offline contracts in the managed verification slot.
- The live path remains explicitly unverified; live runs need separate
  authorization and budget, and the templates stay disabled.
- The branch's ADR number was renumbered because this repository already uses
  057 for build storage.

## Validation

`cargo test -p vibemux_harness_native --example fusion_benchmark` (14 tests),
`cargo clippy -p vibemux_harness_native --all-targets --all-features -D
warnings`, and the `C14` verification case.
