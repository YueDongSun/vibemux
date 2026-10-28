# Disposable development-test validation, 2026-10-27

Scope: ADR 045's explicit owner component on native Windows, Rust 1.92.0,
Node 24.18.0 and Codex CLI `0.164.0-alpha.11.1`. Source is the current uncommitted
component on base commit `45308fc`. No daemon, workflow slot, live provider or
remote-device capability was promoted.

## Startup evidence

The preceding fixed workload used the same offline root-deny policy for four
Node launch forms. Positive controls read/write the disposable workspace and
scratch. Negative controls attempted protected/sealed/outside access, a child
outside write, and IPv4/IPv6 connections to owner listeners proved live first.

| Launch | Observed result |
|---|---|
| Inline `node -e` | Exit 0 and the expected positive/negative observations |
| Plain file | Exit 1 before controls; `lstat` of the drive root returned EPERM |
| Default `--test` file | Exit 1; child loader encountered the same root denial |
| `--preserve-symlinks --preserve-symlinks-main --test` | Exit 0 and the expected observations |

Outside/protected canaries remained unchanged, forbidden output files were
absent, and both listeners observed zero unexpected accepts. This experiment
established a usable fixed launch form; it was not reusable qualification and
did not establish the executor's complete process lifecycle.

## Implemented executor checks

The request tests first failed against the unsupported stub (two failures), then
passed with explicit combined read/run admission and per-input read scope.
The final default crate run passed 22 tests: three new execution-contract tests
and the existing nineteen workspace/MCP/CLI tests. Its nine installed-runtime
tests were deliberately ignored in that default run, then executed explicitly.

The final installed suite passed all nine tests with zero failures or skips:

| Control | Independent result |
|---|---|
| Documented example through actual command | Expected report and output; reused output path refused before a second execution |
| Disposable candidate edits/creates | Copy removed; admitted source record unchanged |
| Outside read/write and IPv4/IPv6 | Denials observed; original canary unchanged and live listeners accepted no candidate connection |
| Failing assertion / `process.exit(0)` | Distinct process exits; both reports leave assertions unverified |
| Timeout / stdout+stderr overflow | Whole Job stopped, root reaped and snapshot removed; overflow capture marked incomplete |
| Cancellation before/after launch | Correct cancelled classification; confirmed cleanup after launched case |
| Lost sender / unpolled future drop | Closed owner refuses launch; dropped unpolled future retains attributable snapshot |
| Descendant with detached console and closed stdio | Normal root exit followed by confirmed Job shutdown and cleanup |
| Inherited pipe handle | Execution times out; whole Job shutdown precedes cleanup |
| Junction creation | Tested backend denies creation; outside canary remains unchanged |
| Owner-marker deletion lock | Execution status preserved; removal false and exact retained owner record available |

These are nine test functions covering multiple observations. The capture budget
counts combined raw bytes and also applies to output buffered when the root exits.
The successful example includes a relative CommonJS dependency, exercising the
final fixed argv with concurrency and `--` entry separation.

An earlier adversarial fixture expected junction creation to succeed. It failed
with EPERM before its cleanup assertion. The corrected test records creation
denial and separately exercises inherited pipes. This is not evidence of removing
a candidate-created junction. The original failed expectation remains a distinct
observation; it was not rewritten as a successful cleanup experiment.

Strict all-target Clippy and workspace formatting passed. Current-tree public
artifact validation found no private-content findings or new exceptions.
Independent source review returned a qualified component PASS. The default test
gate, explicit installed gate and privacy gate are separate from full-workspace
CI and release validation.

## Reproduction and limits

Use the commands and private-directory prerequisites in
[the execution guide](../development_tests.md). A default test count does not
stand in for the explicit installed suite. Failed or incomplete cleanup is an
unknown retained resource, never a reason to change permissions or rerun.

Not established: broker RunDevTests advertisement, daemon attempt/lease joins,
controller configuration exclusion, reusable runtime qualification, independent
assertion verification, remote execution, service-mediated process exclusion,
disk-growth quotas, bounded recursive-cleanup wall time or Linux/macOS operation.
The full personal R&D workflow objective remains incomplete.
