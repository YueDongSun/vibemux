# Daemon-owned goals and operation lookup

These additive endpoints use actual daemon CLI contracts. They do not add a
Node task queue, operation ledger, compiler, scheduler or persistence layer.
The existing `/intake` endpoint remains synchronous and prepare-only. The new
`/goals` endpoint uses a daemon-owned background operation.

## Capability negotiation

The existing capability response adds these booleans:

| Capability | CLI evidence |
| --- | --- |
| `goals` | `workflow submit` advertised and server writes enabled |
| `workflow_index` | `workflow list` advertised |
| `operation_lookup` | `workflow operation` advertised |
| `runtime_readiness` | `workflow readiness` advertised |
| `current_attention` | False: no canonical unresolved-attention endpoint is implemented |

Each endpoint checks its own advertised verb. A main or #52-only binary without
these verbs returns `operation_unsupported` before mutation. A CLI advertisement
does not prove daemon compatibility or provider readiness.

## Goal submission

`POST /api/v1/projects/:id/goals` accepts either:

- `{event_id,goal,preset?,auto_start?}` using a configured private preset; or
- `{event_id,request,policy,auto_start?}` when the project permits imported requests.

The same exact input and byte limits as intake apply. `auto_start` is boolean
and defaults to true. The bridge adds `schema_version:1` and passes the envelope
through `workflow submit --request-file`. Goal text only replaces the preset's
`request_text`; it does not invent TaskSpecs, policy or execution authority.

The matching daemon persists an accepted operation before returning. Its own
background owner prepares the workflow and, when requested and admissible,
starts the coordinator. Node does not invoke a separate start command or perform
background work after returning.

Success uses HTTP 202:

```json
{
  "ok": true,
  "project_id": "example",
  "event_id": "00000000-0000-4000-8000-000000000004",
  "operation": {
    "schema_version": 1,
    "event_id": "00000000-0000-4000-8000-000000000004",
    "kind": "goal",
    "state": "accepted",
    "workflow_id": null,
    "start_contract": null,
    "code": null,
    "duplicate": false,
    "updated_at_ms": 1794441600000
  }
}
```

The identifiers and timestamp above are synthetic. The operation states are
`accepted`, `preparing`, `prepared`, `running`, `blocked`, `failed` and `unknown`.
`running` means the operation handed execution to a coordinator. It does not
mean the workflow completed or was accepted. Observe the referenced workflow's
authoritative phase and verification evidence for its result.

`auto_start:false` requests preparation without starting a coordinator. It
does not convert the HTTP response into a preparation receipt: the response is
still an admitted asynchronous operation, whose later state must be read.

## Lookup and recovery

`GET /api/v1/projects/:id/operations/:eventId` invokes
`workflow operation <event_id>` and returns the same operation object, with
HTTP 200. It performs no retry, restart, reconciliation mutation or replay.

After a lost submit response, preserve the original event ID and draft and query
this endpoint. An operation receipt for another event is refused. A found
`accepted`/`preparing` receipt confirms admission, not successful execution.
A found `unknown` operation remains uncertain even though the lookup succeeded.

The backend's `workflow_operation_not_found` is exposed as HTTP 404. It is not
a `safe_to_retry` signal or proof that an earlier in-flight submit cannot later
be admitted. Preserve unresolved client state and inspect the canonical daemon.
An operator can choose an exact same-event replay where the daemon's admission
semantics allow it; the bridge never makes that decision automatically.
Changed same-event payloads remain subject to the daemon's immutable binding.

This operation contract covers `kind:"goal"` only. The existence of the lookup
verb does not imply recovery support for legacy follow-ups. Follow-up unknown
outcomes still retain their original event IDs and context.

## Project workflow index

`GET /api/v1/projects/:id/workflows?limit=20&after=<uuid>` calls
`workflow list --limit N [--after UUID]`, using the configured project root.
Limits are 1–32. The result is:

```text
{ok,project_id,workflow_index:{
  schema_version:1,
  workflows:[{
    workflow_id,workflow_key,phase,version,evidence_class,
    updated_at_ms,failure_code
  }],
  next:UUID|null,
  more:boolean
}}
```

Follow `next` while `more` is true. The bridge preserves all returned workflow
phases; it does not filter records and thereby hide pagination. A UI can show
blocked or paused workflows as workflows needing inspection, with their actual
phase and failure code. That view is not an authoritative unresolved inbox.

The index avoids guessing a workflow from a Task ID. It does not itself add a
canonical task-to-workflow mapping to task rows. A task remains separate unless
the daemon supplies an explicit relationship.

The existing workflow `/attention` route remains a page of historical worker
report claims with `semantics:"report_claims"`. An old blocked report is not
automatically erased, and a ready claim does not prove completion.

## Readiness for the UI and launcher

`GET /api/v1/projects/:id/readiness` returns an observation even when the
configured CLI or daemon is unavailable:

```text
{
  ok:true,project_id,schema_version:1,observed_at,
  cli:{available,code},
  daemon:{
    reachable,healthy,store_schema_version,queue_depth,queue_saturated,code
  },
  workflow:{
    supported,source,configured,compiler_available,start_ready,blockers
  }
}
```

Unknown booleans/counters are null. Codes are bounded stable identifiers, never
raw CLI stderr or filesystem paths. This endpoint performs read-only commands
and does not start a daemon, worker or provider call.

The daemon layer uses the existing `daemon health` command. When advertised,
the workflow layer uses `workflow readiness` and projects its actual
`configured`, `compiler_available`, `start_ready` and `blockers` values. Its
source is `daemon_readiness`. The full internal slots/configuration payload is
not returned to the browser.

With an older CLI, successful `slots list` proves only that workflow
configuration is available. The source is `legacy_slots`, configured is true,
and compiler/start readiness remain null with `runtime_readiness_unsupported`.
An unavailable/incompatible daemon is reported separately from a healthy
daemon whose compiler or start prerequisites are blocked.

Concurrent observations for the same project share only their in-flight reads.
No readiness result becomes a durable authority or a replacement for admission.
The daemon rechecks its current state when an actual goal is submitted.

## Validation scope

The additive HTTP/CLI tests cover old-version refusal, capability separation,
workflow pagination, HTTP 202 admission, optional prepare-only goals,
operation lookup after a lost receipt, explicit duplicate/conflict forwarding,
wrong-event receipts, and honest readiness failures. Tests use actual loopback
HTTP and synthetic CLI subprocesses. They do not qualify live providers or
replace daemon/store integration tests for persistence and restart recovery.
