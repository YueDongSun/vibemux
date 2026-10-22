# Unified CLI Guide

## Build the Local Tools

```powershell
cargo build --locked --workspace --bins
.\target\debug\vibemuxctl.exe --help
.\target\debug\vibemuxctl.exe --version
```

`vibemuxctl` is the common Rust entrypoint. Building the workspace also places
the daemon and harness plugin beside it. This does not install or start them.
Python `vibemux`, `vibemux_harness_native`, `vibemux_probe`, and `vibemux_dashboard`
remain available for compatibility. `vibemux_frontend` is the separate egui
Supervisor Chat GUI and is unrelated to `vibemuxctl ui`.

## Install for Daily Use

To run `vibemuxctl` by name instead of through `.\target\debug\`, install the
release build into one directory and put that directory on your user PATH. The
daemon, launch trampoline, probe, decision peer, and terminal observer stay
beside `vibemuxctl` because the CLI and daemon find each other by location.

```powershell
.\scripts\install_local_tools.ps1 -build -dry_run
.\scripts\install_local_tools.ps1 -build -add_to_user_path
```

The default directory is `%LOCALAPPDATA%\VibeMux\bin`; `-install_directory` and
`-source_directory` override the two ends (`CARGO_TARGET_DIR` is honored). The
script never starts a daemon, writes operator configuration, or edits the
machine PATH, and it does not touch the Python `vibemux` command. Without
`-add_to_user_path` it only copies the files and prints the directory to add.
Open a new terminal afterwards, then run `vibemuxctl --version`. To update,
run the script again; stop a running daemon first (`vibemuxctl daemon stop`)
because Windows will not replace a running executable.

## Choose Text or JSON

Global output options go **before** the command:

```powershell
.\target\debug\vibemuxctl.exe --format text status
.\target\debug\vibemuxctl.exe --format json workspace status
.\target\debug\vibemuxctl.exe --json harness catalog --config examples/harness/harness_config.json
```

Auto mode selects text for a terminal and JSON for a redirected stream. Help
and version are always text. JSON data remains parseable when a field contains
terminal control characters; readable text has no ANSI styling. Long lists in
text are summaries; JSON retains the complete bounded metadata result.

Exit 0 means the command produced its result, 2 means invalid CLI syntax, and
4 means runtime or operation failure. An inspection can succeed while reporting
an unavailable daemon or a failed probe. Read the state fields rather than
treating exit 0 as model or deployment readiness.

## Inspect Workspaces and Existing Runtime

`workspace status` reports branch roles, baseline divergence, change counts,
and registered worktrees. It omits remote URLs, commit subjects, and absolute
outside-worktree locations. It uses cached refs and does not fetch or mutate
Git state.

External Git filters are disabled for the status subprocess without modifying
configuration. When any are configured, JSON reports
`change_detection=external_filters_disabled` and text/TUI show a warning:
change counts may differ from ordinary filtered Git status (for example with
Git LFS). Do not use these counts to authorize cleanup. Unusual filter names
or excessive driver counts fail closed.

Inspection needs Git 2.36 or newer and probes the installed version once per
run. Git 2.45 and newer get `--no-lazy-fetch`, so a partial clone cannot start
an implicit download. Older Git has no such option, so inspection reads the
configuration key names first and continues only when nothing can create a
promisor remote (`extensions.partialClone`, `remote.<name>.promisor`, or
`remote.<name>.partialCloneFilter`). Otherwise it fails with
`workspace_git_too_old` and an upgrade message instead of risking a network
fetch. Git older than 2.36 fails with the same code, and a version string that
cannot be read fails with `workspace_git_version_unrecognized`.

`status` adds an existing daemon health observation. It never starts, stops,
recovers, or migrates the daemon. The explicit `daemon` group retains its
existing guarded operations and JSON contracts:

```powershell
.\target\debug\vibemuxctl.exe help daemon
.\target\debug\vibemuxctl.exe daemon health --project-root .
.\target\debug\vibemuxctl.exe daemon inspect --project-root .
```

## Navigate the Dashboard

The following example is fully offline and uses a synthetic saved report:

```powershell
.\target\debug\vibemuxctl.exe ui --report crates/vibemux_probe/tests/fixtures/probe_report_v1.json
.\target\debug\vibemuxctl.exe --format text ui --once --report crates/vibemux_probe/tests/fixtures/probe_report_v1.json --width 80 --height 24
```

Use 1-7 to pick Overview, Harnesses, Workspace, Tasks, Daemon, Sessions, or
Decisions, and Tab to move to the next view. Use j/k or arrows to move
selection, Enter for harness details or to open the selected task, / to filter
the focused list, ? for help, and q/Esc to leave. Tiny terminals request
resizing; normal 80-column terminals retain the five harness slots. Quit
restores terminal mode and cursor state.

`L` cycles the single, split, and grid layouts (ADR 042). In split and grid,
Tab moves the focus between panes, 1-7 set the focused pane's view, and `z`
zooms it. Grid needs 120 x 32 and split 90 columns; smaller terminals fall back.
`r` polls the daemon now and `t` cycles the six terminal themes. The four
daemon views read the running daemon of `--project-root` every 5 seconds and
show only content-free state; with no daemon they say `daemon_not_running`.
`--once` renders the probe and workspace views only.

At 80 x 24, Harnesses places the list beside selected details. Workspace keeps
change counts, filter qualifications, and branch rows visible; the separate
worktree-path panel appears only when enough height is available. Use
`workspace status` for the full bounded worktree listing. Narrow Harnesses
views show selected details alone, with j/k still selecting the agent.

This is a read-only viewer. Key presses do not attach native agent TUIs, send
prompts, approve tools, start daemons, or change branches; the only requests
they make are daemon reads. Missing observations
are `not_run`, not successful checks. A reserved native slot is not a running
agent or verified ConPTY surface.

Without `--report`, the existing fixed read-only probe service runs once. It may
inspect local launcher/configuration and gateway metadata, but does not perform
login or model inference. For deterministic testing, always supply a saved
fixture. Saved reports are marked `saved_report`, not presented as current live
observations.

## Harness Commands

```powershell
.\target\debug\vibemuxctl.exe harness catalog --config examples/harness/harness_config.json
.\target\debug\vibemuxctl.exe harness replay --protocol codex_app_server --input crates/vibemux_harness_native/tests/fixtures/codex_app_server.jsonl
.\target\debug\vibemuxctl.exe help harness
```

The unified CLI calls the same command library as `vibemux_harness_native`. The public
example disables execution. Native probing and generation require a private
configuration; real model generation also needs separate authorization and may
consume account usage. See the [harness core guide](harness_core_guide.md).

The standalone harness binary keeps its previous JSON and exit-1 error behavior.
Use `--format json` when migrating to the common entrypoint; unified runtime
errors use exit 4 and the `error_code` field. No provider model defaults are
changed by the UI.

## Daemon-Backed Commands

The same tree carries the daemon-backed verbs: `daemon`, `harnesses`, `switch`,
`dispatch`, `decide`, `decisions`, `dsh`, `workflow`, `slots`, `context`,
`session`, `prompt`, `tasks`, `plugins`, and `terminal`. Most need a running `vibemuxd`; `daemon start`,
`daemon inspect`, and `daemon recover` do not. They keep their JSON payloads
and actionable `error` messages in every `--format`, with one exception for
readability: `daemon` prints text on a terminal. Run `vibemuxctl help dispatch`
(or the name of any other verb) for its syntax.

Three similarly named commands are different things:

- `harness` (singular) is the CLI-process native dispatch described above
  (ADR 040): no daemon, no store record.
- `harnesses` lists, and `switch` sets, the daemon's harness registry.
- `dispatch` is the daemon-owned dispatch of ADR 029, recorded as a Task/Run.

## Read Tasks, Plugins, and Terminal Panes

These verbs need a running daemon (Control v4 for `tasks` and `terminal`). They
print the daemon's own bounded, sanitized read models, the same ones the
console, GUI, web, and MCP frontends use, and they never print a prompt or
vendor output.

```powershell
.\target\debug\vibemuxctl.exe tasks list --project-root .
.\target\debug\vibemuxctl.exe tasks list --limit 5 --after <task_id> --project-root .
.\target\debug\vibemuxctl.exe tasks show <task_id> --project-root .
.\target\debug\vibemuxctl.exe plugins status --project-root .
.\target\debug\vibemuxctl.exe terminal inspect <task_id> <run_id> --project-root .
.\target\debug\vibemuxctl.exe terminal link <task_id> <run_id> --instance-id <id> --pane-id <pane_id> --project-root .
.\target\debug\vibemuxctl.exe terminal focus <binding_id> --project-root .
.\target\debug\vibemuxctl.exe terminal unlink <binding_id> --project-root .
```

- `tasks list` returns one page. `--limit` is 1 to 32 (default 20); pass the
  returned `next_cursor` to `--after` for the next page. `tasks show` returns
  the task, its runs (harness, role, status, worktree path and branch), recent
  event summaries, and artifact digests. An unknown id fails with
  `frontend_task_not_found`. Neither verb creates a task: a task comes from
  `dispatch submit` or `workflow start`.
- `plugins status` lists each registered plugin's state, restart count, and last
  error code. It cannot start, restart, or cancel a plugin. The registry is
  empty unless the operator started the daemon with `--plugin-config`, which
  `vibemuxctl daemon start` does not pass; see the
  [terminal observation protocol](../terminal_observer_protocol.md).
- `terminal` observes existing WezTerm panes through the configured
  `wezterm_observer` plugin (`--plugin-id` selects another). `inspect` lists the
  panes whose working directory matches the run's worktree and any current
  lease. `link` takes the `instance_id` and `pane_id` from that listing and
  records an in-memory observation lease; `focus` raises the linked pane; and
  `unlink` drops the lease. The lease lives in the daemon for at most 30 minutes
  and is lost on restart. Nothing attaches to a pane, sends input, launches a
  harness, or reads terminal output, and a pane is never evidence that a run
  succeeded.

## From the Python `vibemux` CLI

The Python prototype is a single-process task and run runner. The Rust daemon
owns canonical state instead, so several Python commands map onto a different
verb and two have no equivalent yet:

| Python `vibemux` | Rust `vibemuxctl` |
|---|---|
| `init` | None. The daemon creates its state on `daemon start`; operator JSON files configure dispatch, decisions, workflows, and DSH. |
| `doctor` | `status`, `probe`, and `daemon health` |
| `status` (run table) | `tasks list`, then `tasks show <task_id>` for the runs |
| `task` (create) | None on its own. A task is recorded by `dispatch submit` or `workflow start`. |
| `tasks` | `tasks list` |
| `spawn <task_id>` | `dispatch submit <harness> --prompt-file <path>`: one daemon-owned structured turn, no terminal pane. A run in its own worktree exists only inside `workflow start`. |
| `harnesses`, `switch` | The same verbs |
| `send <run_id> <text>` | None. There is no input forwarding into a pane or a continuous chat adapter. |
| `diff <run_id>` | None. `tasks show` gives the run's worktree path and branch; run `git diff` there. |
| `trace` | `tasks show <task_id>` for event summaries; `dispatch output <request_id>` for vendor records |
| `stop <run_id>` | `dispatch cancel <request_id>` or `workflow cancel <workflow_id>` |
| `theme` | None. `vibemuxctl ui` cycles themes with `t`. |

The two missing verbs, `task` creation and `send`, are deliberate gaps, not
oversights: pane input and harness launch into native terminals stay premature
until their own decision record exists.

## Privacy and Evidence

Saved reports have bounded size, schema checks, duplicate-agent rejection, and
telemetry validation. Public projections omit endpoint details, arbitrary
version suffixes, raw diagnostic text, and unrecognized application names.
This is not a general secret detector or proof that a supplied report is true.

Native JSON capture, private paths, credentials, and runtime data remain outside
tracked examples. The [branch workflow](../workflow/branch_workflow.md) explains
how code, local runtime state, and publication are kept separate.
