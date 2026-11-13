# Optional supervisor bridge

A process-external HTTP adapter for VibeMux control commands. It lets a separate
authenticated UI inspect tasks and workflows, submit an already specified
workflow, and send a bounded follow-up to an existing worker. The Rust daemon
remains the only canonical task/workflow writer and scheduler.

This integration uses only Node built-in modules. It has no database, model
client, worker launcher, local task state, persistent draft store, or production
fixture fallback. It does not start a daemon automatically. A task activity view
comes from the daemon's bounded event metadata, not a full transcript.

## Compatibility and qualification

The adapter was checked against these source contracts:

| Backend source | Available through this bridge |
| --- | --- |
| Main `a06a8c70a4e68bf78c160789d3b818e58f0a4ab1` | Task list/detail, workflow status, explicit pause/cancel |
| Native chat PR #49, `b80e04192fada79a69f7314fafc69824f01f38ad` | No intake/follow-up/report capability inferred from Control v17 |
| Superagent PR #52, `f91f0feb0060a4df346fee3c7828a83e112c90af` | Intake/follow-up/reports only when those verbs are actually advertised |
| Daemon async-operation increment | Goals, receipt lookup, workflow index and readiness only when the corresponding CLI verbs are advertised |

The bridge probes `vibemuxctl --format text workflow --help`, caches the
advertisement for five seconds, and checks an operation before invoking it.
Control version numbers are deliberately insufficient: the two open branches
assign different operations to version 11. No branch is merged by installing
this adapter. A compatible CLI and daemon must still be installed as a matched
set. Capabilities say `daemon_qualification: "unverified"`; the next command can
still fail because the daemon is unavailable, has a different protocol, or
refuses the request.

Source checks and synthetic CLI/HTTP tests do not establish a working model
backend. In the cited main revision, the production AAG gateway refuses live
calls with `LiveAttestationUnavailable` before submission. This bridge does not
change that gate or make a model supervisor production-qualified.

The [daemon operations guide](docs/daemon_operations.md) describes the additive
async-goal, receipt-lookup, workflow-index and runtime-readiness contract.
These routes preserve daemon ownership: HTTP 202 acknowledges durable goal
admission, and a later read observes the daemon's operation. Runtime qualification
comes from the matching daemon rather than an inferred Control version.

## Run locally

Use Node 22.22.0 or newer and a trusted, installed `vibemuxctl` executable. There
are no packages to install. Run the CLI and daemon as the same OS user.

1. Copy `config.example.json` to a private file outside the repository. Replace
   the executable and project roots with absolute paths. On Windows the
   executable must be a native `.exe`; `.cmd`, `.bat`, and `.ps1` launchers are
   refused. On Unix use the installed executable's absolute path.
2. Set `VIBEMUX_BRIDGE_TOKEN` in the bridge process environment to an independently
   generated high-entropy secret, at least 32 visible ASCII characters. The
   config stores only the environment-variable name. Do not reuse the
   read-only monitor's bearer. Protect the private configuration and environment.
3. Start the existing daemon through VibeMux's normal operator flow, then run:

   ```sh
   node integrations/superagent_bridge/src/main.mjs --config /absolute/private/bridge.json
   ```

   In native PowerShell, pass the private Windows config path to the same Node
   entry point. The server binds only to `127.0.0.1`, on port 4317 by default.
   A trusted Node parent can create an IPC channel: the bridge sends
   `{type:"superagent:ready",pid,port}`, accepts a `superagent:shutdown` message,
   and closes when that parent disconnects. This is process-local lifecycle
   support for a foreground stack launcher, not an HTTP shutdown route.
4. For an authenticated Pixel server on the same host, set
   `PIXEL_SUPERAGENT_BRIDGE_URL=http://127.0.0.1:4317` and set
   `PIXEL_SUPERAGENT_BRIDGE_TOKEN` to the same secret. Pixel's server-side
   `/api/superagent/...` proxy maps allowlisted routes to `/api/v1/...`. The
   browser receives neither the bridge credential nor the project filesystem
   roots. The Pixel server must authenticate its own users before proxying.

Default configuration is read-only. Set `enable_writes: true` to permit
supported writes. Imported requests additionally need the target project's
`allow_import: true`; report text additionally needs `allow_report_content: true`.
The example has `allowed_origins: []`, suitable for a server-to-server proxy
without an Origin header. For a trusted client that sends Origin, configure its
exact origin. Cross-site browser requests are refused and no CORS permission
is emitted. Restart after changing private configuration.

## HTTP contract

Every route requires `Authorization: Bearer <secret>`. The configured project
alias is the only project selector; requests cannot supply paths, binaries,
command names, provider credentials, or executable arguments.

