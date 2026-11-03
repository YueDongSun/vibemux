# ADR 062: Test-only native GUI acceptance fixture

- Status: Accepted for the test-only native Windows developer runner
- Date: 2026-11-02
- Related: [ADR 043](043_tauri_desktop_app.md), [ADR 057](057_persistent_harness_sessions.md), [ADR 058](058_writable_chat_ownership.md), [ADR 060](060_native_conversation_export.md), [ADR 061](061_native_history_search.md)

## Context

Source, renderer and authenticated daemon fixtures do not establish native Wry,
WebView2, focus, scrolling or Windows Save-dialog behavior. A native acceptance
run needs synthetic retained history and independent application settings and
WebView data without selecting the user's normal desktop state or a live vendor.

## Decision

Add the nondefault `native_qa` Cargo feature and its required-feature binary
`vibemux_desktop_qa`. The closed developer CLI accepts `prepare`, `prepare_cleanup`,
`prepare_surfaces`, `check`, `run` or `stop`, plus only `--root <absolute local directory>`. It has no
renderer entry point, PID-target control, provider override or input-file option.
The feature includes bundled custom-protocol assets and daemon test helpers.
Normal launches keep their existing settings resolver and window behavior.

A private Rust launch mode reuses the same Tauri builder, bundled frontend,
32 fixed commands, capability list, CSP and navigation guard. Its main-window
title is `VibeMux · DEMO · isolated native QA`. Settings live in the owned
`profile/desktop_settings.json`; WebView2 uses the owned `webview` directory.
The root also reserves `exports` for explicit acceptance output.

`prepare` refuses an existing root. It creates a synthetic Git project, fixed
routes with no provider/model/environment selection, exact copies of the built
chat fixture under Codex/Claude aliases, and three conversations through the
real daemon control API. The baseline is four Saved records, two archived Saved
records and two Workspace records, with the lifetime Workspace Run retained
and no process claim. The parent owns daemon startup and explicit shutdown/join.
Only stopped-daemon setup writes detection through the authoritative writer.

`check` reopens the exact root/plan, starts its owned daemon, checks fresh
production AppState resolution and authenticated history twice, compares the
baseline and descriptor stability, then joins the daemon. It launches no GUI.
A clean daemon may remove its runtime leaf; a later owned observation recreates
that leaf through the normal DaemonPaths contract.

### Cleanup-review scenario

`prepare_cleanup` creates the same 4/2/2 history, closes the synthetic execution,
then sends one file-retaining owner Stop through Control and observes that exact
chat/Task/Run as Finalized. It seeds no cleanup Plan, Confirm or Arm. The ordinary
`prepare` scenario continues to require Open and needs no terminal mock binary.

The cleanup scenario copies the built first-party `vibemux_registry_mock` and
selects only its fixed `terminal_cleanup_normal` mode. Its exact manifest matches
the mock Hello, while startup schema 2 grants only cleanup inventory plus
observe/process permissions, with one registration and zero restarts. Config,
manifest, source and copy retain immutable pins; reopening also compares the
complete source/copy bytes. Prepare/check/run pass this explicit configuration
through `start_for_paths_with_plugins`. They wait at most 15 seconds for the
registered peer to become Active. Failed readiness drains and joins the created
daemon while its layout pins remain owned.

The empty inventory describes this synthetic peer only. It does not observe
WezTerm or prove that host terminals are clear. Actual GUI actions remain a
separately approved native acceptance sequence. No renderer can select this
scenario or replace its plugin, mode, permissions or input files.

Private plan schema 2 requires an explicit `history`, `cleanup_review` or
`surfaces` value.
Legacy plan schema 1 without that field is accepted only as History. Missing
schema-2 scenarios, unknown versions/values and schema-1 cleanup selection are
refused before layout/plugin I/O. Root, run, stop and report records keep schema
1. Older binaries refuse new plans; use matching developer binaries. Stop stays
independent of plan/layout parsing so corrupt scenario files cannot prevent an
owned exit request. This changes no production Control/store/native-command
version or installed-provider qualification.

### Tool-card, palette and Console scenario

