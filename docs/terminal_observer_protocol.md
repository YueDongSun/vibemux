# Native terminal observation v1

This capability observes existing WezTerm GUI panes. It never starts a harness,
spawns a pane, sends input, terminates a terminal backend/pane, or interprets terminal output.
The user watches and interacts with the native TUI in WezTerm.

## Configuration

Build `vibemux_terminal_observer` with the rest of the workspace. Use absolute
local paths in three operator-owned files; examples below are placeholders,
not machine-specific defaults. No global terminal configuration is changed.

Observer JSON (schema v1):

```json
{
  "schema_version": 1,
  "wezterm_executable": "C:\\Tools\\WezTerm\\wezterm.exe",
  "backend_executable": "C:\\Tools\\WezTerm\\wezterm-gui.exe",
  "socket_path": "C:\\Users\\Example\\.local\\share\\wezterm\\gui-sock-1234",
  "backend_process_id": 1234,
  "backend_started_at": 1800000000
}
```

`backend_started_at` is the process start time in Unix seconds. Obtain it and
the actual socket from the selected running instance. Do not reuse these
values after restart. Only canonical per-PID GUI endpoints are supported;
generic `sock` endpoints and class aliases are refused. An unavailable instance
stays unavailable; the adapter does not create it.

Plugin manifest (schema v1; substitute the absolute config path):

```toml
schema_version = 1
plugin_id = "wezterm_observer"
plugin_version = "1.0.0"
kind = "terminal"
entry_point = ["vibemux_terminal_observer", "--config", "C:\\VibeMuxConfig\\observer.json"]
capabilities = ["terminal:inventory_v1", "terminal:focus_v1"]
requested_permissions = ["terminal:observe", "terminal:focus", "process:execute"]
supported_platforms = ["windows"]
protocol_major = 1
minimum_protocol_minor = 0
maximum_protocol_minor = 0
```

Daemon startup JSON (schema v2):

```json
{
  "schema_version": 2,
  "plugins": [{
    "manifest_path": "C:\\VibeMuxConfig\\observer.toml",
    "executable": "C:\\VibeMux\\vibemux_terminal_observer.exe",
    "working_directory": "C:\\Projects\\Example",
    "granted_capabilities": ["terminal:inventory_v1", "terminal:focus_v1"],
    "granted_permissions": ["terminal:observe", "terminal:focus", "process:execute"],
    "restart": {"max_restarts": 0, "initial_backoff_ms": 100, "max_backoff_ms": 1000}
  }]
}
```

Pass it to the existing daemon entry point:
`vibemuxd --project-root C:\Projects\Example --plugin-config C:\VibeMuxConfig\plugins.json`.
Stop an existing daemon explicitly before replacing its startup configuration.
The GUI never restarts it implicitly. Existing v1 plugin startup files keep
their no-grant behavior; grants require v2.

## Opt-in cleanup subtree observation

The updated observer also implements `terminal:cleanup_inventory_v1`, the
read-only foundation in [ADR 067](adr/067_terminal_cleanup_inventory.md).
It is disabled by default. Existing JSON without the new field produces the
same two-capability Hello and remains compatible with the existing manifest.

To enable it explicitly, use the updated binary, set
`"cleanup_inventory_enabled": true` in the observer JSON, and add
`"terminal:cleanup_inventory_v1"` to both the manifest's `capabilities` and the
startup policy's `granted_capabilities`. Keep the existing `terminal:observe`
and `process:execute` grants. The flag advertises support; it does not grant
permission. Non-boolean values and unknown config fields are rejected. Enabling
it with the old manifest fails negotiation rather than falling back. No operator
configuration is rewritten automatically.

The method uses `TerminalInventoryRequest` / `TerminalInventory`. It validates
the complete configured-instance CLI inventory before filtering and returns
panes at the worktree root or real descendants. Canonical ancestor identities
cover case aliases, symlinks and junctions. Unknown/unresolvable cwd, unsupported
remote/UNC paths, invalid fields, duplicate pane IDs, malformed output or capacity
failure prevent a successful empty result. Source limits are 512 KiB and 256
panes; the existing maximum of four matching replies still applies, with errors
instead of truncation. Backend process/socket identity is checked before and
after collection, filesystem parsing uses a blocking boundary, and Core checks
the negotiated capability, permissions and active plugin session.

A per-observer worker slot stays held by the actual blocking job after its
request times out or is cancelled. New requests are refused while it remains
busy. Shutdown acknowledges only after async requests and the blocking worker
settle; a blocked drain returns an error and leaves owned-process cleanup to
the Core supervisor. An error or timeout never constitutes a clear inventory.

