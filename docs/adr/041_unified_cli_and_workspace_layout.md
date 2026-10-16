# ADR 041: Unified CLI and Workspace Layout

Status: Accepted for pre-alpha implementation. This record was ADR 024 on the
source branch `codex/harness_request_dispatch` and is renumbered here because
`main`'s ADR 024 is the stateful A2A supervisor. Integrated into `main` on
2026-10-17; see "Integration with main".

## Context

The Rust daemon, harness dispatcher, probe service, and frontend currently have
separate entrypoints. The daemon CLI library combines parsing with lifecycle
I/O, the frontend combines its model with rendering, and guides and historical
reports share one directory. This makes the development path difficult to
discover without changing any domain or protocol contract.

The existing Python CLI remains a behavior reference. When this was written,
the primary checkout was still on an older `main` and the active Rust worktree
contained the slice; that is superseded by the 2026-10-17 integration. Updating
either remote is still not implicit in local organization.

## Decision

Use `vibemuxctl` as the common Rust entrypoint for `status`, `workspace status`,
`probe`, `ui`, `daemon`, and `harness`. Keep existing standalone executables and
the Python package available. Bump the Rust development version to
`0.2.0-alpha.1`; do not change A2A, plugin, database, or request wire versions.

Split CLI command parsing, daemon lifecycle, error handling, application
orchestration, Git inventory, and text/JSON presentation into separate modules.
Expose the harness command boundary as a reusable library API, so the unified
CLI does not invoke another shell or duplicate harness execution logic.

Split frontend model, navigation state, rendering, and terminal ownership.
The renderer accepts immutable presentation data. Keyboard actions only change
the selected section or row, show help, or quit. Native TUI attachment, daemon
startup, model generation, Git mutation, and approval execution are not UI
actions in this slice.

Snapshot source belongs to presentation context, independently of optional Git
workspace data. Both standalone and unified interactive views retain
`saved_report` or `local_probe`; a missing workspace does not erase provenance.

Create a shared bounded diagnostic snapshot boundary in `vibemux_probe` for
the CLI and frontend. Loaded reports are explicitly marked `saved_report`,
checked for schema, size, duplicate agents, and invalid telemetry, and projected
to public metadata. Endpoints are omitted, arbitrary diagnostic strings are
not echoed, and only numeric version components are displayed. The original
`report_json` API is not redefined as a sanitizer.

## Output and Compatibility

- Global `--format auto|text|json` and `--json` precede the command. Auto uses
  readable text in a terminal and JSON when redirected.
- Help and version remain text. Interactive UI requires a terminal; `ui --once`
  is the explicit noninteractive path.
- Existing daemon API names and complete-command JSON payloads are preserved
  through root re-exports. Daemon usage/runtime exits remain 2/4.
- The standalone harness executable preserves its JSON and exit-1 behavior.
  Unified harness failures use the common CLI runtime exit 4.
- JSON escapes terminal controls without changing the decoded data. Text and
  TUI strings remove control and directional formatting characters.
- `status` reports whether inspection succeeded, separately from daemon health.
  Missing runtime evidence is not converted to a healthy daemon.

Workspace inventory uses local Git plumbing with bounded output and deadlines.
It disables optional index writes, pagers, filesystem monitor hooks, submodule
inspection, lazy object fetching, and ambient Git directory/index overrides.
It reads configured filter key names, never their command values, and disables
clean, process, and smudge commands plus the required flag for each driver in
the status subprocess only. Driver count and names are bounded; unsupported
names fail closed before status runs. No Git configuration file is changed.

When filters are configured, `change_detection=external_filters_disabled`
explicitly labels counts that may differ from ordinary filtered Git status.
Text and TUI display the same qualification. This is a diagnostic snapshot,
not cleanup authority or isolation from a concurrently modified Git config.

The inventory does not fetch,
merge, switch branches, read remote URLs, inspect commit messages, or clean
resources. Outside worktree paths are not printed.

