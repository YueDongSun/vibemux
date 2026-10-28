# Broker development tests: bootstrap version 2

The worker component now accepts an explicit version-2 owner bootstrap for a
fifth MCP tool, `run_dev_tests`. Version 1 keeps its four file tools and rejects
the execution grant. The standalone development-test command remains available.
This is component implementation, not daemon admission or runtime qualification.

## Owner configuration

Start from [the version-2 example](../examples/worker_tools/bootstrap_with_tests.json).
Its runtime paths are placeholders that must be replaced with exact owner-selected
installed executables. Put it in a new private owner directory with fresh attempt
identity; use the same private bootstrap/scratch/result prerequisites as
[the file-tool guide](worker_tools.md).

The outer version is 2. `worker` contains the existing version-1 snapshot, path
scopes and independent grants. `test_policy` contains absolute `backend` and `node`
paths, 1–8 immutable suites, and `max_runs` from 1 to 16. Each suite has an ID,
explicit existing JavaScript entry files, timeout and combined output limit.
The complete text/path constraints remain in force. Unknown and duplicate typed
fields are rejected directly from the original JSON bytes, including grant and
policy fields. No caller command, environment, preload or reporter field exists.

The tool is advertised only with both `read_files` and `run_dev_tests`. Every
execution input is checked against the admitted read/owned scopes and forbidden
paths. Only the owner selects suites and runtime paths. Client roots or extra
arguments cannot change this authority.

```text
cargo build --locked --offline -p vibemux_worker_tools --bins
target/debug/vibemux_worker_tools --bootstrap <absolute-private-v2.json> --scratch-parent <absolute-private-directory> --candidate-out <new-absolute-private-result.json>
```

The result path must not already exist. Explicit owner configuration is not proof
that its installed runtime is qualified for live work.

## Tool calls and execution identity

A call supplies only a suite ID and the current workspace revision:

```json
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"run_dev_tests","arguments":{"suite_id":"unit","expected_revision":0}}}
```

The initial workspace revision is zero. Version-2 file-tool results include
`workspace_revision`; only successful file mutations advance it. A stale value
refuses admission. The broker verifies all current file bytes, freezes a separate
text snapshot and runs that copy through the fixed offline executor. Test-side
writes are discarded and never enter the editable broker candidate.

The private admission journal records an owner-generated operation ID, suite,
revision and input binding before execution. A suite cannot execute the same
input snapshot again within this owner session, even with a new JSON-RPC ID,
a no-op edit, an ABA edit, a lost reply, cancellation or failed preparation.
There is no automatic retry. A deliberate rerun needs renewed owner admission;
cross-session replay protection and durable leases belong to the daemon.

## Cancellation, busy state and output

One slot stays active through snapshotting, preparation, execution, cleanup and
receipt publication. All other file/test calls are refused as busy, consume their
request IDs and are never queued for later execution. Ping and the immutable tool
catalog remain available.

A matching `notifications/cancelled` notification signals the owned operation and
suppresses its pending result. Integer and string IDs remain distinct. Unknown,
completed and malformed cancellations have no effect. Enqueuing a completed
response is the result-commit point; a later cancellation is ignored. Stdio EOF
cancels and joins the active operation. Partial input, malformed frames, output
failure and deadlines also stop admission and finalize before returning.

The output queue is bounded, with a five-second per-reply deadline and complete
encoded JSONL size checking before writing. The five-minute session deadline
closes admission and requests cancellation; filesystem finalization can outlast
it. Blocking preparation and cleanup remain explicitly owned/joined operations.

The model receives a separate allowlisted projection with operation binding,
execution/capture/cleanup fields, and text labeled untrusted. Owner recovery
locators are excluded. Captured diagnostics can contain execution-environment
metadata and are not public-safe logs. `assertions_verified` remains false.
Process exit zero, TAP text and test counters are not independent acceptance.

## Publication and recovery

Owner output uses outer schema version 2 and carries the ordinary candidate plus
private `test_receipts`. A candidate still requires normal review, verification
and integration into its independent Run worktree.

The session retains its editable workspace by default, including an unpolled
operation future, idle-server abortion or publisher panic. Cleanup is permitted
only after terminal publication of the complete latest candidate succeeds.
Successful recovery/test receipts do not permit deletion. A partial or uncertain
sink failure retains the candidate and original owner marker.

The execution snapshot is separate. Failed workspace-subtree cleanup retains its
owner marker for attribution. Unknown cleanup is recorded with a content-free
OS error code when available; no ACL repair or wider deletion is attempted.
Reconcile the exact retained operation before any retry.

## Verification and remaining gates

```text
cargo test --locked --offline -p vibemux_platform -p vibemux_worker_tools -- --test-threads=1
cargo clippy --locked --offline -p vibemux_platform -p vibemux_worker_tools --all-targets -- -D warnings
cargo fmt --all -- --check
```

Installed-runtime controls are deliberately separate and ignored in the default
suite. Supply explicit `VIBEMUX_DEV_TEST_BACKEND` and `VIBEMUX_DEV_TEST_NODE` paths
before running `test_server_native` or `dev_test_native`. First require the
startup-only marker to pass. Do not infer a working sandbox from a launcher PID
or an empty Job after forced termination.

[The evidence record](evidence/broker_test_validation.md) separates default
regressions, installed-client observations, pipe-lifetime controls and unresolved
MXC startup/cleanup failures. Production retains `node_test_offline_v1`; the
cooperative v2/v3 owner experiments were not qualified and are not shipped.

Remaining work includes daemon attempt/lease binding, protected controller
startup, complete disclosure/runtime qualification, resource quotas and candidate
integration. No live slot is promoted by this component or its receipt format.

## Current native startup diagnosis

The [phase observations](evidence/mxc_startup_phase_observations.md) retain three
failed native controls. A separate 30-second observation did not change the
five-second acceptance predicate or produce a marker. Runtime qualification
remains open; do not rerun the wider matrix without new diagnostic evidence.
