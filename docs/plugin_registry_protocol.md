# Daemon plugin registry protocol (M4.2)

This private pre-alpha contract integrates the existing Rust supervisor into `vibemuxd`. It does not add Task/Run mutation, plugin request routing, a public SDK, or real vendor adapters. The registry and its worker snapshots are in memory only. SQLite remains owned by `WriterWorker`; the registry has no handle to it.

## Explicit startup

The default daemon starts with no plugins. To opt into trusted local executables:

```text
vibemuxd --project-root <project_path> --plugin-config <configuration_path>
```

`vibemuxctl daemon start` is unchanged and does not forward this option. There is no automatic directory scan, installation, or API-driven launch. The JSON file is bounded to 64 KiB, requires schema version 1, and rejects unknown fields. Referenced manifest/executable/cwd paths must be absolute. Manifest schema remains the existing version 1 TOML contract. Example paths are placeholders:

```json
{
  "schema_version": 1,
  "max_plugins": 16,
  "plugins": [
    {
      "manifest_path": "/absolute/path/mock_manifest.toml",
      "executable": "/absolute/path/mock_plugin",
      "working_directory": "/absolute/path/plugin_workspace",
      "restart": {
        "max_restarts": 3,
        "initial_backoff_ms": 100,
        "max_backoff_ms": 5000
      }
    }
  ]
}
```

On Windows use JSON-escaped absolute Windows paths. The executable is resolved independently of the manifest's first argv item; remaining manifest entry-point items become the argv vector. The startup loader clears inherited environment and supplies no additional variables; the supervisor adds the non-secret session ID. Manifest permission/capability requests do not authorize themselves: the startup policy grants none. This slice accepts heartbeat traffic only.

All registrations are prevalidated before startup. Invalid input returns `plugin_configuration_invalid` without reflecting file contents or paths. The Rust startup API accepts already-resolved configurations for tests/embedding; callers own their explicit environment/policy choices. It is not reachable through local IPC.

## Limits and lifecycle

| Setting | Default | Hard limit / behavior |
|---|---|---|
| Registry entries | 16 | 1..32, including quarantined/stopped entries |
| Additional restart attempts | 3 | 0..16 per identity per daemon lifetime |
| Initial backoff | 100 ms | Positive, no greater than maximum backoff |
| Maximum backoff | 5 s | 60 s |
| Startup handshake / shutdown phase | 2 s each | 2 s each in the registry |
| Heartbeat receive deadline | 15 s (three negotiated 5 s intervals) | 30 s; must exceed negotiated interval; monotonic time |
| Status frame | Existing 64 KiB ceiling | Full bounded snapshot, no pagination |

The retry delay is `min(initial_backoff * 2^restarts, max_backoff)`. `restarts` counts additional attempts actually started after the initial attempt. A limit of two permits at most three launches. Handshake success and reads never replenish the budget. Each attempt receives a new opaque session ID.

```text
starting -> active -> stopping -> backoff -> starting
                   -> stopping -> quarantined
starting -> backoff | quarantined
starting | active | backoff -> stopping/stopped on cancellation
```

Spawn failure, handshake timeout, closed transport, frame I/O failure, and heartbeat timeout are retryable only within budget. Malformed protocol, nonmonotonic heartbeat, peer protocol error, and unsupported messages quarantine immediately. Quarantine remains terminal for this registry lifetime, even when the registry shuts down. Duplicate registration cannot clear it. Recovery across daemon restarts and operator-controlled quarantine clearing are not implemented.

Heartbeat freshness is measured from the last accepted heartbeat; unrelated traffic cannot refresh it. Unsolicited Request/Response/Event envelopes quarantine with `plugin_registry_unsupported_message`. Peer ProtocolError content is replaced with `plugin_registry_peer_error`. No plugin frame becomes an internal event or Task/Run update.

## Read-only control API

