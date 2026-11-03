# ADR 063: Bound read-only task windows

- Status: Accepted for the pre-alpha native desktop
- Date: 2026-11-02
- Related: [ADR 028](028_supervisor_chat_frontend.md), [ADR 043](043_tauri_desktop_app.md)

## Context

The desktop Tasks view has bounded authenticated task, run, activity and artifact
metadata. The planned `task_window_open` command was a stub, while the capability
file already granted `task_*` windows the entire main-window command list.
Opening a separate view must preserve the selected task's identity and avoid
giving that view chat, execution, project-lifecycle or settings-write access.

## Decision

The existing command accepts exactly `{project_key, task_id}` from the main
window and returns no renderer-selected label, URL or path. Rust resolves the
registered directory, reads the task through authenticated daemon Control and
checks the task identity before reserving a view. No daemon is started and no
canonical state changes as part of this operation.

A process-local registry holds at most 16 task views, including opening views.
Each reservation binds an opaque project registration, canonical ProjectId and
TaskId to a fresh native label and generation. A second request while opening
returns a busy error; a completed view is shown and focused again. Window I/O
runs outside the registry mutex. Failed creation releases the reservation.
The native Destroyed callback releases only its matching label/generation.

The main capability matches `main` exclusively. A separate `task_window`
capability matches `task_*` and grants only `tasks_snapshot` and `settings_get`.
This initial two-command boundary is amended by
[ADR 064](064_task_terminal_observation.md): the child also receives the closed
terminal-only `supervisor_action`, with native view and Run provenance.
The read handler obtains its caller label from Tauri. An auxiliary window may
request only its registered task's Detail query with the same project key;
list queries, foreign tasks, revoked registrations and changed directory
identities fail closed. The response's canonical project/task identities and
the reservation generation are checked again before returning it.

The URL carries only bootstrap data. It conveys no authority. The frontend
selects the task entry before loading the main application, rejects malformed
bootstrap data, and shares its detail presentation with the docked Tasks view.
The task entry reads public appearance settings once and performs one bounded
detail observation at a time, with manual refresh and a five-second poll.
Disposal cancels future polling and ignores late observations. It initializes
no chat, Ask, consent, project-list or event-listener controller.

Every task window uses the same app-origin navigation guard and trusted
Rust-owned WebView profile as its launcher. Native QA windows retain their
isolated data directory and DEMO title context. Closing a task view cancels no
task, run, terminal or daemon. If another window remains, the application stays
alive; final application exit retains the existing app-owned Ask shutdown path.
Only the main application writes appearance settings.

## Security and failure behavior

A forged bootstrap URL or renderer query cannot replace native registration.
The former broad task-window grant is removed rather than combined with a narrow
grant. The task capability includes no event, filesystem, shell, opener, chat,
export, project lifecycle, execution or settings-write command. The public
settings projection contains no project paths or consent records. Failed reads
retain the last confirmed metadata with a visible error, rather than presenting
the observation as current. This view is an observation surface, not a process
or filesystem sandbox.

## Compatibility and verification

This increment retained 31 native command names. It activates a previously unsupported
pre-alpha command with a strict body; its former no-argument stub had no successful
behavior to migrate. Control v14, store schema 13 and export schema 1 are unchanged.
No new dependency, listener, production profile or database migration is added.

Decisive checks cover closed invoke arguments, actual generated-capability denial,
unbound/foreign/list reads, concurrent and duplicate reservations, generation-aware
release, capacity, bootstrap and controller disposal, and authenticated reads
across main selection changes and registration revocation without canonical
mutation. MockRuntime window creation is separate from native WebView2 lifecycle
and appearance acceptance. The paused GUI/export step is not resumed by this ADR.

The initial delivered view contains Overview, Activity and Artifact metadata.
ADR 064 adds the shared Terminal tab and its explicit observation/presentation
actions. Artifact contents and the remaining phase-1 parity items retain their
existing implementation and acceptance requirements. The native observations
below cover the initial three metadata tabs.

### Bounded native acceptance

On Windows 11 build 26300, the approved isolated fixture batch opened the
Workspace task and inspected Overview, recorded Activity and the empty Artifact
metadata state. A repeated open reused the same child window. Closing and
reopening created a new child with the same task/project/run and revision.
After the main window closed, the child continued polling and manual Refresh
returned a later observation. Closing the final child returned runner exit 0,
with daemon and monitor joined and zero unresolved Asks.

This verifies those native fixture interactions. Active Ask shutdown, other
platforms, broader appearance/IME coverage and full desktop parity remain
separate. See the [execution record](../evidence/native_harness_m1.md#native-task-window-and-workspace-json-acceptance).
