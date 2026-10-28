# Owner planning profile validation

Date: 2026-10-28. Uncommitted task diff over VibeMux 45308fc29 and Hub c78ec6d.
Explicit Rust/Cargo 1.92.0, Node 24.18.0 and isolated Chrome 156.0.8078.48.
No installed runtime, real model, native sandbox qualification or deployment changed.

## Implemented scope

Startup-owned profile policy and digest; immutable request/compiler/start records;
one compiler call; deterministic approved-slot assignment; existing source/policy/
path/verifier admission; optional original-UUID deterministic execution; read-only
observation; additive Control v12 and CLI; local/signed-chat Hub consumer and UI.

Profile records live separately from the existing strict WorkflowPlan schema.
The store schema is unchanged. A bounded read-only compiler-owner getter supplies
positive correlation and refuses a foreign reservation. Generic preparation refuses
the reserved profile_ namespace before any model or preparation effect.

## Executed checks

- Fifteen owner planning/profile/state/Control tests passed.
- Six explicitly selected integration fixtures passed through the ordinary
  BoundReceiptV2 model factory, using a synthetic gateway and owned fixture workers.
- The execution fixture reached accepted through its independent review/verifier,
  retained original UUID and generation one, then repeated no compiler/worker on
  duplicate request. Its evidence class remained fixture.
- Actual Rust Control JSON passed the Hub validator: two execution observations,
  four deliberate prepare-only consumer refusals, and ten corrupted-response checks.
- The new bounded compiler lookup passed its read-only identity regression.
- Strict Clippy passed across changed daemon, CLI and store packages/all targets.
- Hub full suite passed 1,806 tests; syntax and current producer source checks passed.
- Chrome goal recovery passed 13 checks with desktop/mobile images inspected;
  broad browser regression passed 207 checks and 192 render claims.
- Independent source review passed after the namespace and exact-goal parser fixes.

Additional compatibility and final artifact results are appended when complete.

## Reproduction commands

```text
cargo test --locked --offline -p vibemuxd --features test_helpers --lib planning -- --test-threads=1
cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_planning -- --ignored --nocapture --test-threads=1
cargo test --locked --offline -p vibemux_store --test model_call_ledger
cargo clippy --locked --offline -p vibemuxd -p vibemux_cli -p vibemux_store --all-targets --features vibemuxd/test_helpers -- -D warnings
```

The selected integration requires `AAG_BOUND_FACTORY_TOKEN` set to the synthetic
constant in `aag_bound_fixture_support.rs`; it creates its own ephemeral listener.
`VIBEMUX_PLANNING_WIRE_CAPTURE=1` emits only closed public observations for the
Hub's `scripts/rd_planning_observation_contract.mjs` checker. Never substitute a
live gateway or real provider for this controlled fixture.

## Preserved failures and limits

The first compile caught a sync/async discovery-method mismatch; it was corrected
before running the owner tests. Source review found possible adoption of a legacy
workflow under a planning UUID, closed by namespace isolation and exact compiler
correlation. It also caught the chat parser consuming leading goal whitespace;
a single space/tab delimiter now preserves the remaining original bytes.

The first full Hub run failed its English-artifact gate on a literal Unicode test
character. The equivalent escaped fixture preserved behavior; the final full suite
passed. Assertions and execution gates were not weakened.

Power-loss durability is platform-dependent; create-new records plus sync protect
normal process restart, and POSIX directory synchronization is explicit. A missing
or partial claim is never turned into retry permission. State maintenance/retention
is manual, and old profiles are not automatically rebound after configuration changes.

This completes the source/fixture planning path, not live installed-controller
qualification, remote freeform-goal grants, actual model quality, substantive QQ
result delivery, or the full four-direction R&D objective.


## Compatibility and resource evidence

The final unchanged compatibility run passed 46 Control tests, six workflow admission
tests, one existing compiler integration, three start-observation/conditional-cancel
tests and all 14 model-call ledger tests. The planning subset overlaps three Control
tests; counts must not be added as distinct tests.

The first compatibility link attempt failed with LNK1318 and LNK1180. Measurement
showed only 35,958,784 bytes free on drive I:. With no Cargo/rustc/link process active,
five large regenerable incremental directories in this task's build root were
removed after absolute-path/reparse-point checks. Source, binaries, verification
logs and other-project caches were retained. Free space rose to about 6.1 GB.
The unchanged tests then compiled serially and passed. No debug, assertion or
acceptance requirement was disabled.

Artifact validation passed 729 VibeMux text files with zero findings, and 297 Hub
artifacts passed. Independent source review returned PASS after the namespace and
exact-goal-byte corrections. The full goal and prior native/live blockers remain.


The final CLI parser/input gate passed five tests. Its first build exhausted disk
space while generating the test artifact. A second bounded cleanup removed six
measured inactive incremental directories from this task's target (compiled
dependencies and evidence were preserved); the same CLI tests then passed. The
first cleanup attempt refused while a compiler was active, and proceeded only
after a fresh process check confirmed no Cargo/rustc/link process remained.
No source, model output, binary evidence, test assertion or global setting was
removed or relaxed to obtain this result.

The final default daemon check (without test_helpers) and workspace formatting
check passed. No required check for this source increment remains pending.
