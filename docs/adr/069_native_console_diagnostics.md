# ADR 069: Native Console local diagnostics

- Status: Accepted and implemented for native Windows local diagnostics; installed-tool and native Console interaction qualification remain open
- Date: 2026-11-03
- Related: [ADR 016](016_probe_and_unified_frontend.md), [ADR 029](029_daemon_harness_dispatch.md), [ADR 043](043_tauri_desktop_app.md)

## Context

Console Refresh reads recorded daemon data. Explicit local diagnostics need
separate execution ownership: the shared probe performs version checks, bounded
local route discovery, gateway health and an A2A loopback self-test. Its standalone
in-process timeout/Drop path does not establish joined cleanup of version-child
trees. The native action uses the contained boundary below.

## Implemented I/O boundary

`vibemuxd::local_probe::run(cancel)` resolves only the fixed sibling probe and
trampoline, with empty argv and optional allowlisted environment values. It
holds immutable ordinary-file and root-first ancestor handles with full-width
identities, then repeats verification through an owned blocking task before
release. The bundle remains retained through joined cleanup. Cancellation is
checked after verification and before authorizing the go byte.

Execution after release is bounded to 20 seconds, stdout to 256 KiB and stderr
observations to 64 KiB. The runner closes stdin and watches root exit separately
from stdout EOF. It terminates/reaps the contained tree and completes bounded
pipe drains before publication. Partial spawn failures supply no fabricated
quiescence. Positive report publication requires exit zero, actual joined
cleanup, complete known-Agent coverage, authentication/inference NotRun and the
probe's validated metadata projection. Raw report bytes stay private; no cache
flag is supplied. Non-Windows execution refuses explicitly.

Local validation includes fixed synthetic process cases and an isolated host
that calls this actual public entry point beside exact fixture/trampoline
copies. These checks qualify the Windows I/O boundary. Host and renderer checks
are recorded separately; installed diagnostic behavior remains unqualified.

## App ownership and native contract

The native action runs the fixed bundled `vibemux_probe` CLI under the existing
daemon-library `NativeProcess` trampoline and Windows Job owner. It supplies no
arguments, so the CLI prints its report and performs no cache write. Only native
code resolves the probe/trampoline artifacts and environment allowlist. The
renderer supplies no executable, argv, path, environment, endpoint, model or
provider. Missing or mismatched helpers refuse; no discovery/install fallback
or daemon setup is part of this action.

`AppState` owns `ProbeRegistry` and its worker handles beyond individual Tauri
waiters. It admits one active diagnostic job globally and retains at most 64
operation bindings for the app lifetime. Receipts are never evicted to readmit
an old ID. Capacity exhaustion refuses fresh work until the app is reopened.
An operation binds to the exact registered-project instance. Fresh starts check
its physical identity and hold the registration's job guard during admission.
Forget refuses unsettled jobs. Cancellation can signal an already owned job
after its folder moves; the facade still reports a failed post-check as uncertain.

The existing Main-only `probe_rerun` command now takes the strict body
`{ project_key, request }`. The closed request variants are:

```json
{"operation":"start","operation_id":"11111111-1111-4111-8111-111111111111"}
{"operation":"status","operation_id":null}
{"operation":"cancel","operation_id":"11111111-1111-4111-8111-111111111111"}
```

Status also accepts a non-nil exact operation UUID. A repeated Start returns the
retained operation without launching again. The actual Tauri caller, Main
lifetime and current registration are checked; renderer labels are not authority.
Unknown keys and nil operation IDs are rejected. Snapshot schema 1 has exactly
`schema_version`, `project_key`, `job`, `result`, `can_start` and `blocking_code`.
An optional job contains `operation_id`, `phase` and `error_code`; its phase is
`running`, `completed`, `failed`, `cancelled` or `unconfirmed`. An optional result
contains its own `operation_id` and the validated public report. A completed job
must reference its own report; failures and cancellation may retain an earlier
completed report. Unknown exact-ID status is not proof that a delayed Start
was never admitted.

Worker panic or missing positive quiescence produces Unconfirmed, withholds any
new result and retains the global admission fence. Closing Main stops admission
and requests cancellation. App exit joins owned workers against one absolute
35-second deadline; repeated shutdown and Drop calls do not renew that budget.
Unjoined work remains explicitly unresolved. The native QA runner captures the
managed AppState during setup, checks probe cleanup even without Ready, and gives
unresolved cleanup priority over startup/monitor errors. It preserves Ask counts
separately and persists `cleanup_unconfirmed` for later Stop refusal.

## Console presentation

The native Harnesses section mounts Local checks with explicit Run local checks,
Cancel and Check status controls. Mounting, navigation, ordinary Refresh and
polling perform status reads only. A controller reserves the operation ID before
Start and retains an uncertain operation's original project across navigation.
Status recovery never resubmits it. Cancellation acknowledgement is not stopped
evidence. Bounded polling stops after 60 one-second ticks or view disposal;
manual Check status remains available. Replies cannot regress completed or
Unconfirmed observations to an older Running acknowledgement.

Individual failed checks retain their failed/unavailable states. Existing
cross-platform read-only Console data remains available. The report withholds
paths, endpoints, environment values and arbitrary CLI text and displays its
probe timestamp independently of Console read time. Failed requests preserve
the previous completed report. This action writes no canonical harness state,
dispatch route or trusted probe cache. Fixture-browser mode has no native probe
controller and performs no diagnostics.

## Threat and compatibility boundaries

Containment owns the spawned tree; it is not a filesystem or network sandbox
for installed tools. Fixed `--version` checks request no model turn or login.
Local health responses cannot choose a redirect destination (ADR 016).
Probe metadata is not Task completion, authentication proof or permission to
run a harness. Task children gain no diagnostic execution capability.

This action adds no core IPC operation or store migration. Control v16, store
schema 14, the 32 native commands and Task-child permissions are unchanged.
The previous stub had no successful result shape to migrate. Bundled desktop,
probe and trampoline artifacts must match their documented build/deployment
contract. Keep raw report bytes private and bounded, including on parse failure.

## Verification and limits

Use fixed synthetic children for valid/private report projection, malformed
data, nonzero exit, output bounds, cancellation, deadline and a root that exits
while a descendant holds stdout. Verify actual reap/tree-empty and both pipe
drains. Host tests cover caller/argument rejection, duplicate admission,
cross-project ownership, retained prior results, cancellation, panic, capacity,
moved-folder cancellation and the nonrenewing shutdown budget. Renderer tests
cover uncertain admission, status-only recovery, stale replies, original-project
binding and bounded polling. A QA fault matrix covers unresolved probe cleanup
with panic, missing Ready/owner and ownership/monitor failures. Source review
passed these bounded changes; exact commands and results are in the
[execution record](../evidence/native_harness_m1.md#native-console-local-checks-integration).
The later Surfaces native QA sequence observed Console completion, cancellation,
status and prior-report retention using the fixed in-process NotRun runner.
This qualifies that synthetic UI sequence. Installed tools and Linux/WSL runtime
remain unverified. The earlier cleanup-review GUI image supplies no Console
qualification. See the
[native observation](../evidence/native_harness_m1.md#native-surfaces-interaction-and-observed-gaps).
