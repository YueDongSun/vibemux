# Fusion-inspired native harness benchmarks

Status, 2026-11-09: implemented as an opt-in Rust example; **NOT RUN**. No
compilation, offline tests, live model request, benchmark, or platform
qualification has been executed for this addition. Formatting and static review
are not runtime evidence. See [ADR 057](../adr/057_fusion_benchmark_workflows.md).

## Source and adaptation

GitHub's [Copilot Fusion documentation at commit
5c9f8941dd6b10b27f350ea7de093aebc5d2a3ef](https://github.com/github/docs/blob/5c9f8941dd6b10b27f350ea7de093aebc5d2a3ef/content/copilot/concepts/models/copilot-fusion.md)
defines Single, Cascade, and Critique. Its [2026-10-28
announcement](https://github.blog/changelog/2026-10-28-route-and-orchestrate-models-with-github-copilot-fusion/)
announces the combination of auto model selection and HydraFusion. Both primary
sources were retrieved on 2026-11-09. The documentation says Fusion is separate
from subagents and that discarded drafts do not automatically undo file edits.

This experiment implements the documented patterns using existing
`vibemux_harness_native::dispatch::Dispatcher` requests. It does not invoke
GitHub's Fusion product, reproduce its automatic router/model pool, or inherit
its performance claims. Each role has an explicit operator-selected model.

| Mode | Executed pattern | Maximum dispatches per sample |
|---|---|---:|
| `single_draft` | Draft model answers | 1 |
| `single_strong` | Strong model answers the identical task | 1 |
| `cascade` | Draft, independent judge, accept or fresh strong answer | 3 |
| `critique` | Draft, two independent critics in parallel, original model revises | 4 |

The two critics receive exactly the same task/draft and cannot see each other's
feedback. Aggregation is a deterministic JSON array in role order, retaining
both reviews without a third model call. Revision uses a new request to the
same pinned draft route. A malformed judge/review, failed dispatch, missing
final answer, or tool/approval event fails that sample; it never silently
becomes Single or an accepted result. An escalation receives the same prompt
as the strong baseline, without carrying forward the discarded draft.

## Workloads and scoring

Three self-contained coding-reasoning tasks have 38 deterministic checks:

| Case | Work required | Checks |
|---|---|---:|
| `keyset_pagination` | Select a constrained patch and derive cursor, tie, limit, and end-page outputs | 11 |
| `idempotent_transfer` | Select atomic/scoped-key fixes and derive replay, conflict, rollback, balances, and event count | 16 |
| `stream_framing` | Select byte-buffering fixes and derive split UTF-8, CRLF, EOF, oversized/malformed frame outcomes | 11 |

Answers are bounded JSON data. Independent Rust graders evaluate semantic
results; a model's completion or judge acceptance never establishes correctness.
The oracle is evaluated after the pattern settles and is never supplied to the
judge, critics, or revision. No model-produced program is executed. These are
public, fixed diagnostic tasks, not hidden holdouts, repository-editing trials,
SWE-bench, or evidence of general coding superiority. Repetitions improve timing
coverage but do not create additional independent tasks.

The four modes run for every case/repetition with cyclically rotated order.
Four repetitions balance each mode's position within a case. The shipped
configuration plans 48 samples and at most 108 harness dispatches. Lower
repetition counts are permitted but only partially balance ordering. There are
no unreported warmup exclusions, retries, or failed-sample filtering.

## Prepare for a later authorized run

The following commands are instructions for later execution; they were **not
run** while this addition was written. Run them from the repository root.

Build only the required binaries/example (PowerShell, native Windows):

```powershell
cargo build --locked --release -p vibemux_harness_native --bin vibemux_harness_native_plugin --example fusion_benchmark
cargo build --locked --release -p vibemux_platform --bin vibemux_launch_trampoline
.\target\release\examples\fusion_benchmark.exe --plan
```

With no arguments or with `--plan`, the example emits only a `NOT_RUN` plan. It
does not read route configuration, inspect credentials, create reports, probe a
vendor, or make a model call. Ordinary Cargo tests/benchmarks never execute its
live `main`; the example's offline unit tests also use no vendor processes.

Copy [the configuration templates](../../examples/harness/fusion/benchmark.json)
to a local experiment directory outside the checkout, then edit:

1. Set `plugin` to the absolute path of the built native plugin. Each role has
   its own existing native-dispatch configuration file with exactly one route.
2. Set each vendor executable to its native absolute path. On Windows `.cmd`,
   `.bat`, and PowerShell launchers are unsupported. No executable-prefix
   arguments are accepted by this benchmark.
3. Assign explicit model IDs and record the actually observed vendor version.
   Draft/strong must request distinct model labels, as must the two
   critics. The names `draft` and `strong` are operator assignments, not measured
   rankings. Aliases can still resolve to the same underlying model; actual
   upstream identities remain unknown in this report.
4. Give all five roles separate, non-nested working directories outside the
   checkout. Keep them free of the grader, private files, and experiment
   evidence. For a `codex_exec` route, use separately initialized disposable
   Git repositories if the installed CLI requires a Git root. The benchmark
   neither creates nor cleans these contexts. Its roles are read-only requests,
   not writable canonical Runs. Directory separation and separate sessions do
   not establish a clean-room model context: vendor/ancestor/global instruction
   loading is unmeasured and remains operator-controlled. In particular, Codex
   can load settings or instructions without producing a tool event. Keep
   those inputs fixed and record them outside the content-free report when
   comparing runs. The header explicitly marks this source of confounding.
5. Allowlist only the environment variable names required by your installed
   vendor's authentication/setup. Keep all values in the process environment;
   never write tokens into configuration. Record OS/CPU/compiler/source-revision
   metadata from the machine that will actually execute the run. Unedited
   `REPLACE_` and `RECORD_` placeholders are rejected; use `unknown` for a machine
   observation that is unavailable rather than inventing a value.
6. Deliberately enable both `enabled` and `allow_execution` in each role file.
   Defaults are false. Preserve `maximum_in_flight: 1`, the fixed vendor posture,
   and a per-request timeout no larger than 120 seconds. Review the total
   `max_dispatches` and `budget_seconds` before enabling live execution.

The model-controlled comparison currently accepts `codex_exec`,
`codex_app_server`, and `claude_stream_json`. Native ACP has no verified model
selector in the existing VibeMux adapter; this runner refuses ACP instead of
silently comparing unspecified defaults. Copilot/Grok/OpenCode ACP and DSH
integration require subsequent work, not a change of a template label.

Then, only when ready to spend live inference budget:

```powershell
.\target\release\examples\fusion_benchmark.exe --execute-live C:\Experiments\fusion\benchmark.json C:\Experiments\fusion-results-001
```

On Linux/macOS, use `./target/release/examples/fusion_benchmark` and native
binary paths without `.exe`. These paths remain unverified. The output parent
must already exist and the final output directory must not exist. A live run
can incur provider charges. The harness retains its normal permission denials;
the benchmark never grants tool permissions or retries a refused request.

## Read the evidence

`events.jsonl` is append-only within a newly created output directory. It has
`experiment_started`, `dispatch_admitted`, `dispatch_finished`,
`sample_finished`, and `experiment_finished` records with `schema_version: 1`.
This private experiment format is separate from core events and canonical state.

The header records the suite/source digest, executable/plugin/config hashes,
requested role models, observed host OS/architecture/logical parallelism, build
profile flag, and explicitly operator-reported machine/vendor versions. Model
text, prompts, tool arguments, stderr, environment values, and absolute paths
are omitted. Per-dispatch prompt/answer hashes and sizes identify the evaluated
bytes without retaining them. Keep originals of local configs separately to
reproduce requested routes.

Hash-only answer evidence cannot be independently regraded after execution.
The binary/suite fingerprints and recorded scores identify the grader and its
verdict, not recoverable answer text. Regrading requires a separately designed,
explicitly authorized content-retention path; none is enabled here.

Per-sample records retain final/draft scores, judge false acceptance, escalation,
dispatch counts, elapsed time, and the parallel critic window. The report's
latencies include model inference, native/plugin startup, and orchestration;
whole-sample time also includes evidence writes. Initial configuration and
fingerprinting are outside the timed experiment. These are end-to-end request
measurements, not core routing microbenchmarks. The critic window measures
concurrent dispatch orchestration, not proven simultaneous provider inference.

Final per-mode summaries use every planned sample in the accuracy denominator.
They separately count correct, incorrect, failed, and not-run samples. Latency
percentiles use attempted samples, including failures, and exclude not-run
entries. These are nearest-rank descriptive percentiles, with no significance
or cost-efficiency claims. Compare failures and paired case/repetition rows as
well as latency. `provider_requests`, `reported_usage`, `known_cost`, and actual
model identity are null, because the runner has no normalized authoritative
provider receipt. A harness dispatch is not a billable-request count.

Budget/deadline exhaustion does not count as success. A deadline signals the
existing dispatcher cancellation channel, waits for active work/cleanup, and
records remaining samples as not run. Two critics are both awaited on failure.
The process exits zero only if every planned sample is correct; otherwise it
prints a fixed error code and exits nonzero. A killed process or interrupted
report lacking `experiment_finished` is incomplete evidence and cannot support
an aggregate result. No automatic resume, history rewrite, or cleanup is done.
Ctrl+C has no explicit graceful signal handler in this example; a user
interruption may leave the report incomplete. The joined cancellation guarantee
described above applies to the configured deadline, not arbitrary process kills.

Later offline qualification commands (also **not executed** in this change):

```text
cargo test --locked -p vibemux_harness_native --example fusion_benchmark
cargo clippy --locked -p vibemux_harness_native --example fusion_benchmark -- -D warnings
```

Repository-wide Rust gates and native Windows/Linux/live-provider qualification
remain deferred until testing is authorized.