The filter regression was reproduced with a synthetic, constant command that
writes a marker during normal Git hashing. Inventory must leave that marker
absent and the index bytes unchanged. See the upstream descriptions of
[filter drivers](https://git-scm.com/docs/gitattributes#_filter) and
[lazy fetch and optional lock controls](https://git-scm.com/docs/git).

## Integration with main

`main` had grown its own `vibemux_cli` since this slice branched: the
decision, DSH, workflow, dispatch, and recovery command runners, a control
parser (`parse_cli_arguments`), and `harnesses`/`switch`. The merged tree
layers the unified command tree over that client instead of replacing it.

- `daemon` and every daemon-backed verb (`harnesses`, `switch`, `dispatch`,
  `decide`, `decisions`, `dsh`, `workflow`, `slots`, `context`, `session`,
  `prompt`) still parse with `parse_cli_arguments` and keep their JSON payloads
  and their actionable `error` text in every `--format`. The new commands keep
  the branch's 32-argument and 8,192-byte-per-argument limits; the control
  verbs, which `main`'s parser left unbounded, are capped at 1,024 arguments. `daemon` prints text on a
  terminal and JSON when piped; the other verbs have no text view and print
  their JSON.
- The branch's `daemon_client.rs`, `error.rs`, and `parser.rs` were an
  extraction of lifecycle code that `main` already holds in a richer form, so
  they are not carried over; the unified modules call `main`'s library API
  through its root exports.
- `vibemuxctl harness` is the standalone native dispatch of ADR 040. It is not
  `vibemuxctl harnesses` (registry listing) or `vibemuxctl dispatch` (ADR 029,
  daemon-owned).
- The dashboard lives in `vibemux_frontend::dashboard` and builds as
  `vibemux_dashboard`. `main`'s egui `vibemux_frontend`, `vibemux_frontend_tui`,
  `vibemux_frontend_dump`, and `vibemux_dsh_tui` are unchanged, and the new
  view shares neither their themes nor their keys.
- Rust workspace version `0.2.0-alpha.1` was already `main`'s version, so there
  is no bump. Exit codes remain 2 for usage and 4 for runtime failures.

## Repository Layout

Use `docs/guides` for repeatable use, `docs/workflow` for Git/development policy,
`docs/reports` for dated evidence, and `docs/adr` for decisions. Group executable
examples by feature, beginning with `examples/harness`.

Keep the existing development worktree in place to preserve its private local
configuration and build paths. Future development worktrees use
`.vibemux/developer_worktrees/<topic_name>`, separately from runtime run
worktrees. Historical branch references remain intact. Build caches are kept
ignored, not deleted.

## Consequences

There is one discoverable command tree and consistent output selection, without
moving canonical state ownership into the UI. The CLI binary links the existing
frontend and probe libraries; no new third-party dependency is introduced.
Compatibility binaries remain during the pre-alpha migration.

Saved snapshots are not live verification. A numeric version fragment is not
proof of binary identity, authentication, inference, or protocol compatibility.
Git ancestry labels are not release approval or cleanup permission. No speed
improvement is claimed; this change concerns usability and modularity.

## Alternatives

- Replace all executable names immediately: rejected because it unnecessarily
  breaks existing scripts and the Python reference.
- Forward commands through shell strings: rejected because argv and structured
  library boundaries already exist.
- Put the whole UI and routing implementation in one binary module: rejected
  because parsing, I/O, and presentation need separate tests and ownership.
- Rename or delete all integrated branches: rejected because milestone refs
  remain useful and cleanup requires an explicit, verified resource plan.
- Fast-forward primary `main` during organization: deferred to an explicit
  integration request; it is not required for a local feature commit.

## Rollback

Use a compatibility binary or the previous commit. No database migration or
protocol change is involved. Do not delete private runtime state to roll back
the UI. Directory moves are backed by an ignored source archive; source history
and the commit diff retain the reviewed file changes.