The existing four-byte big-endian JSON framing, bearer authentication, request IDs, 64 KiB limit, Windows named pipes/POSIX UDS, and deadlines remain in effect. There is no TCP fallback. IPC version 2 adds this request:

```json
{"version":2,"request_id":"status_1","token":"<descriptor_token>","operation":"plugin_status"}
```

Example response shape (illustrative, not a measurement):

```json
{
  "version": 2,
  "request_id": "status_1",
  "payload": {
    "kind": "plugin_status",
    "data": [{
      "plugin_id": "mock.harness",
      "state": "quarantined",
      "restarts": 2,
      "max_restarts": 2,
      "session_id": "opaque_session_id",
      "process_id": null,
      "last_error_code": "plugin_frame_io",
      "graceful_shutdown": null
    }]
  },
  "error_code": null
}
```

`ControlClient::plugin_status()` returns the snapshot. It has no registry mutation handle. Entries are sorted by plugin identity; each entry is a coherent snapshot, while different entries can advance independently during the read. Empty registry returns `[]`. `session_id` identifies the current/most recent attempt. `process_id` is cleared only when cleanup is confirmed; an unconfirmed cleanup retains PID evidence in quarantine. `graceful_shutdown` is null until a shutdown outcome exists. `last_error_code` retains the last stable local error, including through a subsequent successful restart; it is not raw plugin diagnostics.

Reading status does not spawn, retry, probe, clear quarantine, cancel, stop, or write the database. Plugin health is separate from writer health. Status omits launch paths, argv, environment, permissions payloads, stderr content, tokens, prompts, and Task/Run IDs. There are no `plugin_start`, `plugin_stop`, `plugin_cancel`, `task_create`, or `run_update` operations.

## Cancellation and shutdown

Internal registry cancellation is idempotent and joins that lifetime. It is not exposed as a writable control operation. Cancellation during backoff bypasses the delay; cancellation during handshake waits for the owned handshake to settle within its limit, then cleans up without restarting.

Daemon shutdown signals all lifetimes before joining any. The supervisor accepts already-queued heartbeat/event traffic while awaiting the session-bound shutdown acknowledgement. It drains/shuts down or kills/reaps the exact child and joins I/O tasks. A forced shutdown reports the original error, even when cleanup succeeds. Failed reap confirmation quarantines and retains PID evidence. Other entries still receive cleanup attempts. The writer is released after registry joins, including when cleanup reports an error.

`shutdown_accepted` acknowledges an accepted request only. `DaemonControlServer::wait()` / `shutdown()` joins cleanup; standalone completion requires matching process exit and descriptor/lock removal. Dropping a server requests cancellation but is not a cleanup receipt. Abrupt daemon termination, unrelated child processes, and descendant-tree cleanup are outside the verified scope.

## IPC compatibility and rollback

Old health request:

```json
{"version":1,"request_id":"health_1","token":"<descriptor_token>","operation":"health"}
```

New servers accept v1 `health` and `shutdown` and return version 1 responses with unchanged payloads. New descriptors advertise v2. Updated clients can query/stop v1 servers, but `plugin_status` requires v2. Existing v1 clients reject a v2 descriptor; upgrade the client alongside the daemon. No descriptor is rewritten in place and no database migration runs. Existing descriptors/locks remain governed by the established recovery rules. To disable plugin loading, stop the daemon normally and restart without `--plugin-config`.

## Verified scope

Native Windows Rust 1.85 tests use the existing feature-gated Rust mock executable over real pipes, real local control IPC, and a separate daemon process. They verify crash/restart exhaustion, quarantine, sibling visibility, no Task/Run mutation, cancellation, explicit shutdown, authentication, backward request compatibility, and post-join OS process absence. M4.2 Linux/WSL, real vendor plugins, live terminal backends, sandbox enforcement, fuzz/soak gates, and production readiness remain unverified. See [PROGRESS.md](../PROGRESS.md) for exact current commands and outcomes.
