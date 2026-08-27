# Workspace and Unified CLI Validation

Date: 2026-08-28. Platform: native Windows, x86_64 MSVC. Rust toolchain:
1.85.0. Git: 2.51.1.windows.1. Python: the verified project Python 3.12
environment. Delivery branch: `codex/harness_request_dispatch`; parent baseline:
`6498e6aa711f751d9f9e50e37034d61c17d4b341`. The delivery is the commit containing
this record, not a remote release or deployment.

## Delivered Scope

- Unified `vibemuxctl` groups daemon, harness, probe, status, workspace, and UI
  commands with consistent text/JSON output and sanitized errors.
- CLI parsing, lifecycle, application, Git I/O, and presentation are separate
  modules. Frontend model, context, navigation, rendering, and terminal
  ownership have separate boundaries. Existing daemon exports and JSON remain.
- Shared diagnostic snapshots validate and redact saved reports. Source labels
  survive both standalone and unified interactive paths, independently of Git
  workspace metadata.
- Guides, workflow policy, dated reports, and examples have dedicated folders.
  The Rust development version is `0.2.0-alpha.1`; no database, plugin, or A2A
  wire version changed. The prior harness slice is included in this delivery.

## Automated Checks

The following final-source commands exited successfully:

```powershell
cargo fmt --all -- --check
cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings
cargo test --offline --locked --workspace --all-features
cargo build --offline --locked --workspace --bins
```

The complete Rust run passed **160 tests**, zero failed and zero ignored,
across 42 targets including empty binary and documentation targets. This
includes five unified-command integration tests, eight frontend tests, and
the native harness/daemon ownership, cancellation, framing, and recovery tests.

Python checks used the project interpreter, represented below as
`$project_python` to avoid publishing a personal installation path. The current
worktree's `src` was explicitly selected through process-local `PYTHONPATH`.

```powershell
$env:PYTHONPATH = Join-Path (Get-Location).Path 'src'
& $project_python -m mypy --exclude-gitignore .
& $project_python -m ruff check .
& $project_python -m ruff format --check .
& $project_python -m pytest
& $project_python scripts/check_public_artifacts.py
& $project_python scripts/smoke_test.py
```

Results: **30 Python tests passed**, Mypy reported no issues in 21 source files,
Ruff passed, 65 files were already formatted, and the mock workflow completed.
The final candidate scan checked 171 publishable text files, found zero
unresolved findings, and retained one exact, hash-bound existing negative URL
fixture. File-target checks verified 45 local links across 45 Markdown files;
all 54 new file paths passed the no-space/no-hyphen audit.
This is not a complete secret scanner
or a scan of Git history, binaries, ignored files, or native capture.

Windows ACL/process tests used isolated, ignored `LOCALAPPDATA`, `TEMP`, and
`TMP` directories under the validation workspace. A sandbox run had correctly
rejected its runtime ACL; the complete suite was rerun with actual user access.
No security test or runtime ownership gate was weakened. An initial Python
collection imported the old checkout through the shared editable installation;
process-local `PYTHONPATH` corrected that without reinstalling the environment.

## Real Offline Terminal Exercise

Final built `vibemuxctl` and `vibemux_frontend` binaries were run in native
80 x 24 terminal sessions with the checked-in synthetic saved report. The
unified session exercised Workspace, Harnesses, row navigation, help, and quit.
The standalone session retained `saved_report` with no workspace supplied.
Both exited 0; alternate-screen exit and cursor restoration were observed.
This verifies the normal interactive path, not every terminal crash condition.

The exercise exposed clipped workspace qualifications and harness details.
The existing TestBackend test was extended to 80 x 24, failed on the old
layout, and passed after responsive layout changes. Branch rows, change counts,
filter qualification, inference state, and disabled native attachment remain
visible at that size. Worktree paths move to the CLI listing when height is
insufficient. Tiny snapshots remain bounded and request resizing.

No live probe, authentication flow, model request, native TUI attachment, or
remote executable A2A task was performed by this terminal exercise.

## Read-only Git Boundary and Review

A synthetic clean filter proved that ordinary Git status can execute a command
while rehashing a tracked file despite disabled optional index writes. The
inventory now reads only filter key names and supplies subprocess-local
overrides for clean/process/smudge commands and required flags. Driver names
and counts are bounded. A regression proves no filter marker is created and
index bytes remain unchanged; long-running process filters are also disabled.

JSON, text, and TUI expose `change_detection=external_filters_disabled` when
applicable. Such counts can differ from normal filtered Git status. They do
not authorize cleanup. Git configuration is not modified, and concurrent
configuration mutation is not claimed to be isolated. Lazy object fetching is
also disabled. The rationale and upstream references are in
[ADR 024](../adr/024_unified_cli_and_workspace_layout.md).

An independent source review closed the filter-side-effect and standalone
source-label findings. It found no additional P1/P2 issue in that bounded
review. The reviewer did not substitute for the parent-run tests.

## Workspace, Branches, and Publication

- The primary checkout remains on `main` at `99d1f8e`. Its pre-existing
  `target/` cache was preserved and added to the local repository exclude file.
- The implementation worktree stays in its existing location. All 15 historical
  local branch refs were preserved. Future worktree and topic naming is defined
  in the [branch workflow](../workflow/branch_workflow.md).
- A private, integrity-checked source archive preceded restructuring. It remains
  ignored. The temporary review worktree and its newly created review branch
  were removed only after clean-state, baseline, path, and 122-file checksum
  checks. No historical branch was deleted.
- A fresh read confirmed `origin/main` at `6498e6a`. The `linux` query failed
  with exit 128; its current SHA and synchronization state are unknown. The
  [2026-08-27 inventory](branch_inventory_20260827.md) is historical evidence.
- The local commit uses a verified public GitHub no-reply identity only for
  that commit. Global Git identity, remote configuration, runtime defaults,
  model settings, and the Python editable installation are unchanged.

No fetch, main update, merge, push, PR creation, release, or deployment is part
of this delivery. A local commit is not evidence of remote synchronization.

## Remaining Qualification

`cargo nextest`, `cargo deny`, and `cargo audit` were not installed on this
machine. Those declared additional gates are **not passed** by the checks above.
No global tool installation or gate waiver was performed. Linux execution and
remote CI remain unverified for this change.

Real model generation remains separately unauthorized and untested. The prior
Grok initialization/shutdown gap is unchanged; see the
[harness validation record](harness_validation_20260827.md). Canonical daemon
Task/Run binding, durable replay/recovery fencing, and authenticated executable
A2A tasks remain the next implementation boundary.
