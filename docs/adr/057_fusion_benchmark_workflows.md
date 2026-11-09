# ADR 057: Opt-in Fusion-inspired benchmark workflows

Status: Accepted for a standalone experimental example, 2026-11-09.
Implementation is unexecuted; no runtime or platform qualification is claimed.

## Context

The requested scope is to research GitHub Copilot Fusion, adapt its workflow to
this harness, and write benchmarks for later real testing without running them
now. GitHub's [pinned primary documentation](https://github.com/github/docs/blob/5c9f8941dd6b10b27f350ea7de093aebc5d2a3ef/content/copilot/concepts/models/copilot-fusion.md)
defines Single, Cascade, and Critique. It does not publish the automatic model
router's implementation. Its warning that discarded drafts do not undo file
changes makes a read-only initial experiment appropriate.

ADR 040 already provides a standalone bounded native dispatcher. ADR 031's
daemon workflows have canonical ownership/admission and writable capability
gates that a benchmark must not bypass. ACP model selection remains explicitly
unsupported by the native shared protocol engine.

## Decision

Implement the three explicit patterns plus a second Single baseline in the
`vibemux_harness_native` example `fusion_benchmark`. Fixed prompts, strict judge
and review JSON, two concurrent critics, deterministic review aggregation, and
revision through the original model make the experiment reproducible at the
workflow level. No proprietary routing or model-ranking equivalence is claimed.

Use existing read-only Codex/Claude native dispatch profiles with pinned
operator model IDs. Every role has a distinct, non-nested configured context
and its own dispatcher/configuration digest. Five roles and at most two active
dispatches bound concurrency. Repetitions, dispatch count, per-call timeout,
whole-experiment deadline, response size, and reviewer feedback are capped.
There are no automatic retries or permission grants. Both critic futures settle
even when one fails; experiment timeout requests cancellation and awaits cleanup.

The suite evaluates three fixed coding-reasoning tasks with independent Rust
graders, including constrained patch selection and boundary outputs. Candidate
JSON remains data and is never compiled/executed. The oracle runs after each
pattern settles, so neither routing nor revision sees hidden grading feedback.
Protocol completion remains separate from answer correctness.

Default/no-argument execution emits only a `NOT_RUN` plan. Actual inference
requires `--execute-live`, a valid local config with enabled routes, and a new
output directory. This is a Cargo example, not an automatically run benchmark
or a new `vibemuxctl`/daemon API. The existing workspace-pinned `time` crate is
reused as a dev dependency for UTC/RFC3339 report timestamps; no package version,
new transitive dependency, default-feature policy, or production dependency is
introduced.

## Evidence contract and compatibility

Private JSONL evidence version 1 records experiment start, each dispatch before
and after execution, each sample result, and experiment completion. Config,
source/suite and binary hashes identify the experiment. Prompts, generated text,
environment values, filesystem paths, and raw diagnostics are omitted. Missing
provider usage, actual upstream identity, and cost are null, not estimated zero.
Every planned sample remains in the accuracy denominator. Records without a
final experiment-completion record are incomplete, never a passing experiment.

Old behavior: `live_codex_smoke` supports one exact-text Codex smoke turn; there
is no Fusion experiment format. New behavior: the additive example emits, for
example, `{"schema_version":1,"record_type":"plan","status":"NOT_RUN",...}`
without launching anything, or an explicit live invocation writes version-1
experiment evidence. No old report reader is reused. Format changes must bump
this private version rather than reinterpret historical files. Rollback removes
the example/configuration; previously generated reports stay intact.

There is no database, canonical state/event, plugin protocol, IPC, A2A, stable
CLI, or platform-guarantee change. No store migrations or daemon admission are
required because the example never creates canonical Task/Run state.

## Security and failure boundaries

Model and peer output are untrusted bounded data inside JSON envelopes. Only
fixed `accept`/`escalate` decisions are accepted; reviews cannot supply actions.
Unknown/duplicate fields and oversized content are rejected. Prompts never
become command strings. The native configuration gates and permission denials
remain authoritative, and any observed tool/approval activity fails the sample.
Read-only process profiles and separate directories are not a hostile-code
sandbox. Operator-selected executables and their startup configuration remain
trusted under ADR 040; this example does not claim to prevent a malicious
same-user process from accessing the oracle or other files.
Directory/session separation also does not fingerprint or suppress a vendor's
ancestor/global instructions and settings. These are explicitly marked as
unmeasured operator-controlled context in the report, and must be kept fixed
for a meaningful comparison. Distinct requested model labels do not prove
distinct upstream model identities when aliases are in use.

The output directory must be new; old files, branches, worktrees, primary
checkout and database are never modified or cleaned. Deadline cancellation
uses the existing process supervisor rather than inventing resource ownership.
Crashes, evidence I/O failures and budget exhaustion must remain visible; the
report cannot infer model/provider concurrency or billable cost from local
dispatches. Native containment guarantees and unverified platform limits stay
those of ADR 040.
Only deadline cancellation has an explicit joined path in the example; Ctrl+C
and process kills can leave incomplete evidence. Because answer text is omitted,
the resulting hash-only report cannot support independent later regrading.

## Verification status

Offline contracts have been written for strict answer/review schemas, reference
grading boundaries, prompt encoding, path independence, and descriptive metrics.
They have not been executed. Compilation, test suites, live inference, native
Windows/Linux and terminal validation are explicitly deferred by the request.
Only formatting and static review are permitted in this task. See the
[runbook](../guides/fusion_benchmarks.md) for later commands and experiment limits.
