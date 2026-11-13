# ADR 074: Durable goal submission and observable execution

Status: Accepted for the pre-alpha integration, 2026-11-13.

## Decision

The daemon owns goal admission, compilation, preparation and coordinator start.
An HTTP bridge or frontend is only an adapter. It must not create a parallel
database, invoke a provider, retry an uncertain mutation or infer completion
from a successful CLI exit. This extends ADRs 031, 036, 037, 057 and 073.

Control v18 adds these operations, with a matched `vibemuxctl` command:

| Operation | Command | Payload returned by the daemon |
|---|---|---|
| `workflow_submit` | `workflow submit --request-file <path>` | Durable operation receipt |
| `workflow_operation` | `workflow operation <event_id>` | Current receipt for that event |
| `workflow_list` | `workflow list [--limit <1..32>] [--after <workflow_id>]` | Project workflow index with `next` and `more` |
| `workflow_readiness` | `workflow readiness` | Configuration, compiler availability, slot eligibility and stable blockers |
| `workflow_qualify` | `workflow qualify --slot <slot_id> --event-id <uuid>` | Explicit native-worker qualification receipt |

CLI wrappers are respectively `{ "ok": true, "operation": ... }`,
`{ "ok": true, "operation": ... }`, `{ "ok": true, "workflow_index": ... }`
and `{ "ok": true, "readiness": ... }`; qualification uses
`{ "ok": true, "qualification": ... }`. The index contains workflow IDs,
keys, phases, versions, evidence classes, update times and failure codes;
it carries neither absolute paths nor private plans.

The submission document is strict schema 1:

```json
{
  "schema_version": 1,
  "event_id": "13399106-3618-4cd7-9d3f-2e7258dac324",
  "request": { "...": "the existing WorkflowRequest document" },
  "policy": { "...": "the existing OperatorPolicy document" },
  "auto_start": true
}
```

`auto_start` defaults to true. The daemon replaces the supplied request key
with `goal_<event UUID>` and binds the exact request, policy and start choice
to a canonical digest. It validates the envelope, request shape and operator
policy before admission. Changing a body under the same event is a conflict.
Duplicate events return their stored receipt and never create a second owner.
Legacy `workflow intake` remains explicitly prepare-only.

## Ownership and outcome semantics

The writer atomically commits a content-free `accepted` operation and its
audit event before the service spawns work. A maximum of four preparation
owners can exist. The existing owned `JoinSet`, lifecycle admission gate and
shutdown signal supervise them; this is not a detached HTTP request task.

The operation moves from `accepted` to `preparing`, then to `prepared`
(`auto_start: false`), `running`, `blocked`, `failed` or `unknown`. A `running`
receipt proves the deterministic coordinator was started; only the workflow's
canonical `accepted` phase and verification receipts prove task acceptance.
The operation receipt is not a substitute for workflow status.

Proven validation/configuration refusals are `blocked`. Missing writer,
model, process or internal receipts are conservatively `unknown`. Startup
marks interrupted `accepted`/`preparing` owners unknown, associates an already
prepared workflow where possible and never replays effects. Lookup of an
absent event is `workflow_operation_not_found`; absence is not a retry grant.
This ledger currently covers `workflow submit`, not historic intake/follow-up
events. Existing follow-up deduplication and manual uncertain-outcome
reconciliation remain in force.

The compiler reuses the existing model-call ledger, fixed call identity and
single-call budget. On cancellation or deadline the runtime signals the
gateway, continues polling that same future for up to five seconds to allow
contained child cleanup, and records unknown. Late output does not become
success. Service shutdown allows eight seconds for this settlement. An
adapter that does not settle remains unknown; cleanup is not claimed.
The compiler receives a checked complete TaskSpec format example and the
validator's exact UTF-8 clause offsets, literals and numbers. The example is
format guidance, never policy, ownership or task requirements. All generated
candidates still pass the existing structural, source-coverage and policy gates.

## Explicit native compiler