`prepare_surfaces` selects exact fixed `native_surfaces` fixture bytes. Two
Saved Codex turns and one Workspace turn each retain one command and one
file-change Tool body beside User/Assistant records: the baseline is 8 Saved,
2 archived Claude and 4 Workspace records. The owner stays Open and its session
Closed, with no process claim, empty grants and a settled verified worktree.
This scenario starts no terminal plugin and seeds no cleanup action.

Prepare, check and run validate the private bodies through authenticated Control
using the existing GUI-neutral conversation collector and final snapshot
verification. Counts alone are insufficient. Each canonical turn/attempt must
have exactly one User, Assistant, Command and File record, matching fixed
reported fields and a complete 300-line diff with the unique late Find marker.
Missing, malformed, partial, truncated, duplicated or wrongly grouped bodies
refuse. Reported tool IDs may repeat across separate canonical groups.

All NativeQa launches now use the same Rust-only AppState factory as non-GUI QA
observations. Its diagnostic runner is an eight-second in-process, cancellable
fixture with all eleven launchers, authentication, inference, gateway and A2A
checks NotRun. It performs no host discovery, process launch, network operation
or environment read. This permits explicit Run local checks/Cancel/status UI
acceptance without installed tools. A completed fixture job is not an observed
environment check. Standard startup keeps its existing shared runner.

The mock/real choice is not serialized or renderer-selectable. Capabilities,
native command count 32, Control v16 and store schema 14 remain unchanged.
Legacy schema-1 plans remain History-only; older binaries refuse the new CLI
action or unknown scenario. Root/run/stop/report schema 1 and plan-independent
nonce Stop remain unchanged. Fresh namespaces require matching built images;
the scenario prepares native acceptance and does not itself complete it.

