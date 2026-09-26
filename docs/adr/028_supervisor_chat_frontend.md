# ADR 028: Supervisor Chat, task windows, and native terminal observation

Status: Accepted for the requested pre-alpha implementation

Supersedes ADR 027's home screen and harness-seat navigation. ADR 016's native
terminal boundary and ADR 026's shared palette contracts remain in force.

## Decision

The default GUI is one coordinator conversation with a continuous task activity list. Agents,
Settings, and Diagnostics are secondary surfaces. A task can be inspected in a
docked panel or one independent native egui viewport. View identity is TaskId;
run selection uses RunId. Closing a view never cancels a run or a terminal.
Child viewports read shared snapshots and enqueue bounded presentation or
runtime actions; only the main application persists user appearance settings.
The requested complete visual revision introduces Studio: an off-white canvas,
muted sidebar, dark-green accent, native Segoe UI/CJK typography, flat navigation,
and one grouped task list instead of repeated dashboard cards. Studio is the
default for new profiles. Existing theme preferences remain unchanged and the
five dark palettes remain available; the exported palette list adds `studio`.

The coordinator is a communication/dispatch role. The current daemon does not
provide a continuous coordinator-chat transport. The composer keeps a local
draft with Send unavailable, explains the missing capability, and never fakes a
model response. The existing structured-data supervisor recipe is not treated
as arbitrary coding-agent chat. Runtime screens contain no fixture transcripts.

Control v4 adds project-scoped task list/detail reads through the writer query
queue. Responses omit descriptions, event payloads, and workspace ownership
tokens. Task pages use a TaskId cursor. Details are bounded to the most recent
12 runs, 24 event summaries, and 24 artifact references. Truncation is visible.
Canonical state graphs and SQLite schema v3 are unchanged.

Native terminal observation uses an out-of-process Rust plugin with optional
Protobuf payloads inside the existing v1 Request/Response envelopes. Its only
methods are `terminal:inventory_v1` and `terminal:focus_v1`. Explicit startup
configuration grants capabilities and observation/focus/process permissions;
absence means denial. The daemon matches replies by request, correlation,
session and deadline. Late or unsolicited replies fail the plugin session.

The first adapter supports an explicitly configured, already-running WezTerm
GUI. It checks the executable, PID, start time and canonical `gui-sock-<pid>`
endpoint generation. Class aliases and background-mux sockets are unsupported.
Commands use argv, `--skip-config`, `--no-auto-start`, a pinned socket and an
allowlisted environment. Inventory filters to the authoritative Run workspace.
No screen text, commands, credentials, or user input enter this protocol.

Manual linking creates an ephemeral, bounded observation lease. A lease grants
only observation and focus; it does not assert that the pane is executing the
Run. Focus rechecks project, task, run, workspace, plugin session and terminal
identity. Restart/mismatch invalidates the lease. Unlink only removes the lease.
TUI contents and permissions are handled in WezTerm itself. Embedded rendering
is a future terminal-plugin capability, not a screen-scraping completion path.

## Compatibility and rollback

Control v1-v3 operations keep their original minimum versions. V4-only clients
show an unavailable capability against older daemons. The plugin envelope
remains v1.0; terminal payload additions are optional capability contracts.
Startup configuration v2 adds explicit grants; v1 remains accepted with no
grants. The observer's separate configuration is schema v1. Existing frontend
theme/window schema v1 and all five previous palette IDs remain valid.

Rollback stops the new daemon normally, removes the optional observer startup
entry, and runs the earlier GUI/daemon. There is no database migration to undo.
Observation leases and view state intentionally do not survive restart.

## Threat model and verification limits

The same-logon trusted-operator boundary is unchanged. Process and socket
generation checks prevent accidental cross-instance targeting; they are not a
security sandbox against a malicious same-user process forging endpoint files.
Requests never contain executable paths; those come only from operator startup
configuration. The plugin cannot obtain a database handle. Raw terminal titles
and terminal contents are not returned or logged. JSON responses and command
output are bounded separately from Protobuf frame limits.

Automated fixture tests prove contracts and failure behavior, not a live
WezTerm installation, IME behavior, or embedded native rendering. Platform and
live evidence are recorded separately in PROGRESS.md.
