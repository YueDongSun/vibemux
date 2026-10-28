# Bound gateway model-call validation

Date: 2026-10-28. Source is the uncommitted task diff over VibeMux 45308fc29 and
AAG 98ae3df2 at intake; concurrent unrelated work advanced AAG through 92e56fbe to 45c98a2d.
The task-owned receipt diff remained separate and uncommitted. The full R&D goal remains active. No live model, native executor,
installed runtime, credentials, deployment or release was changed.

## Scope

AAG v2 pairs receipt capability with exact snapshot admission. VibeMux explicitly
opts in, preserves the original call deadline and requires matched terminal
metadata before returning model text. The existing single-writer model-call ledger
and native workflow gates remain authoritative. Legacy v1/disabled behavior stays.

## Commands and evidence

Validation uses explicit Rust/Cargo 1.92.0 with locked offline dependencies. Run
ordinary tests before the selected owner fixtures. Producer and consumer use their
own target directories. Synthetic credentials exist only in the test process.

```text
cargo test --locked --offline -p vibemux_workflow
cargo test --locked --offline -p vibemuxd --features test_helpers --test aag_gateway_receipts --test aag_gateway_transport --test workflow_gateway_compile --test model_runtime_output -- --test-threads=1
cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_bound_gateway -- --ignored --exact startup_owned_bound_factory_compiles_once_and_keeps_native_gates --test-threads=1
```

The last command requires `AAG_BOUND_FACTORY_TOKEN` set to the fixture constant
from `aag_bound_fixture_support.rs`. It creates its own listener and disposable
project; it must never target an installed gateway. It uses ordinary workflow
startup, without an injected model factory, and verifies zero native worker starts.

From the AAG checkout:

```text
cargo test --manifest-path src-tauri/Cargo.toml --locked --offline --lib routing_admission -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --locked --offline --lib proxy::routing_admission_tests::routing_admission_vibemux_consumer_reads_actual_v2_owner_over_http -- --ignored --exact --test-threads=1
```

The selected cross-repository test requires `VIBEMUX_AAG_V2_PROBE_TEST` to name the
freshly built VibeMux `aag_gateway_receipts` test executable. The AAG fixture owns
its ephemeral listener, synthetic upstream and token. It launches only the exact
ignored consumer test and requires both exit zero and the explicit success
sentinel. Output is bounded and the child is reaped. No real provider is contacted.

## Initial and intermediate results

The new policy test first failed because the explicit policy was unsupported;
legacy serialization already passed. After implementation, the workflow crate
passed 164 unit tests and two policy integration tests. The existing gateway,
compiler and durable-output suites and the new normal consumer tests passed.
AAG's eight active v2 tests passed; its owner fixture was separately selected.

The first selected actual-owner fixture failed before inference because its
consumer configuration used a reserved `VIBEMUX_` credential-reference prefix.
The reference was corrected while preserving the production validation and the
separate synthetic environment transport. The original failed log is retained;
only a fresh successful owner run can establish the interoperability gate.

Final command results and independent source review are appended below after
validation completes. These fixtures are not live provider or native qualification.


## Final observed checks

| Gate | Result |
| --- | --- |
| Workflow contract crate | 164 unit tests and 2 policy integration tests passed |
| New normal receipt consumer tests | 13 passed; 1 owner-driven fixture separately selected |
| Existing AAG gateway transport | 8 passed |
| Durable model output regression | 2 passed |
| Existing compiler integration | 1 passed |
| Production startup factory fixture | 1 passed, explicitly selected; no injected model factory or native worker start |
| Actual AAG source to VibeMux consumer over HTTP | Passed after fixture-reference correction; exactly one synthetic upstream request |
| AAG proxy suite | 459 passed, 5 ignored, with explicit test-process loopback proxy bypass |
| AAG runtime snapshot suite | 43 passed |
| Existing actual AAG-to-Hub v1 fixture | Passed separately |
| Current Hub producer-source compatibility | Passed; generic Control descriptor 11/request cap 5 retained |
| Strict consumer Clippy, all targets/test_helpers | Passed after equivalent terminal-state predicate simplification |
| Default daemon check without test_helpers | Passed |
| Workspace formatting and whitespace | Passed |
| VibeMux public source artifacts | 722 text files, zero findings |
| Independent Sol source review | PASS after fixture-reference fix; planner/snapshot immutability separately traced |
| AAG privacy contract tests | 32 passed |
| AAG working-tree publication audit | 22 blockers in unchanged pre-existing files; zero findings in task-owned receipt changes |

AAG build output contains 142 compiler warnings. This stage does not claim a green
full-library, desktop, native or release gate. The previous unrelated full-library
failures remain historical evidence. No publisher or deployment action occurred.

## Preserved failures and environment distinctions

A broad AAG run was first blocked by concurrent Gemini code returning a borrowed
path from a temporary value (E0515). This task did not alter that file. Its owner
subsequently changed it to return an owned path; compilation then succeeded.

The broad proxy run next passed 458 tests and failed the unchanged idle-connection
reuse test (two observed connections rather than one). The same test also failed
in isolation. A paired control used the exact same built image with unchanged
length/mtime: ambient environment exited 101; setting only test-process
`NO_PROXY=127.0.0.1,localhost,::1` exited zero. The unchanged complete proxy suite
then passed 459 tests under that explicit direct-loopback fixture condition.
No production client, test assertion, global proxy setting or machine policy was
changed. Ambient proxy connection reuse is not thereby repaired or qualified.

The first strict Clippy run found two equivalent boolean simplifications in the
new receipt validator. A clear terminal-transport match replaced that expression;
contract/consumer tests and strict Clippy then passed. Original failure logs remain
under the task's private verification directories.

The outcome is source/fixture acceptance of the bounded model-call chain. Real
provider calls, installed-runtime upgrades, native writable execution, high-level
automatic workflow construction and the complete R&D objective remain open.