| Method and path under `/api/v1` | Result or input |
| --- | --- |
| `GET /projects` | `{ok:true,projects:[{id,label}]}` |
| `GET /projects/:id/capabilities` | Effective CLI/write capabilities and public server policy |
| `GET /projects/:id/readiness` | Layered CLI, daemon and workflow execution diagnostics |
| `GET /projects/:id/workflows?limit=20&after=<workflow_uuid>` | `{ok,project_id,workflow_index:{schema_version:1,workflows,next,more}}` |
| `GET /projects/:id/operations/:eventId` | Read-only goal operation lookup; unknown results are never replayed |
| `POST /projects/:id/goals` | Durable daemon goal admission, HTTP 202; optional `auto_start` defaults to true |
| `GET /projects/:id/tasks?limit=20&after=<task_uuid>` | `{ok,project_id,canonical_project_id,tasks,next_cursor}`; limit 1–32 |
| `GET /projects/:id/tasks/:taskId` | `{ok,project_id,task,runs,events,artifacts,has_more_runs,has_more_events,has_more_artifacts}` |
| `GET /projects/:id/workflows/:workflowId` | `{ok,project_id,workflow}`; workflow status schema 2 |
| `GET /projects/:id/workflows/:workflowId/reports?after_sequence=0&include_content=false` | `{ok,project_id,worker_reports:{schema_version:1,reports,next,more}}` |
| `GET /projects/:id/workflows/:workflowId/attention?after_sequence=0&include_content=false` | Historical attention claims, described below |
| `POST /projects/:id/intake` | Imported request/policy, or a configured goal preset |
| `POST /projects/:id/workflows/:workflowId/followup` | Exact follow-up envelope below |
| `POST /projects/:id/workflows/:workflowId/pause` | Empty JSON object; existing daemon phase-control receipt |
| `POST /projects/:id/workflows/:workflowId/cancel` | Empty JSON object; existing daemon phase-control receipt |

Unknown envelope/query fields, duplicate query parameters, and invalid
identifiers are refused. Unsupported operations return HTTP 501 with
`operation_unsupported`; keep the client's draft.

### Capabilities and task detail

An illustrative capability response, not a production observation:

```json
{
  "ok": true,
  "project_id": "example",
  "source": "cli_help",
  "observed_at": "2026-11-12T00:00:00.000Z",
  "daemon_qualification": "unverified",
  "capabilities": {
    "tasks": true, "task_detail": true, "workflow_status": true,
    "intake": false, "followup": false, "reports": false,
    "pause": false, "cancel": false,
    "goals": false, "workflow_index": false, "operation_lookup": false,
    "runtime_readiness": false, "current_attention": false
  },
  "server_config": {
    "allow_import": false, "allow_report_content": false, "presets": []
  }
}
```

Write capabilities already include `enable_writes`. An imported-intake button
needs both `capabilities.intake` and `server_config.allow_import`. Presets expose
only `{id,label}`, never their private policy/request objects.

Task rows retain `task_id`, canonical `project_id`, `title`, `title_truncated`,
`status`, `latest_sequence`, and `detail_revision`. Detail is limited to the
daemon's 12 runs, 24 events and 24 artifacts, with its `has_more_*` flags.
Absolute run worktree paths are removed. Events are
`{sequence,event_type,at,run_id}`. These are bounded activity metadata, not a
complete event replay or structured completion inference.

There is no task-row `workflow_id`. Use an explicit bookmarked workflow ID or
the intake receipt's `prepared.workflow_id`, never assume a task ID is also a
workflow ID. Workflow display fields include `phase`, `version`, `mode`,
`evidence_class`, `blocked_reason` and `running_here`. Task contract identities
are in `workflow.tasks`; worker identities are in `workflow.sessions`. The
daemon rechecks all of them during follow-up admission.

### Intake prepares a workflow

Imported intake accepts exactly `{event_id,request,policy}`, where `event_id`
is a UUID and the two objects are an existing valid WorkflowRequest and operator
policy. Obtain those objects through the existing preparation process. The
bridge adds `schema_version: 1` to the daemon envelope, preserves the event ID,
and passes the exact objects. The daemon derives its request key and owns
duplicate detection, source spans, contract admission and policy checks.

On a matching #52 backend, success is
`{ok:true,project_id,event_id,intake:{schema_version:1,event_id,authority,prepared,mandate}}`.
`prepared` contains `workflow_id`, `phase`, `start_contract`, `contracts`,
`narrowing_count`, and `duplicate`. The operation does not start, resume, or
select a driver. A duplicate returns the daemon's existing preparation receipt;
its current phase is not evidence of a new start by the bridge.

An operator may configure private presets `{id,label,request,policy}`. A goal
submission is exactly `{event_id,goal,preset?}`. It selects one configured preset
(the sole preset may be implicit), clones it, and injects the goal only into
`request.request_text`. Preset `task_specs` must be empty or absent because
precompiled source spans cannot safely be reused with new text. This path
requires the daemon's already configured compiler and normal admission gates.
It is not a new natural-language compiler; an unavailable compiler or live
qualification is a backend refusal. No preset is enabled in the example.

### Follow-up keeps the exact context

The body is the daemon's exact schema, including a caller-owned event ID:

