# Fusion benchmark contracts (offline)

The Fusion-inspired benchmark compares four workflows over the same small
coding task:

| Mode | Shape |
|---|---|
| `single_draft` | one fast model draft |
| `single_strong` | one strong-model baseline |
| `cascade` | a judge decides whether to escalate a draft to the strong model |
| `critique` | two critics review in parallel, then the original model revises |

The implementation lives in the `vibemux_harness_native` example
`crates/vibemux_harness_native/examples/fusion_benchmark.rs` and its modules.
It is an experiment harness, not canonical VibeMux state: evidence is
append-only JSON under the task-owned output directory, never the database or
event log.

## Offline contracts

```bash
cargo run -p vibemux_harness_native --example fusion_benchmark -- --plan
cargo test -p vibemux_harness_native --example fusion_benchmark
```

`--plan` prints the planned samples and exits; it is not execution evidence.
The tests cover, with deterministic fixtures and no provider call:

- configuration independence (nested deployment/working roots are refused);
- bounded labels and no control text in reports;
- prompt construction that keeps a hostile candidate inside data;
- completion requiring a final message and no tool activity;
- model reviews that cannot add actions or grow unbounded;
- nullable fields, page bounds, malformed or ambiguous answers;
- plausible boundary and atomicity mistakes losing credit;
- append-only evidence that is never reopened or truncated;
- the default plan never counting as execution evidence.

## Live runs

Live inference is opt-in and separate:

```bash
cargo run -p vibemux_harness_native --example fusion_benchmark -- \
  --execute-live <config.json> <new-output-dir>
```

Every shipped `examples/harness/fusion/*.json` template disables execution
(`"enabled": false, "allow_execution": false`). A live run needs an explicit
operator authorization and a bounded budget; no live run has been executed in
this repository's evidence. Costs, model identity, and usage are recorded as
reported or `null`, never inferred.

## Limits

- The benchmark is offline-contract evidence only. It does not prove a live
  provider path, latency, or cost.
- Browser suites and the real TaskBoard repository are separate benchmarks;
  this example uses its own small cases.
- Evidence lives in the task-owned output directory; deleting it is a
  deliberate operator action, not automatic cleanup.
