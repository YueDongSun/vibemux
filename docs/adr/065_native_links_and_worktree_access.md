# ADR 065: Native links and checked worktree access

- Status: Accepted for scoped pre-alpha implementation
- Date: 2026-11-02
- Related: [ADR 043](043_tauri_desktop_app.md), [ADR 058](058_writable_chat_ownership.md), [ADR 063](063_read_only_task_windows.md), [ADR 064](064_task_terminal_observation.md)

## Context

The desktop design requires branch/path copying, Open worktree folder and safe
HTTPS links. The current Run view displays a sanitized branch and worktree
presence. Recorded paths are deliberately omitted from renderer snapshots, and
`open_link` is an unsupported native command. Recorded metadata alone does not
establish current physical ownership.

The daemon already has the full A2A/Workspace bindings and noncreating Git
worktree inspection. Its terminal-specific owner validator permits Open owners;
folder access must also account for finalized owners whose files are retained.
Execution admission and worktree removal have stronger, separate requirements.

## Decision

### Closed native command

Activate the existing `open_link` slot with a closed request enum:

~~~json
{"request":{"kind":"https","url":"https://example.org/reference"}}
{"request":{"kind":"worktree","project_key":"opaque registration","task_id":"UUID","run_id":"UUID","action":"open"}}
{"request":{"kind":"worktree","project_key":"opaque registration","task_id":"UUID","run_id":"UUID","action":"copy_path"}}
{"request":{"kind":"worktree","project_key":"opaque registration","task_id":"UUID","run_id":"UUID","action":"copy_branch"}}
~~~

Worktree actions accept no renderer path, executable, command or clipboard text.
Native code resolves the current registration, authenticated Task and loaded Run.
Task children must match their actual Tauri label's canonical Task binding and
generation. HTTPS requests are admitted only from main. A Task child gaining
`allow-open-link` therefore receives only its bound worktree presentation actions.
This increment retained 31 fixed command names; the later ADR 066 adds command 32.

HTTPS input is bounded UTF-8, an absolute HTTPS URL with a host, no credentials,
whitespace ambiguity or control characters. Other schemes and relative paths
are refused before any OS action. Renderer link clicks use this boundary rather
than WebView navigation. The native navigation guard and CSP remain unchanged.

Replies contain operation kind, canonical scope and an outcome acknowledgement.
They omit paths, branch contents, URLs, native handles and ownership secrets.
An open acknowledgement means the OS launcher accepted a request; it does not
prove a particular external window exists or that a remote page loaded.

### Authoritative workspace read

Add a typed, authenticated `workspace_read` operation at Control v15. Its query
contains ProjectId, TaskId, RunId and a closed `metadata` or `directory` mode.
The writer resolves either the complete Workspace owner or the canonical A2A
binding, checks all identities and refuses conflicting/corrupt authority. The
writer returns the record before physical Git/filesystem work begins.
After that work, directory mode reloads current authority, repeats its policy
checks and compares the immutable workspace binding. It holds no writer
transaction across I/O. Inspection must bind the physical Run slot to the
requested RunId, rather than accepting another valid managed Run's directory.

Metadata mode returns the complete bounded recorded branch for native copying;
it does not require directory existence or expose a path. Directory mode also
requires fresh noncreating inspection: exact managed location, ordinary ancestry,
Git worktree/common-directory inventory, branch, ownership receipt and resource
identity. Its bounded native reply includes the canonical directory and logical
resource key. That path-derived key is not an OS file-object identity. Debug
output redacts private paths, keys and branch text. No SQLite reader or state
mutation is introduced.

Workspace directory presentation admits verified, settled Open or Finalized
owners. Provisioning and recovery-required owner/session projections are refused
using the complete checked projection. Dirty files and
an advanced owned HEAD remain browseable; this grants no execution authority.
The existing terminal wrapper keeps its Open-only policy, and execution retains
its empty-grant/base-HEAD policy.

A2A directory access requires the same managed-worktree physical receipt checks.
Legacy A2A records without that managed authority remain readable as metadata,
but directory actions return an explicit unavailable result. Existing Control v4
terminal behavior is unchanged. Main-repository targets and missing namespaces
are refused; inspection never provisions or repairs a directory.

### Native effects and lifetime

One owned native job serializes link/clipboard work off the UI thread. Registry
locks cover local checks only. Caller generation, registration and scope are
rechecked before an effect and before publishing its acknowledgement. Directory
identity is rechecked/pinned locally before handoff. After the first directory
read, native retains ordinary ancestry/directory handles and makes a second
complete fresh Core directory read. Core reloads and validates authority and
physical ownership again. Native compares scope, canonical path and logical
resource key, rechecks retained OS-directory identity/reparse state, and holds
the handles through handoff. Windows handles deny delete sharing while allowing
compatible reads/writes. POSIX rechecks do not promise Windows no-delete
semantics. Remaining in-place mutation/OS-resolution races stay within the
trusted-operator limitation; this is not an OS sandbox or a Git-state freeze.

