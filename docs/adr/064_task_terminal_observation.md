# ADR 064: Task terminal observation and presentation actions

- Status: Accepted for scoped pre-alpha implementation
- Date: 2026-11-02
- Related: [ADR 016](016_probe_and_unified_frontend.md), [ADR 028](028_supervisor_chat_frontend.md), [ADR 063](063_read_only_task_windows.md)

## Context

The accepted desktop design includes a Terminal tab and WezTerm pane inspection,
linking, focusing and unlinking. The legacy frontend already uses Control v4
terminal operations and an explicitly configured observation plugin. At adoption,
the native desktop command `supervisor_action` was a stub and its Task views
exposed canonical metadata and public appearance reads.

The earlier daemon observer resolved only A2A Run workspaces. Workspace chat
owners have separate durable bindings and a unique Run index, so forwarding the
old call alone would leave the desktop's own Workspace conversations unavailable.
Raw backend terminal snapshots also contain cwd and cannot be serialized directly
into the renderer. Focus and Unlink accept only a backend lease ID, which requires
native provenance before a task-bound child can use them.

## Decision

### Native action boundary

Activate the existing command with closed typed arguments:

~~~json
{"project_key":"opaque registration","task_id":"UUID","run_id":"UUID","request":{"operation":"inspect"}}
{"project_key":"opaque registration","task_id":"UUID","run_id":"UUID","request":{"operation":"inspect","previous_observation_id":"last reviewed native UUID"}}
{"project_key":"opaque registration","task_id":"UUID","run_id":"UUID","request":{"operation":"link","observation_id":"issued native UUID","pane_id":"12","instance_id":"observed instance"}}
{"project_key":"opaque registration","task_id":"UUID","run_id":"UUID","request":{"operation":"focus","observation_id":"issued native UUID"}}
{"project_key":"opaque registration","task_id":"UUID","run_id":"UUID","request":{"operation":"unlink","observation_id":"issued native UUID"}}
~~~

No renderer-selected plugin, lease ID, path, executable or command is accepted.
The existing WezTerm observer ID stays native-owned. Each action resolves the
current registered directory and canonical task/project. A Task child must also
match its actual Tauri label's task binding. Core terminal operations retain
their canonical Run, plugin-session, instance, pane and cwd checks.

Main and Task views use this same boundary. The child capability gains only
`allow-supervisor-action`; the decoder exposes only these four terminal actions.
All unrelated mutation/private-chat commands remain denied. This amends ADR 063's
two-read-command capability without granting task execution or terminal input.

Responses include canonical scope IDs and the existing path-free
`TerminalView` projection. Pane captions use the existing sanitization rule.
Private cwd, backend binding objects, titles, screen text, credentials and
ownership tokens remain native-owned. Optional terminal failure affects the
Terminal surface, not ordinary task reads or task-window creation.

### Lease provenance and lifecycle

A native cache keeps one current observation and at most one cleanup receipt per
actual caller window, bounded by main plus the 16 Task children. Each binds the window generation,
registration, canonical Task/Run, native lease identity and offered candidates
to an opaque UUID. Link, Focus and Unlink require that view's UUID; the renderer
never supplies an arbitrary Core lease. Changing a link in main cannot retarget
a child's stale controls. Superseded or foreign-view tokens fail closed.
A known reviewed lease can be unlinked even when its plugin is unavailable.
The cleanup receipt retains the exact reviewed lease while Link/Focus authority
is retired. Failed effects and failed inspection can still permit explicit
idempotent Unlink of that reviewed lease without requiring the plugin. The
receipt never substitutes a newer lease created by another view or an uncertain
replacement. Forgetting an observation only requires Find again; it never unlinks a terminal.
Window destruction and registration revocation clear matching cached data.
This cache is not another authoritative lease store and owns no execution.