The first approved Surfaces batch observed palette navigation, command/file
cards, Saved late-diff Find, and mock Console completion/cancellation/status
with retained prior results. Normal close joined daemon and monitor with zero
unresolved Asks. It also exposed disabled Workspace Find and a cramped Saved
viewport. Their subsequent UI repair has actual-component browser evidence;
the repaired images need their own native retest. The original run and its
observed gaps remain in the
[Surfaces observation](../evidence/native_harness_m1.md#native-surfaces-interaction-and-observed-gaps).

## Namespace ownership and fixture provenance

This is an exclusive, trusted developer QA namespace. External mutation during
an operation is unsupported. Root/ancestor handles and a versioned marker bind
the fresh namespace. Fixed child directories retain no-delete handles and file
identities while allowing the production settings atomic-save sequence and
normal WebView2 writes. Each use and the running monitor check observed child
identity and reparse state. Detection requests exit through the owned app handle;
unknown state is retained and reported as a failure.

These checks are not a security sandbox and do not exclude every possible
in-place reparse mutation by an equally privileged concurrent process. A path
consumer can race the checks. Protecting against such a writer requires a
separate isolation design; neither Git worktrees nor this QA helper provide it.

Before daemon consumers start, read-only/no-write/no-delete file handles retain
the built fixture and launch trampoline, both copied executables, fixed fixture
configuration, dispatch configuration and fixed project files. The existing
fixture guard compares source/copy bytes while these handles are held. The
handles and their checked directory identities remain alive through GUI,
monitor and daemon shutdown. Mutable settings, database, Git and WebView files
are not pinned as immutable inputs. No installed vendor or model call is needed.

Each later check/run compares the retained copies with the current built fixture.
Rebuilding that image can invalidate an already prepared namespace even when its
size and intended behavior stay the same. A mismatch is refused; retain the old
namespace and prepare a fresh one after completing the builds. Do not replace
its vendor copies to manufacture a passing check.

## Lifecycle and reports

`run` is once per prepared namespace and validates the baseline before GUI
construction. One owned monitor checks the root/layout and a root/run-bound
stop request every 100 ms. Its 20-minute deadline starts after application
construction; it does not bound a hung native startup. After the event loop,
The actual managed AppState is captured during setup before window creation.
Ask and local-probe shutdown are observed explicitly even without Ready or on an
abnormal return, the monitor is cancelled/joined, and the daemon is shut
down/joined. Any unresolved probe takes cleanup-failure precedence over panic,
missing Ready/owner, ownership and monitor errors. Ask counts remain separate;
`cleanup_unconfirmed` persists and later Stop refuses it. Cleanup failures retain
their namespace.

`stop` writes an idempotent, versioned root/run nonce request. It never kills a
PID from disk. Persisted outcomes must have the current schema, matching IDs,
fixture-only true, production-profile-qualified false and a terminal phase.
Unconfirmed cleanup cannot become an AlreadyStopped success. A NotRunning
response means no GUI run record exists; it is not a general process inventory.

Reports always keep production qualification false. `gui_ready_observed`
records only Tauri RunEvent::Ready, not DOM/JavaScript readiness or successful
scenario interactions. An injected startup-failure test exercises cleanup
without opening a window. CLI/test success is separate from live GUI acceptance.

## Compatibility and acceptance

Current Control remains v16, store schema 14, export schema 1 and native commands 32.
No new dependency, production profile, renderer privilege or database schema
is introduced. Windows activation manifests have one linker-owned producer for
all desktop executable targets, including library tests; Tauri still supplies
icons and its separate command ACL manifest. Common Controls v6 is retained.

The source gate, focused native tests, normal/QA builds and exact prepared root
must be verified before desktop interaction. Native mouse/keyboard control needs
its own bounded user approval. Initial acceptance covers synthetic history
search/deep navigation and export cancel/save, then normal window close and
owned shutdown. No Send, Start, Resume or provider setup is part of that sequence.

Executed evidence and remaining platform/UI limits belong in
[the native harness record](../evidence/native_harness_m1.md#native-gui-qa-preparation).

The first approved native run observed Search, Saved record navigation and export
cancellation, then exposed header clipping and reached its input-time limit.
Its owner stopped normally through the nonce protocol and joined the monitor
and daemon with zero unresolved Asks. A successful lifecycle report does not
complete the remaining archived/Workspace navigation or JSON-save scenarios.
Repairs and any renewed native check are recorded separately in
[the GUI observation record](../evidence/native_harness_m1.md#native-gui-search-and-export-observation-and-scroll-repair).


The renewed batch verified Saved, archived and Workspace navigation with the
repaired header and icon, then stopped with daemon and monitor joined. JSON
selection reached the Save dialog but the file save remained incomplete.
A final export attempt was interrupted by the user's physical Escape; it
retained `cleanup_unconfirmed` with `monitor_joined=false`, despite daemon
join and zero unresolved Asks. That failed cleanup acceptance remains visible
in the GUI observation record and must be reconciled before a retry.

### Atomic stop publication

The stop publisher now writes and syncs a bounded owned staging file, closes its
exclusive writer, then creates the final name through an atomic no-replace hard
link. A matching competing publisher preserves the winner and cleans or reports
its own unpublished stage. This requires local filesystem hard-link support;
an unsupported operation fails without a replacement fallback. A stage whose
deletion is denied remains an explicitly reported owned recovery link.

Root and ancestor directory pins permit read/write sharing while still opening
with read access and denying delete sharing. Metadata, reparse and identity
checks remain enforced. JSON readers and immutable fixture file pins retain
read-only sharing. Windows tests confirm publication succeeds while namespace
rename remains denied, and rename becomes possible after owned pins are dropped.

The historical `monitor_joined=false` does not prove that a thread failed to
join: the field also becomes false when a joined monitor returns an ownership
error. The native runner now logs the inner stable monitor error code before
returning the existing cleanup status. The old outcome remains unchanged; its
exact failure branch is unknown. The non-GUI reproduction and repair checks are
in [the stop-publication record](../evidence/native_harness_m1.md#qa-stop-publication-and-monitor-diagnostics).

### Task-window and JSON acceptance after the repair

The fresh task-window namespace completed its bounded native sequence and
normal final-window shutdown. The main window closed while its Task child
remained usable; the last child then closed with runner exit 0, Ready and
event-loop return observed, daemon/monitor joined and zero unresolved Asks.
Its Workspace JSON export contains two complete records and matches the plan.
This batch used normal window close; atomic nonce-stop publication retains its
separate non-GUI filesystem/concurrency evidence. The historical failed run
remains unchanged. See the [native acceptance record](../evidence/native_harness_m1.md#native-task-window-and-workspace-json-acceptance).