The optional `native_compiler` object in `workflow_config.json` selects the
new `native_codex_exec` model-call protocol. It uses the configured absolute
Codex executable and launch trampoline, an empty working directory outside
the project and Git/config ancestors, a fixed model, an environment name
allowlist, time and output bounds. Configuration and executables are checked
at the local boundary; prompts travel only through stdin. The native module
uses a read-only, ephemeral CLI profile with tools/config integrations
disabled, rejects tool records and requires correlated final output, clean
exit and successful process cleanup.

Native compiler calls require `policy.aag_only: false`. They cannot coexist
with an AAG compiler-role binding. The AAG adapter still refuses unattested
live routing; the native protocol cannot be sent to an HTTP AAG endpoint.
Requested model names and unknown reported usage/provider facts stay distinct.
The CLI does not expose a documented hard generation-token limit: the adapter
bounds time/output bytes and rejects a reported token total above the budget.
It does not pretend that this guarantees a provider-side spending limit.

`native_workers: true` is a separate opt-in for live native worker routes.
It permits a live configuration without an AAG gateway and labels those
routes `native:<harness>:<role>`. It requires `policy.aag_only: false` and
does not itself certify a slot. Codex compilation does not grant Codex write
authority or qualify Claude. Independent worker qualification remains a
separate explicit operation and evidence boundary.

## Explicit live worker qualification

`workflow qualify` is an operator action that may consume at most two native
Claude turns, each bounded to 45 seconds plus owned process cleanup. Its CLI
deadline is 240 seconds. Readiness and doctor never call it. It requires a
configured live native route, no active workflow and an unleased Claude slot.
The opaque event ID derives both dispatch IDs; any previously admitted attempt
is observed as `qualification_previously_attempted` and never replayed. A fresh
operator event is required to repeat a failed or interrupted qualification.

The daemon creates an independent owned worktree and unguessable canary input.
The positive turn must successfully use read/search/edit/write and the exact
`node --test` command; tool uses need correlated, successful tool results.
The read-only negative turn must expose only Read/Glob/Grep (plus the inert
EndConversation tool when retained by the CLI), and leave the canaries intact.
Inventory, files and expected test bytes are verified by Rust. This proves the
bounded native profile behavior; it does not make a Git worktree an OS sandbox.

Only canonical completed dispatches with successful owned process cleanup may
lead to canary cleanup. Failed, uncertain or externally modified worktrees are
retained and confer no capability. Successful qualification records one hour
of `write_mode_certified` evidence for structured turns and writable worktrees,
bound to workflow configuration, dispatch configuration, actual CLI bytes,
trampoline bytes and profile identity. Readiness, start and every native turn
recheck that binding; stale evidence is downgraded in the writer and the current
snapshot. Restart requires fresh qualification. Cancellation capability is not
inferred from the file canaries.