An empty result covers this configured supported terminal instance only. It
does not reserve a worktree or authorize deletion. Ordinary exact-directory
inventory/focus remains unchanged. The removal plan/claim/executor/recovery
lifecycle is implemented separately under
[ADR 066](adr/066_explicit_worktree_removal.md). This method itself adds no
Control operation or native Remove button; the service owns those boundaries.

## Control v4 operations

- `frontend_tasks`: argument is JSON `{ "after": null, "limit": 32 }`; returns
  `project_id`, `tasks`, `next_cursor`. Reuse the returned cursor for the next
  page; zero or excessive limits fail.
- `frontend_task`: argument is JSON `{ "task_id": "<uuid>" }`; returns an
  optional sanitized detail. No task descriptions or full event payloads.
- `terminal_inspect`: argument contains `project_id`, `task_id`, `run_id`,
  `plugin_id`; returns matching pane candidates, optional current binding and
  `checked_at_epoch_ms`.
- `terminal_link`: argument contains `query` (the inspect argument),
  `instance_id`, `pane_id`; returns a session-local `binding_id`.
- `terminal_focus` / `terminal_unlink`: argument is a JSON string containing
  that binding ID. Unknown, expired or mismatched bindings cannot be focused.

Each summary row also carries `detail_revision`, a monotonic revision covering
everything `frontend_task` renders for that task: the Task projection, every Run
projection that belongs to it, and every event carrying its `task_id`. It exists
because a Run-only transition does not move the Task projection, so
`latest_sequence` alone cannot detect it. A client that sees an unchanged
revision may skip the per-task detail read; `0` means "unknown" and forces the
read, so a client talking to a daemon that omits the field degrades to the
unconditional behavior instead of trusting it. The list and detail paths publish
the same value for the same task.

Before: v3 accepts `harness_snapshot` but rejects `frontend_tasks`.
After: v4 accepts both, with unchanged v3 harness semantics and response version
echoing. All requests still require the existing authenticated local transport.

## Plugin payloads and limits

`terminal:inventory_v1` carries Protobuf `TerminalInventoryRequest` and returns
`TerminalInventory`; `terminal:focus_v1` carries `TerminalFocusRequest` and
returns `TerminalFocusResult`. The checked-in schema defines the fields.
The daemon supplies expected CWD from authoritative Run state; clients cannot
supply a path. It resolves a Workspace chat owner through the writer's indexed
Run lookup and complete owner validator. The owner must be Open with settled,
verified workspace authority. Existing managed containment, Git inventory,
branch, ownership receipt and resource identity are checked without creating
directories. A corrupt or mismatched owner fails closed. Runs without an owner
retain the A2A binding path. Legacy Python Run IDs cannot be linked through the
Rust control API.

Physical observation permits a dirty or advanced owned worktree. Fresh execution
retains its separate empty-grant and base-HEAD checks. This owner resolution adds
no wire operation or schema migration. The native desktop wraps the four existing
terminal operations in a task-bound, path-free command with per-view provenance;
see [ADR 064](adr/064_task_terminal_observation.md) and the
[Task Terminal guide](native_harness_conversations.md#task-terminal-observation).

There are at most four matching candidates, 4096 UTF-8 bytes per CWD, 256 bytes
per workspace, 40 KiB per plugin payload, and 64 KiB per Control frame. Too many
matching panes produce an explicit capacity error. Queue capacity is eight;
request deadline is four seconds. Complete cleanup collection retains its owned
blocking worker slot through actual completion; legacy inventory/Focus retains
its deadline-covered async behavior. A timed-out Focus is not proof that every
external CLI effect ended and is never automatically retried.
Links expire after 30 minutes without inspection, and are limited to 128 per
daemon. The GUI inspects selected terminals while open and clears stale links
when disconnected. Backend failure never changes canonical task success.

Important error codes include `terminal_plugin_unavailable`,
`terminal_permission_denied`, `terminal_project_mismatch`,
`terminal_identity_mismatch`, `terminal_instance_changed`,
`terminal_link_expired`, `terminal_request_timeout`, and
`control_frame_too_large`. Cleanup observation additionally uses
`terminal_cleanup_incomplete` and `terminal_cleanup_busy`. Errors carry codes,
not terminal contents.

The explicit cleanup service selects a single configured terminal registration
advertising the cleanup method, then relies on its negotiated grants/session and
fresh complete result. Startup JSON v2 now accepts an explicit
`terminal:cleanup_inventory_v1` grant; v1, omitted grants and unknown methods
retain their refusal semantics. The original direct-registry qualification did
not cover this startup-file allowlist; a later RED/GREEN configuration regression
closed that integration gap. No operator file is upgraded automatically.
