# Offline development-test component

`vibemux_dev_test` runs explicit Node test entries in a fresh disposable copy.
It is an owner-only component for development feedback and qualification.
The version-1 worker MCP surface exposes read, search, create and edit.
An explicit [version-2 broker session](broker_development_tests.md) now adds test dispatch. See [ADR 045](adr/045_disposable_development_tests.md).

## Input and disclosure contract

The strict version-1 request contains an attempt ID, operation ID, suite ID,
the existing independent grants, explicit read/forbidden scopes, text-file records,
test entries, and time/output limits. Both `read_files` and `run_dev_tests` must
be true. Every supplied input needs a read scope, including dependencies that are
not selected entries. A forbidden path still wins. Running a program can reveal
its readable inputs through output, exit status or timing; granting Run alone is
therefore insufficient for this feedback interface.

Existing text limits apply: at most 256 files, 64 KiB per file, 4 MiB in total,
and the closed relative-path grammar. Up to 16 explicit `.js`, `.cjs` or `.mjs`
entries must already exist in those records. The internal `.vibemux_test_scratch`
namespace is reserved. The timeout is 1–60,000 ms; combined stdout/stderr retention
is 1–65,536 raw bytes. Lossy UTF-8 representation may expand invalid bytes by up
to three times that raw limit. Serialized JSON escaping can expand it further.

There are no shell-command, environment, preload, reporter, package-install,
network or retry fields. The owner selects the exact runtime executables outside
the request. The fixed Node arguments are:

```text
--preserve-symlinks --preserve-symlinks-main --test --test-reporter=tap --test-concurrency=1 -- ./<entry> ...
```

The owner must keep runtime paths and the private parent stable throughout the
invocation. This API accepts admitted bytes; it does not discover a checkout or
decide which repository content the model is entitled to disclose.

## Build and run

Build with the repository-pinned Rust toolchain and existing lockfile:

```text
cargo build --locked --offline -p vibemux_worker_tools --bins
```

Copy [the example request](../examples/worker_tools/development_test.json) into an
owner-private directory and assign fresh attempt/operation IDs. The request file,
scratch parent and output parent must pass the component's existing private-path
checks. Use an explicitly selected installed backend and Node executable:

```text
target/debug/vibemux_dev_test --request <absolute-private-request.json> --scratch-parent <absolute-private-directory> --backend <absolute-codex.exe> --node <absolute-node.exe> --report-out <new-absolute-private-report.json>
```

The command reserves a new report path before constructing the snapshot. An
existing report is refused without executing again. It never changes saved MXC
settings or falls back to direct execution. Zero means completed zero-exit
execution with complete capture, confirmed process/Job shutdown and successful
snapshot removal. Any other result or infrastructure error exits 2. Neither exit
code certifies assertions or workflow acceptance.

The Node 24.18.0 / Codex CLI `0.164.0-alpha.11.1` combination was exercised on
native Windows. Other installations require their own controls. Non-Windows
preparation returns `worker_tools_execution_unsupported`.

## Owner integration and recovery

The library has three explicit ownership steps:

1. `prepare_dev_tests(request, runtime, private_parent)` validates and constructs
   the copy synchronously, before any candidate process exists.
2. `PreparedDevTest::execute(cancellation)` supervises the contained Job
   asynchronously. Sending true or closing the watch sender requests cancellation.
   Await this future through finalization; there are no detached reader tasks.
3. `ExecutedDevTest::finish()` performs synchronous removal after quiescence.
   An async daemon must own and join these blocking preparation/cleanup boundaries.

The owner marker is outside the candidate-writable directory. It binds a retained
envelope to attempt, operation, suite and admitted-input identity. Dropping the
execution future invokes child/Job emergency termination but cannot prove reaping;
the envelope remains. Cancellation delivered through the channel instead waits
for bounded process reconciliation and reports what happened. No retained tree is
automatically retried or reused.

The report separates `execution`, `backend_exit_code`, `process_started`,
`process_exited`, `process_tree_empty`, `output_complete` and `snapshot_removed`.
`untrusted_stdout` and `untrusted_stderr` retain their candidate-origin trust level.
`assertions_verified` and `reusable_qualification` are always false. `completed`
also applies to a naturally exiting nonzero process; inspect the exit field.
Timeout/cancellation/output-limit classification is not replaced by a cleanup
result. A failed removal leaves `retained_directory` for the owner.

The CLI stages and syncs its private report before publication. An empty reserved
report after interruption or publication failure is an unknown outcome, not an
invitation to rerun. Distinct result paths are not an operation-ID registry:
durable deduplication and lease binding belong to the integrating daemon.

Cleanup uses the original envelope and Rust's reparse-safe removal after the Job
is empty. It does not repair permissions, follow candidate cleanup manifests or
broaden deletion to a parent. Execution and capture have deadlines/limits;
test-created disk growth and recursive cleanup wall time do not have enforced
quotas in this component.

## Verification

Default offline component checks do not launch an installed sandbox:

```text
cargo test --locked --offline -p vibemux_worker_tools
cargo clippy --locked --offline -p vibemux_worker_tools --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: the contract/component suites pass; installed-runtime tests are listed
as ignored with an explicit reason. Run the native controls deliberately in
PowerShell, with absolute executable paths supplied by the owner:

```powershell
$env:VIBEMUX_DEV_TEST_BACKEND = '<absolute-codex.exe>'
$env:VIBEMUX_DEV_TEST_NODE = '<absolute-node.exe>'
cargo test --locked --offline -p vibemux_worker_tools --test dev_test_native -- --ignored --test-threads=1
```

The native suite exercises the real binary/example, fresh-output refusal,
disposable mutation, outside-file denial, independently live IPv4/IPv6 endpoints,
assertion failure and forged exit zero, timeout, combined output overflow,
cancellation before/after launch, lost sender, unpolled-future retention,
descendants with closed/inherited pipes, junction-creation denial, and cleanup
failure while the owner marker is held open. Parent canaries and Job accounting
are independent of candidate output. Candidate junction creation was denied by
the tested backend; that observation does not claim a created junction was removed.

## Migration and remaining acceptance

The standalone command and owner API coexist with the explicit version-2 broker
integration. `default-run` remains `vibemux_worker_tools`; Bootstrap v1, the native
writable profile, Control API, database and workflow slot state are unchanged.
Do not set RunDevTests in a version-1 bootstrap and assume it is supported.

Remaining integration requires broker snapshot extraction, immutable suite
selection, daemon attempt/lease admission, controller configuration exclusions,
runtime/profile qualification bound to each launch, and candidate review and
integration. A trusted assertion oracle must remain outside candidate execution.
Service-mediated process escape, enforced disk/resource budgets, interrupted-owner
reconciliation and non-Windows support are not established by the focused controls.