The Claude writable profile is version 2 and requires a CLI with `--safe-mode`,
which retains subscription authentication while disabling user customizations.
It keeps `--restricted`, strict MCP configuration, `dontAsk` and the exact
granted tool allowlist. It does not use `--bare`, which disabled the subscription
authentication path in earlier repository evidence. The installed CLI still
must pass qualification; recognizing a command or an auth status is insufficient.
See the [official CLI reference](https://code.claude.com/docs/en/cli-reference)
and [historical native evidence](../evidence/harness_dispatch_validation.md).

## Native host setup

Build the matched CLI, daemon and containment trampoline together in the
VibeMux checkout:

```sh
cargo build -p vibemux_cli --bin vibemuxctl -p vibemuxd --bin vibemuxd --bin vibemux_launch_trampoline
```

All three are in `target/debug` (`.exe` on Windows). Keep the trampoline beside
the daemon and CLI when copying an installation. Its source is the auto-binary
`crates/vibemuxd/src/bin/vibemux_launch_trampoline.rs`; installing only the CLI
and daemon leaves contained launches unavailable. Use native executables, not
`.cmd`/`.bat` wrappers. The application repository is the project being worked
on, distinct from the VibeMux checkout and these trusted installed programs.

Inspect the actual Claude version and capability on the target host before
configuring execution. For example, in PowerShell:

```powershell
$Claude = 'C:\Tools\Claude\claude.exe'
& $Claude --version
$ClaudeHelp = (& $Claude --help 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0 -or $ClaudeHelp -notmatch '--safe-mode') {
    throw 'This writable profile requires Claude Code with --safe-mode.'
}
```

Record the displayed version. No numerical minimum Claude version has been
live-validated by this change, so the required advertised flag and the actual
qualification are the gates; a version string alone is insufficient. Do not
remove the profile flags to accommodate an older binary. For Codex, inspect
`codex --version` and `codex exec --help` and select an explicit model available
to the installed account. The compiler requires the read-only, ephemeral and
ignore-user-config options used by `native_provider.rs`.

The following are complete configuration shapes for a native Windows host.
Replace the example absolute paths and both `YOUR_ALLOWED_*_MODEL` values before use. The
Git, Node, CLI, trampoline and trusted verifier paths must exist outside the
application project. The compiler directory must already exist, be empty, and
have no ancestor containing a Git repository or `.codex/config.toml`.

In the application project's private `.vibemux/harness_dispatch.json`:

```json
{
  "schema_version": 1,
  "routes": [{
    "harness": "claude",
    "protocol": "claude_stream_json",
    "executable": "C:/Tools/Claude/claude.exe",
    "model": "YOUR_ALLOWED_CLAUDE_MODEL",
    "enabled": true,
    "allow_execution": true,
    "environment_names": ["PATH", "USERPROFILE", "APPDATA", "LOCALAPPDATA"]
  }],
  "limits": {
    "frame_bytes": 131072,
    "capture_bytes": 16777216,
    "record_count": 16384,
    "deadline_ms": 120000,
    "shutdown_grace_ms": 2000
  }
}
```

In `.vibemux/workflow_config.json`:

```json
{
  "schema_version": 1,
  "evidence_class": "live",
  "git_executable": "C:/Program Files/Git/cmd/git.exe",
  "slots": [
    {"slot_id": "worker_a", "harness": "claude", "route_role": "worker_a"},
    {"slot_id": "worker_b", "harness": "claude", "route_role": "worker_b"},
    {"slot_id": "review", "harness": "claude", "route_role": "reviewer"}
  ],
  "verifier": {
    "directory": "C:/VibeMuxPrivate/verifier",
    "executable": "C:/Program Files/nodejs/node.exe",
    "suites": {
      "checks": ["{verifier_dir}/verify.mjs", "--candidate", "{candidate}", "--out", "{out}"]
    },
    "timeout_ms": 120000,
    "environment_names": ["PATH"]
  },
  "gateway": null,
  "native_compiler": {
    "schema_version": 1,
    "executable": "C:/Tools/Codex/codex.exe",
    "trampoline": "C:/Tools/VibeMux/vibemux_launch_trampoline.exe",
    "working_directory": "C:/VibeMuxPrivate/compiler-empty",
    "model": "YOUR_ALLOWED_CODEX_MODEL",
    "environment_names": ["PATH", "USERPROFILE", "APPDATA", "LOCALAPPDATA"],
    "max_output_tokens": 8192,
    "timeout_ms": 120000
  },
  "native_workers": true,
  "heartbeat_ms": 30000,
  "candidate_limits": {"max_files": 256, "max_file_bytes": 1048576, "max_total_bytes": 8388608},
  "content_retention_days": null
}
```

The example `checks` verifier must be replaced by an actual operator-owned
verifier implementing [the workflow verifier contract](031_dual_track_workflow.md)
and the result shape accepted by [the verifier runner](../../crates/vibemuxd/src/workflow/verifier_runner.rs).
It is not a supplied passing test. Match its suite ID in the OperatorPolicy and
workflow request. That policy must explicitly set `aag_only: false` for native
execution, while retaining appropriate writable roots, protected paths, tool
grants and budget caps. See [the operator request guide](../guides/superagent_workflow.md).

Only environment **names** appear in these private files. Their values come
from the daemon environment and are never copied into operation receipts.
`SYSTEMROOT`, `WINDIR`, `TEMP` and `TMP` are automatically forwarded when present;
all other needed names must be explicit. On POSIX replace the Windows account
names with existing `HOME` and `PATH` names. Add a required vendor credential or
configuration-variable name only when it is deliberately supplied to the daemon;
never put its value in this JSON. A missing explicitly named variable refuses
launch. `PATH` must find a trusted Node installation for the qualification test.
Using a native account login still requires the appropriate account-directory
environment; an empty allowlist does not inherit the whole interactive shell.

After starting the matched daemon for the application project, inspect routes
and qualify each configured slot with a **different new** event UUID. For example:

```powershell
$Ctl = 'C:\Tools\VibeMux\vibemuxctl.exe'
$Project = 'C:\Projects\application'
& $Ctl harnesses --project-root $Project
& $Ctl workflow readiness --project-root $Project
foreach ($Slot in @('worker_a', 'worker_b', 'review')) {
    $EventId = [guid]::NewGuid().ToString()
    & $Ctl workflow qualify --slot $Slot --event-id $EventId --project-root $Project
    if ($LASTEXITCODE -ne 0) { throw "Qualification request failed for $Slot" }
}
& $Ctl workflow readiness --project-root $Project
```

Read each `qualification.qualified` field: exit zero can report a refused or
previously attempted qualification with `qualified: false` and a stable code.
Do not submit a goal until the actual slots and verifier meet readiness. The
compiler-available flag still does not prove a model completion. Preserve event
IDs to identify interrupted attempts, and do not retry them as if they were
unseen. After a daemon restart all slots return to declared evidence; old
qualification event IDs do not replay the probes. Use a new explicit event to
qualify again. Evidence expires after one hour; changed CLI or trampoline bytes invalidate
it before the next native turn. Configuration remains pinned for the daemon
lifetime: restart loads edits and revokes all prior qualifications. The later actual goal remains subject to
its own compiler, contract, independent review and verifier checks.

## Persistence and compatibility

Schema 15 adds `workflow_operations` and its index in an immediate transaction.
Rows and append-only `workflow_operation_updated` events commit atomically.
Every read checks its projection against the event. The migration retains
existing workflows, events and reports; it never resets a user database.

The unmerged native-conversation branch used Control versions 12–17 and store
versions 10–14 with different meanings. This integration reserves those
numbers and uses Control 18/schema 15. A schema-10 database must have the
ADR 057 worker-report lineage before it can advance; incompatible native
branch schemas are refused as `store_schema_lineage_incompatible`. No format
is silently reinterpreted. Integrating that native-session branch later needs
a deliberate data migration and another protocol revision, not cherry-picking
its numeric version constants. Rollback needs a separately retained backup
and a stopped daemon. Older clients/daemons are not a matched installation.

## Related lifecycle fix and validation

The selected execution fixes from PR 56 make `NativeProcess::finish` fallible
and idempotent. Failed tree termination cannot produce a completed worker or
a passing verifier. Other desktop/native-conversation scope is not imported.
`daemon stop --expected-process-id <pid>` checks health and submits shutdown
through one captured authenticated descriptor. A different owner is refused;
the CLI never reloads a replacement descriptor to shut it down.

Validation covers store durability, conflicts, migration lineage, tamper
rejection, paged recovery, real in-process writer/worker/worktree/review/verifier
goal execution, workflow pagination, prepare-only submission and cooperative
gateway cleanup. These are separate from native provider execution and the
IPC/browser transport. The managed validation host denies AF_UNIX sockets;
existing IPC tests remain required on a normal native host. Native Windows,
real vendor qualification and full release gates must be reported separately.
On this validation host Codex 0.167.0-alpha.4 reported a ChatGPT login, but the
bounded compiler-profile probes produced no model completion. Claude was not
installed. Neither provider's live goal execution or qualification is claimed.
