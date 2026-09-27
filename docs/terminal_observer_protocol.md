# Native terminal observation v1

This capability observes existing WezTerm GUI panes. It never starts a harness,
spawns a pane, sends input, kills a process, or interprets terminal output.
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

Before: v3 accepts `harness_snapshot` but rejects `frontend_tasks`.
After: v4 accepts both, with unchanged v3 harness semantics and response version
echoing. All requests still require the existing authenticated local transport.

## Plugin payloads and limits

`terminal:inventory_v1` carries Protobuf `TerminalInventoryRequest` and returns
`TerminalInventory`; `terminal:focus_v1` carries `TerminalFocusRequest` and
returns `TerminalFocusResult`. The checked-in schema defines the fields.
The daemon supplies expected CWD from authoritative Run state; clients cannot
supply a path. Only A2A-backed Runs currently have this owned workspace record.
Legacy Python Run IDs cannot be linked through the Rust control API.

There are at most four matching candidates, 4096 UTF-8 bytes per CWD, 256 bytes
per workspace, 40 KiB per plugin payload, and 64 KiB per Control frame. Too many
matching panes produce an explicit capacity error. Queue capacity is eight;
request deadline is four seconds; the observer serializes CLI operations.
Links expire after 30 minutes without inspection, and are limited to 128 per
daemon. The GUI inspects selected terminals while open and clears stale links
when disconnected. Backend failure never changes canonical task success.

Important error codes include `terminal_plugin_unavailable`,
`terminal_permission_denied`, `terminal_project_mismatch`,
`terminal_identity_mismatch`, `terminal_instance_changed`,
`terminal_link_expired`, `terminal_request_timeout`, and
`control_frame_too_large`. Errors carry codes, not terminal contents.
