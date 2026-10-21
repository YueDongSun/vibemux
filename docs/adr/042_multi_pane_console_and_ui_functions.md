# ADR 042: Multi-pane console and terminal UI functions

Status: Accepted for pre-alpha implementation (2026-10-21).

Extends ADR 041's read-only dashboard and implements a subset of ADR 030
phase 2. ADR 016's native terminal boundary, ADR 028's Send rule, and ADR 032's
DSH surface rules remain in force.

## Context

`vibemux_frontend` has four surfaces. The diagnostic TUI
(`vibemux_frontend_tui`) renders one static probe screen. The inspector
(`vibemux_dashboard`, `vibemuxctl ui`) navigates a probe report and the local
Git inventory in three tabs. The DSH surface (`vibemux_dsh_tui`) owns one DSH
child process. The egui GUI reads daemon task state over Control v4.

No terminal surface shows daemon state, although the daemon already answers
content-free reads for health, plugins, tasks with their runs, events, and
artifacts, the dispatch route catalog, the DSH session status, workflow slots,
and decisions. An operator who wants several of these at once has to run
several `vibemuxctl` commands. The terminal surfaces also lack small functions
that the operator asked for: a filter, a refresh, theme cycling, re-probing,
and transcript search. ADR 030 phase 2 (shortcuts and the quick switcher) is
not implemented.

"Multi TUI" here means several read-only views in one terminal at the same
time. It does not mean attaching or embedding vendor CLI TUIs.

## Decision

### 1. Multi-pane console

The inspector becomes a console with seven views and three layouts. It is still
the `dashboard` module, the `vibemux_dashboard` binary, and `vibemuxctl ui`.

| Key | View | Source |
|---|---|---|
| `1` | Overview | probe report |
| `2` | Harnesses | probe report |
| `3` | Workspace | local Git inventory |
| `4` | Tasks: task list plus the selected task's runs, events, and artifacts | daemon, Control v4 |
| `5` | Daemon: health, plugins, and dispatch routes | daemon, Control v1, v2, and v5 |
| `6` | Sessions: DSH session status and workflow slots | daemon, Control v7 and v6 |
| `7` | Decisions: recorded decisions | daemon, Control v8 |

- **Layouts.** `L` cycles Single, Split (two panes), and Grid (two by two).
  Single is the default, so the console starts as the inspector did. A layout
  that does not fit the terminal falls back: Grid to Split, Split to Single.
  The fallback is a pure function of the terminal size, so the snapshot tests
  can pin it.
- **Focus.** In Single, `Tab` and the arrow keys cycle views, as before. In
  Split and Grid, `Tab` moves the focus between panes, `1` to `7` set the view
  of the focused pane, and `z` zooms the focused pane to the full body and
  back.
- **Selection.** `j`/`k` and the arrow keys move the selection in the focused
  pane's list. `Enter` on a task shows its detail in the Tasks view.
- **Filter.** `/` opens a filter line for the focused pane. Typing narrows the
  list rows (harnesses, tasks, decisions, branches) by a case-insensitive
  substring, `Enter` keeps the filter, and `Esc` clears it. A filter changes
  only what is drawn.
- **Refresh.** `r` asks the daemon feed for an immediate poll. The probe
  report is not re-run by the console; its source label (`saved_report` or
  `local_probe`) is unchanged.
- **Themes.** `t` cycles the six audited terminal themes of `tui::Theme`, and
  `VIBEMUX_FRONTEND_THEME` selects the starting theme. The console no longer
  hard-codes colors.
- **Help.** `?` lists every key. `q` and `Esc` quit, and `Esc` first closes an
  open filter or help.

Keys change presentation and request reads. They never start a daemon, a
harness, a dispatch, a DSH session, a workflow, or a decision.

### 2. Daemon feed

`dashboard::daemon_feed` owns one background thread with a current-thread
Tokio runtime. It polls every 5 seconds and when `r` is pressed, and publishes
an immutable `DaemonView` through a `watch` channel. Requests from the UI
(refresh, select task) go through a bounded channel. Dropping the feed stops
and joins the thread.

The connection code moves from `supervisor_client.rs` into a shared
`daemon_session` module that the GUI client and the feed both use. It re-reads
the published descriptor on every poll, verifies the runtime directory and, on
Windows, the control ACLs once per daemon generation, and drops the cached
client after any connection failure; a daemon's own error answer (for example
"not configured") or an unsupported operation keeps it. It never starts a
daemon or performs recovery. Health is read first with a 2-second deadline, so
a crashed daemon whose descriptor is left behind shows "the daemon is not
answering; check vibemuxctl daemon inspect" within one short poll instead of
waiting out the 10-second pipe retry.

Each read is independent. A failure marks only its own section, with a code
and an operator hint:

- `daemon_not_running`: "start it with `vibemuxctl daemon start`";
- `control_unsupported_version`, an operation newer than the daemon's
  protocol: "unsupported by this daemon";
- a feature the operator has not configured (`harness_dispatch_unconfigured`,
  `dsh_session_unconfigured`, `workflow_unconfigured`,
  `decision_not_configured`): "not configured";
- `dsh_session_not_found`: "no DSH session is open";
- any other failure: its code.

The feed reads only content-free data: `health`, `plugin_status`,
`frontend_tasks` (the first page, at most 32 tasks), `frontend_task` for the
task opened with `Enter` (re-read on each poll while it stays open),
`harness_dispatch_catalog`, `dsh_session_status`, `workflow_slots`, and
`decision_list`. A decision can be consumed by a dispatch after it is
recorded, so each poll re-reads from the oldest decision the feed keeps, reads
at most four pages of 50, replaces kept rows by id, and keeps the newest 64; a
longer history catches up over the next polls. It never calls
`dsh_session_poll`, `harness_dispatch_output`, `workflow_session_inspect`, or
any operation that sends a prompt or changes state. Every string passes
through `safe_text` before it reaches a renderer.