```json
{
  "schema_version": 1,
  "event_id": "00000000-0000-4000-8000-000000000004",
  "workflow_id": "00000000-0000-4000-8000-000000000002",
  "expected_workflow_version": 7,
  "task_key": "track_a",
  "session_id": "00000000-0000-4000-8000-000000000003",
  "contract_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "contract_version": 1,
  "kind": "question",
  "text": "A synthetic example of bounded follow-up context."
}
```

These are synthetic identifiers. Use the real current status for real work.
`kind` is `question`, `progress`, or `finding`; text is nonempty and at most
4096 UTF-8 bytes. Versions are positive safe JSON integers; `contract_version`
also fits `u32`. Path and body workflow IDs must match.

Success is
`{ok:true,project_id,event_id,followup:{message,duplicate,context_only:true}}`.
The receipt is checked against the requested workflow, contract, worker
session, kind and text digest. It means context was admitted for an existing
cooperative worker's next allowed turn. It does not prove delivery,
acknowledgement, resumption, or task completion. Missing contract/session/version
information stays a client draft until the operator can supply it.

### Reports and attention are claims

Reports preserve the daemon's immutable `claim` and, on an explicitly allowed
content read, its optional `content`. Content remains subject to the daemon's
ownership, opt-in retention, expiry, purge and digest checks.

Attention returns
`{ok,project_id,semantics:"report_claims",attention,next,more}`. Each item contains
`sequence,task_key,session_id,state,needs_attention,summary,human_action`.
Only reports in the requested page with `needs_attention: true` appear.
Unavailable text is `null`. This is historical declarations, not an
authoritative unresolved inbox: an older blocked claim can remain in history
after a later ready claim. Label the view accordingly.

Continue with `after_sequence=next` while `more` is true, including when an
attention page is empty. A worker's `ready` claim never becomes `completed` or
daemon acceptance. A client that shows only one page must disclose that limit.

### Failure and retry semantics

Errors are `{ok:false,error:{code,message,outcome},event_id?}`:

- `not_started`: bridge validation, authorization, admission capacity, missing
  capability, or executable startup prevented this command from starting.
- `rejected`: a known daemon refusal, or a failed read.
- `unknown`: a mutation was possibly submitted but its matching receipt cannot
  be confirmed, including timeout, output overflow, invalid output, uncertain
  transport failure or incomplete local request-file cleanup.

Raw CLI stderr, filesystem paths and input text are not returned in errors.
The bridge does not retry commands. Client and proxy must not automatically
retry mutations or silently replace event IDs. After an unknown result,
preserve the original draft, event ID and expected context; inspect canonical
daemon state before the operator decides whether an exact replay is appropriate.
Changing project selection must not transfer an in-flight draft or its
unknown outcome to another project. A disconnected browser does not cancel a
workflow; pause/cancel are separate explicit operations.

## Resource and credential boundary

Calls use the configured executable and an argument array with `shell:false`.
The child environment allowlists OS essentials and removes the bridge token,
provider keys, and interpreter/library injection variables. Text is placed in
an exclusively created temporary JSON file, never a command string. The file
and directory use `0600`/`0700` on Unix; the current user's protected temp-directory
ACL is the Windows privacy boundary. Cleanup touches only those exact owned
paths. Abrupt termination can leave a private file; no broad cleanup is attempted.

Defaults are four concurrent CLI calls, 512 KiB combined stdout/stderr,
56 KiB HTTP/envelope input, 48 KiB workflow request, 8 KiB policy, 64 KiB
configuration, 24 active HTTP requests, and a 125-second CLI deadline. Configure
a server-side proxy timeout of at least 135 seconds for that default. VibeMux's
slow control operation deadline is 120 seconds. Shorter caller timeouts can
produce unknown outcomes. Request upload has a separate ten-second limit.

The bridge owns only its short-lived control CLI children. Timeout/shutdown
signals those children, never the daemon or its workers. An unconfirmed child
exit retains its concurrency/ownership slot; shutdown reports incomplete
cleanup instead of claiming it was reaped. This adapter does not supply Windows
Job Object or vendor-process containment and does not expose CLI verbs that
launch workers, terminals, daemon processes or arbitrary subprocesses.

The bearer authorizes every alias in this configuration. It is not a per-user
or per-project role system. This loopback service is intended to sit behind an
authenticated same-host application server. It does not provide TLS, LAN
exposure, mobile enrollment, origin-based identity, or the monitor's read-only
token flow. These properties cannot be inferred from the existence of the API.

## Tests and evidence

```sh
cd integrations/superagent_bridge
node --test tests/*.test.mjs
```

Tests launch actual synthetic CLI child processes and an actual loopback HTTP
server. They cover main/#49 capability absence, #52 verb advertisement, read
projection, fixed project roots, prepared receipts, exact/duplicate follow-ups,
unknown mutation outcomes, auth/origin/host checks, body/output/time limits,
private-file cleanup and historical attention semantics. Fixtures are explicitly
marked synthetic and imported only by tests.

See [ADR 073](../../docs/adr/073_process_external_superagent_bridge.md) and
[PROGRESS.md](../../PROGRESS.md) for execution evidence and platform limits.
This change has no Cargo dependencies, store migration, Control version bump,
or modification to the existing read-only web monitor.