One owned native operation guard bounds terminal work. Receipt locks cover only
local map operations, with no I/O under a lock. Capacity refusal occurs before
new linking effects. Link/Focus authority is retired before Core submission,
so uncertainty cannot re-arm the same attempt. Cleanup-only Unlink may be
explicitly retried for its exact remembered lease; no effect is automatically
replayed. Inspect optionally echoes the frontend's last reviewed observation
token. Only a matching actor/generation/scope token can confirm the cleanup
anchor; unknown or omitted acknowledgements neither promote an unreviewed
successor nor erase known cleanup authority. This bounds recovery even when a
successful native reply or repeated Find replies are lost. A new observation
is published only if its native window generation and canonical scope remain current. The UI
shares one pending request, retains safe errors and requires observation before
retrying an uncertain effect. Disposal prevents new requests and late rendering;
an effect already admitted to the backend may complete after its view closes.
Closing the view never cancels canonical work or a terminal.

### Workspace chat-owner authority

Add an indexed store read from RunId to `ChatExecutionOwnerCommit`, using the
existing unique Run index and complete owner loader. Unknown runs return None;
corrupt projections fail through the existing validator. The daemon calls it
through the authoritative writer queue. There is no second database reader,
JSON workspace shortcut, schema migration or new wire operation.

The observer receives the daemon's trusted project root. For Workspace owners,
share the existing physical workspace checks: Open ownership, a settled and
verified workspace receipt, managed containment, branch/Git/common-directory
inventory, the ownership receipt and matching resource identity. Provisioning,
recovery and finalized owners are unavailable. Renderer metadata is never cwd
authority. A2A behavior remains supported through its existing canonical binding.

Observation/focus does not admit execution. The execution wrapper retains its
empty-grant and HEAD-equals-base policy. Observation may inspect a dirty or
advanced owned branch while its managed identity, branch and receipt remain
valid. This separation preserves the stronger execution checks while allowing
the observation surface to follow legitimate work in an owned Run.

### Shared presentation

The shared Task detail presenter adds Terminal once for docked and independent
views. It provides Run selection, explicit Find terminals, candidate Link,
Focus/Unlink, last-observed metadata, unavailable states and stable error codes.
Terminal freshness is independent of canonical detail revision. It initializes
no pane-input path, automatic plugin/WezTerm startup or screen renderer.

## Threat model and verification

The trusted-operator boundary of ADR 028 remains. Native receipts protect the
renderer/native boundary; Core leases and the configured backend retain resource
authority. Neither Git worktrees nor observation leases are a security sandbox.

Decisive checks must cover closed arguments; foreign project/task/run and lease
injection; unavailable plugins and older Control capability; stale instance and
session; same-revision terminal refresh; duplicate and uncertain effects;
disposal; and Workspace owner physical mismatch/revocation. Fixture tests must
exercise both A2A and Workspace owner Runs and preserve canonical state. Actual
WezTerm behavior requires a separate task-owned live namespace and bounded
desktop authorization. The previous desktop-input grant is spent.

## Compatibility and limits

This increment retained 31 command names; existing Control terminal minima stay at v4.
No terminal-input method, production-profile grant, daemon startup or cleanup
command is introduced. Existing and newly activated native assets/permissions
must ship together. Full desktop parity and writable-profile qualification
remain separate acceptance work.

## Implementation and local verification

The shared Terminal tab, strict native command, bounded current/cleanup
observations and optional Inspect acknowledgement are implemented. The indexed
owner read and noncreating physical validator support Workspace owner Runs
without weakening execution admission. Independent native, UI and Core source
reviews passed.

The final local checks include 12 bridge and 25 controller regressions within
57 passing desktop library tests (two preparation cases ignored) and 203 passing
UI tests. Closed native invoke/capability and real Control-service checks,
daemon/store regressions, formatting, typechecking, production assets and
Clippy have separate evidence in the
[execution record](../evidence/native_harness_m1.md#task-terminal-observation).
An approved native Windows 11 batch subsequently rendered the docked and child
Terminal tabs, observed the unavailable-plugin result from explicit Find, kept
the child usable for another Find and metadata refresh after main close, and
verified clean final shutdown. This establishes the native unavailable-plugin
path. Real WezTerm Link/Focus/Unlink remains unverified.
