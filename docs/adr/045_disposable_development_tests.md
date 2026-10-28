# ADR 045: Disposable offline development-test execution

Status: Accepted for component implementation and qualification (2026-10-27).
Daemon admission, broker tool integration and reusable qualification are pending.
Decision disposition: auto-approved within the requested implementation scope.

## Context

ADR 044 forbids running candidate code inside the editable broker workspace:
code execution there would bypass independent edit/create grants. A Windows
experiment established that Node 24.18.0's ordinary file loader tries to resolve
the drive root, which an offline root-deny MXC profile refuses. The fixed
`--preserve-symlinks --preserve-symlinks-main` flags allowed the explicit test
entry to start under the same policy without adding a drive-root read grant.

Node test reports and exit status originate in the candidate execution boundary.
A candidate can exit zero before assertions or forge output. They are development
feedback, not an independent assertion oracle. Even exit status can encode any
input readable by the candidate, so removing stdout alone does not establish
disclosure control.

## Decision

Add a separate owner-only `vibemux_dev_test` entrypoint and library API to
`vibemux_worker_tools`. Keep its existing MCP and bootstrap schema unchanged;
they still reject RunDevTests. The executor requires both ReadFiles and
RunDevTests, with every admitted file inside an explicit read scope and outside
forbidden paths. No edit or create grant is needed for mutations of its disposable
copy; those mutations are discarded and never enter a candidate export.

The owner supplies bounded name/text records, an attempt and operation ID,
an explicit suite ID and existing test entries, and bounded time/output limits.
There is no request field for commands, environment, reporters, preloads, network
targets or retry policy. The runtime is an owner-selected canonical Node executable
and the installed Codex MXC backend. Arguments are fixed, with a delimiter before
explicit relative entry paths. Existing platform code constructs the offline
root-deny policy and clears the environment to its bootstrap allowlist.

Preparation creates a fresh private envelope, with an owner record outside the
candidate-writable copy. Each admitted file is newly created from bytes. A consumed
input-snapshot identity binds the report to those records. No existing checkout,
link or controller directory is adopted. The owner must protect runtime paths and
the envelope's parent throughout execution; this is not hostile-same-user isolation.

A contained launch trampoline waits for its release byte until it belongs to the
owned Windows Job. Cancellation, timeout and output overflow stop that whole Job.
Root exit, Job emptiness and output completion are separate observations. Capture
has one combined byte limit, drains both pipes and retains no detached reader task.
Normal root exit also terminates remaining descendants before cleanup. Cancellation
uses an explicit channel; dropping the future triggers best-effort kill-on-close
and retains the envelope, without claiming completed reaping or cleanup.

Preparation and cleanup are synchronous ownership boundaries; async supervision
does not run recursive filesystem operations. A daemon embedding must own and join
their blocking work. Cleanup uses Rust's reparse-safe removal of the original
envelope only, after confirmed quiescence. Unknown cleanup retains its exact path.
There is no recursive ACL repair, automatic retry or reuse of a retained snapshot.

Reports label output untrusted and always set `assertions_verified` and
`reusable_qualification` false. Zero CLI exit means clean execution and cleanup,
not workflow acceptance. Independent verification requires a separate trusted
oracle outside the candidate's authority. The AAG transport receipt, daemon lease,
candidate review and integration gate remain independent obligations.

## Alternatives and limits

- Running native shell/test tools in the broker workspace breaks its operation
  contract and is rejected.
- Granting the whole drive read access to satisfy Node's loader is unnecessary
  for the observed fixed profile and is rejected.
- Parsing TAP or a candidate-written JSON report cannot upgrade its trust.
- A new sandbox framework or custom unsafe recursive remover is unnecessary;
  existing MXC, Job/trampoline and Rust cleanup APIs provide the component seams.
- Execution and capture are bounded. Test-created disk usage and recursive
  cleanup wall time are not bounded by this component. Service-mediated process
  escape, hostile same-user interference, controller configuration exclusion and
  runtime replacement remain qualification concerns.
- The installed backend and Node controls are observations for those artifacts;
  they are not reusable grants or proof of non-Windows behavior.

## Primary evidence

- [Node 24.18 test execution model](https://nodejs.org/download/release/v24.18.0/docs/api/test.html#test-runner-execution-model)
  and [official runner source](https://github.com/nodejs/node/blob/v24.18.0/lib/internal/test_runner/runner.js).
- [Windows Job accounting](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_accounting_information)
  and [asynchronous Job termination](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-terminatejobobject).
- [Pinned Rust 1.92 Windows cleanup implementation](https://github.com/rust-lang/rust/blob/1.92.0/library/std/src/sys/fs/windows/remove_dir_all.rs).
- ADR 044 records the preceding GitHub MCP investigation and arXiv CaMeL grounding.
  They motivate capability checks outside model interpretation and are not
  qualification evidence for this executor.

See [the execution guide](../development_tests.md) for exact commands, report
semantics, verification and remaining migration work.