`DaemonView` and its projection from daemon types are pure
(`dashboard::daemon_model`); the renderer (`dashboard::daemon_render`) takes
only presentation data. The `--once` path does not read the daemon and renders
the probe and workspace views only.

An unconfigured dispatch service answers the catalog with
`harness_dispatch_unconfigured`; the decision list answers without a decision
config, because decisions are stored records. A feed that cannot start (for
example, an invalid project root) leaves the daemon views showing the error
code; the probe views stay usable.

### 3. Diagnostic TUI functions

- `r` re-runs the read-only probe of `vibemux_probe::run_probe` on a worker
  thread. The footer shows "probing" until the report arrives, and a second
  `r` while probing is ignored. This is the same probe the TUI already runs at
  start; it sends no prompt.
- `j`/`k` select a harness row. The footer's line names the selected
  harness's launcher, authentication, inference, version, route, code, and
  adapter support, and the keys move to the footer's title.

The golden fixtures render without a selection and stay unchanged; the
selection marker and the details line have their own snapshot tests. The key
loop now ignores key release events, which Windows terminals also report.
Quitting while a re-probe runs waits for the probe's own deadlines, so its
child processes are reaped.

### 4. DSH surface functions

- `Ctrl-F` opens a search over the bounded in-memory transcript. While search
  is open, typed characters edit the query, `Enter` and `Down` go to the next
  older match, `Up` goes to the next newer match, and `Esc` closes the search.
  `Esc` cancels a turn only when search is closed, as before.
- `Home` and `End` jump to the oldest and newest retained lines. Scrolling
  past the first page now pins the first page, so `Home` shows a full page.

Search reads the same bounded transcript the screen already shows. Nothing is
written to disk. The `[scrolled back N lines]` marker now leads the hint line,
so a narrow terminal still shows it next to the longer key hint.

### 5. GUI shortcuts and quick switcher (ADR 030 phase 2 subset)

- `gui/shortcuts.rs` holds one table that drives both the key handling and the
  `Ctrl+/` overlay: `Ctrl+K` quick switcher, `Ctrl+N` new task, `Ctrl+B`
  toggle sidebar, `Ctrl+,` settings, `Ctrl+/` overlay, plus the existing `Esc`
  and `Ctrl+1` to `Ctrl+0` rows. The new chords are consumed before text
  widgets see them and are ignored during IME composition.
- `quick_search.rs` (crate level, no egui dependency) builds the entries and
  ranks them: a label prefix ranks above a word-start match, which ranks above
  a substring match; ties keep source order; case-insensitive; at most 8
  results.
- `gui/quick_switcher.rs` renders the popup. Entries are the loaded tasks, the
  agents, and the actions New task, Settings, Diagnostics, Keyboard
  shortcuts, Toggle sidebar, Light (`claude_light`), and Dark (`claude`).
  Opening a task opens its drawer.

ADR 030's Match system, Reduce motion, spark animation, and copy buttons
remain unimplemented.

## Compatibility and rollback

- No control operation, protocol version, plugin payload, database schema, or
  persisted setting changes. A daemon older than an operation shows that
  section as unsupported.
- `vibemuxctl ui` and `vibemux_dashboard` keep their flags. The JSON
  `ui --once` output and `vibemux_dashboard --once` are unchanged; the
  `--format text ui --once` render keeps its content but shows the seven-view
  tab bar (every name down to 76 columns) and the new footer keys.
  `vibemux_dashboard` gains `--project-root <path>` (default: the current
  directory) for the daemon feed; `vibemuxctl ui` already has it.
- The diagnostic TUI's `--json` output, `vibemux_frontend_dump`, and the
  golden fixtures are unchanged.
- Rollback is reverting the change. No state needs cleanup.

## Security and privacy

- The console, like the GUI, is a client of the same-user daemon behind the
  existing descriptor and ACL checks. It adds no listener and no network
  access.
- No prompt, transcript, vendor output, environment, endpoint, or database path
  is read by the feed. Task titles are bounded summaries that the GUI already
  shows.
- Native TUI attach stays disabled, Send stays unavailable, and no screen shows
  a fabricated transcript or outcome.

## Verification

- Pure tests: layout fallback by terminal size, focus and zoom, view
  assignment, filter matching and selection bounds, the feed's section states,
  the DSH search cursor, quick-search ranking (ASCII, CJK, IDs, empty query,
  cap), and a shortcut table without duplicate chords.
- `TestBackend` snapshots of every view in every layout with loaded and
  unavailable daemon sections, and checks that no daemon string reaches the
  screen without `safe_text`.
- A feed test against a real local daemon started in a temporary project,
  where the daemon's test server can be used from the frontend crate.
- Gates: `cargo fmt --check`, workspace clippy with `-D warnings`, and the
  `vibemux_frontend` and `vibemux_cli` tests, on Windows and WSL Ubuntu.

Automated snapshots prove layout at the tested sizes. They do not prove
behavior in every terminal emulator, with every font, or with IME input.

## Alternatives

- A unified launcher that switches between the inspector, the diagnostic TUI,
  and the DSH surface: deferred. It moves code without adding views, and the
  DSH surface owns a child process whose lifetime a switcher would have to
  manage.
- Side-by-side DSH transcript and dispatch output panes: deferred. They print
  vendor content, and the daemon has no operation that lists dispatches or
  workflows, so the operator would type request ids by hand.
- New daemon list operations for dispatches and workflows: deferred to a
  separate change, because they need a control protocol version.
