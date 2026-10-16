# Unified CLI Guide

## Build the Local Tools

```powershell
cargo build --locked --workspace --bins
.\target\debug\vibemuxctl.exe --help
.\target\debug\vibemuxctl.exe --version
```

`vibemuxctl` is the common Rust entrypoint. Building the workspace also places
the daemon and harness plugin beside it. This does not install or start them.
Python `vibemux`, `vibemux_harness_native`, `vibemux_probe`, and `vibemux_frontend`
remain available for compatibility.

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
or excessive driver counts fail closed. Local inspection also requires Git's
`--no-lazy-fetch` support so partial clones cannot start an implicit download.

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

Use Tab or 1/2/3 to select Overview, Harnesses, or Workspace. Use j/k or arrows
to move selection, Enter for harness details, ? for help, and q/Esc to leave.
Tiny terminals request resizing; normal 80-column terminals retain the five
harness slots. Quit restores terminal mode and cursor state.

At 80 x 24, Harnesses places the list beside selected details. Workspace keeps
change counts, filter qualifications, and branch rows visible; the separate
worktree-path panel appears only when enough height is available. Use
`workspace status` for the full bounded worktree listing. Narrow Harnesses
views show selected details alone, with j/k still selecting the agent.

This is a snapshot viewer. Key presses do not attach native agent TUIs, send
prompts, approve tools, start daemons, or change branches. Missing observations
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

## Privacy and Evidence

Saved reports have bounded size, schema checks, duplicate-agent rejection, and
telemetry validation. Public projections omit endpoint details, arbitrary
version suffixes, raw diagnostic text, and unrecognized application names.
This is not a general secret detector or proof that a supplied report is true.

Native JSON capture, private paths, credentials, and runtime data remain outside
tracked examples. The [branch workflow](../workflow/branch_workflow.md) explains
how code, local runtime state, and publication are kept separate.