Use the cached `open` 5.4.5 crate with `shellexecute-on-windows` and without
`insecure`. On Windows, `that_detached` uses the native Shell API rather than the
legacy command-shell launcher. Other platforms retain the library's default
desktop handoff and its explicit failure limitations. Never terminate a browser,
file manager or reused external process during app cleanup.

Use text-only `arboard` 3.6.1 with default features disabled. Keep one serialized,
lazy clipboard handle for the application lifetime, and never read the user's
clipboard. Retaining the handle is required for Linux selection ownership;
clipboard availability after app exit depends on the desktop clipboard manager.
No Tauri shell, opener or clipboard plugin is registered.

Shared Task/Workspace controls use canonical IDs, one pending action and stable
errors. Fixture mode is inert. Selection change/disposal ignores late results and
prevents queued frontend invocation. Without a cancellation protocol, selection
changes cannot veto a native job already accepted during Core reads. Native
caller/registration checks still precede effects. An admitted OS handoff may finish after the view
closes. Effects are never replayed automatically. Drafts, attachments, pending
execution bindings and canonical Task/Run state are unchanged by these actions.

## Threat model and verification

The trusted-operator boundary remains. Defend the renderer/native boundary,
foreign project/task/run substitution, stale generations, corrupted ownership,
truncated metadata, path/URL injection and namespace replacement. Do not infer
folder authority from displayed branch text, `has_worktree`, terminal cwd or a
failed owner lookup. Native errors and normal logs contain stable codes, not
private paths or URL payloads.

Tests must cover closed arguments and actual capabilities; main-only HTTPS;
malformed/nil/foreign identifiers; unknown fields and invalid URLs; managed A2A
and Workspace success; finalized retained ownership; dirty/advanced branches;
forged receipts, branch/common-directory drift, reparse ancestry, missing and
main-repository targets; revocation/disposal during a read; exact native clipboard
values and no effect on refusal. Use fake effect adapters for unit/service tests.
Real browser/file-manager/clipboard behavior requires a separately approved
native batch; no current desktop-input grant remains.

## Compatibility and remaining work

Control v15 adds one read; existing operation minima and v1-v14 behavior remain.
Old descriptors cannot provide this new capability. Upgrade the bundled clients
and daemon together; test both old-version refusal and unchanged older requests.
Store schema 13, persistent history and production profiles are unchanged. Native
assets, typed arguments and capabilities must ship together. The former no-argument
`open_link` stub had no successful behavior to migrate.

This decision does not add Remove worktree. That feature needs an owner-facing
plan/confirmation/result and unknown-outcome reconciliation, fresh execution and
terminal admission, dirty-file refusal and branch retention. The existing Git
cleanup primitive remains reusable beneath that future lifecycle.

## Implementation and verification

The Core v15 read, native effects/pins, closed command, actual caller checks and
Task/Workspace controls are implemented. Native transcript Markdown now routes
explicit HTTPS activation through that command. Automated Windows tests cover
real authenticated Core transport, main/child Tauri invocation, managed
Open/Finalized workspaces and exact fake clipboard values. Independent Core,
native and UI source reviews passed. The normal browser surface preserves its
default link behavior.

A bounded Windows 11 synthetic native batch verified clipboard values, the
exact folder window, Task-child copy acknowledgements and HTTPS browser handoff.
Browser page-content inspection and the ordinary manual close sequence remain
unverified after tool availability and consent deadlines prevented completion;
the root-bound stop command verified owned shutdown. The nondefault QA seed uses
a fixed synthetic HTTPS reference and performs no automatic navigation. Dependency
feature inspection passed, but `cargo deny` and `cargo audit` are unavailable on
this host. See the [execution record](../evidence/native_harness_m1.md#native-worktree-and-https-presentation)
for commands, platform coverage and retained failures.

## Dependency sources

- [open 5.4.5 API and security boundary](https://docs.rs/open/5.4.5/open/)
- [arboard 3.6.1 clipboard API and platform lifetime](https://docs.rs/arboard/3.6.1/arboard/struct.Clipboard.html)

Both libraries are already cached locally. `open` is MIT with MSRV 1.62;
`arboard` is MIT/Apache-2.0 with MSRV 1.71. Their use avoids new project-owned
Win32 unsafe code and overlapping Tauri plugins. Lockfile and dependency-policy
checks remain required before accepting the implementation.
