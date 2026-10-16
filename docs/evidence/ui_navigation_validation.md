# Native GUI navigation validation

This historical receipt describes the navigation slice before the subsequent
language and rounded-control enhancement in ADR 035. Its source hashes and
previews identify that earlier working state, not the current source tree.

Validation completed on 2026-10-13. This is a local implementation handoff,
not a release or hosted-CI receipt.

## Scope

- Task: update from GitHub, then improve the native GUI with useful navigation.
- Baseline: `main` fast-forwarded from `2445a0153b665b60f402cb5606106022a43d6775`
  to `9e738f12c6dae4d97a5a567a0d261427c4c6fb86`; local implementation branch
  `codex/ui_navigation` starts at the latter commit.
- Owned modules: frontend local search, GUI navigation, clipboard presentation,
  preview scenes, and their documentation. No daemon, database, protocol, DSH
  execution, terminal ownership, or permission changes.
- Existing A2A documentation and its PROGRESS entry were preserved. The
  pre-existing toolchain component edit was already present upstream.

## Implemented

- `Ctrl+K` and a sidebar Search entry open a palette over loaded task titles,
  IDs, harness names, agents, and local actions. Results use stable,
  case-insensitive Unicode ranking and are capped at eight; queries are
  capped at 256 characters. A partial task list is identified in the footer.
- Arrow selection, Tab focus, Enter, Escape, focus restoration, and an empty
  result state. The result area scrolls within the minimum supported window.
- A shared shortcut table and `Ctrl+/` help, with actual agent names;
  `Ctrl+N`, `Ctrl+B`, and `Ctrl+,` work from the composer. Native Windows
  Control/command modifier flags and multi-frame IME composition are covered.
- Explicit Copy buttons for task titles, task/run IDs, branches, and worktree
  paths, with 1.5 seconds of local feedback.
- Search, diagnostics, and a current-page marker in the collapsed rail;
  Recent highlighting clears when its drawer closes. Narrow drawers reserve
  more conversation width, and the activity heading wraps.

## Behavior and invariants

- Search and navigation are local. Opening a task uses the existing bounded
  `LoadTask` read request. No new execution action is introduced.
- Drafts survive navigation. Starting another draft still asks before
  discarding text. Coordinator Send remains unavailable.
- Clipboard output requires a click and contains exactly the selected field.
  Search queries and copy feedback are not persisted or logged.
- The palette retains keyboard ownership after egui focus requests. Temporary
  test tracing was removed after diagnosing a focus-filter reset that could
  let native directional focus replace the selected result.

## Files changed

- New: `src/quick_search.rs`, `src/gui/quick_switcher.rs`, and
  `src/gui/shortcuts.rs` under `crates/vibemux_frontend`.
- Updated: frontend module declarations, app, sidebar, conversation,
  design primitives, task details, and the preview runner. Regression tests
  live beside these modules.
- Documentation: README, CHANGELOG, ADR 030, PROGRESS, this handoff, the
  [source manifest](ui_navigation_source_manifest.json), and three retained
  native previews. The manifest records SHA-256 hashes for all 11 changed
  Rust files, the screenshot runner, and the retained PNG files.

## Validation executed

Environment: native Windows `10.0.26300.0`, MSVC target, Rust/Cargo 1.95.0,
locked dependencies, egui/eframe 0.32.3. Commands used the process-local
`RUSTUP_TOOLCHAIN=1.95-x86_64-pc-windows-msvc`; repository toolchain settings
were not changed by the UI work.

| Command | Result |
|---|---|
| `cargo test -p vibemux_frontend --all-features --locked --offline -j 2` | **135 passed, 0 failed, 1 ignored**; the ignored test is the existing golden-fixture generator. Includes 15 new regression tests. |
| `cargo fmt --all -- --check` | Passed. |
| `git diff --check` | Passed. |
| `cargo clippy --workspace --all-targets --all-features --locked --offline -j 2 -- -D warnings` | Blocked by the unchanged `vibemux_platform/src/windows_acl_native.rs:140` `manual_div_ceil` diagnostic. |
| `cargo clippy -p vibemux_frontend --all-targets --all-features --no-deps --locked --offline -j 2 -- -D warnings` | Blocked by unchanged `src/dsh_tui/mod.rs:251` (`collapsible_match`) and `src/theme/mod.rs:81` (`derivable_impls`). No lint suppression was added. |
| Read-only Git release inspector | Diff checks passed; no potentially sensitive tracked paths reported. Existing user changes remain excluded from this task's ownership. |

The test suite includes real daemon read/disconnect integration, CLI checks,
palette parity, existing seven-theme headless rendering checks, Unicode
search/copy, draft preservation, disabled sending, native modifier flags,
IME handling, keyboard focus, and minimum-window palette bounds.

An initial sandbox build failed when Windows rejected the vendored Protobuf
compiler with DLL relocation error 623. The same compiler ran successfully
outside that sandbox through the supported approval path; subsequent tests
completed there. Failed layout/focus regression attempts remain in ignored
local validation logs. No failed run is counted as passing evidence.

## Platform coverage

- Windows: native test suite and screenshot renderer, as specified above.
- Screenshot matrix: **18 successful captures**: `switcher`, `drawer`, and
  `shortcuts`, each in `claude_light` and `claude`, at 1280x800 and 960x600
  with 1.0 rendering scale, plus 1280x800 with 1.5 rendering scale.
  Representative light/dark, compact, and scaled frames were visually inspected.
- [Search palette](../frontend_previews/ui_navigation_switcher.png),
  [compact task drawer](../frontend_previews/ui_navigation_drawer.png), and
  [keyboard help](../frontend_previews/ui_navigation_shortcuts.png) are retained.
  These are visibly labeled synthetic DEMO scenes, not live task evidence.
- Linux/WSL/macOS: not executed. Real IME candidate windows, screen readers,
  physical monitor DPI transitions, and OS clipboard delivery were not
  manually exercised; clipboard command output was tested headlessly.
- Live terminal backend: no WezTerm/tmux or vendor execution was started.

## Compatibility / migration

- No configuration schema, palette export, persistence, IPC, or plugin change.
- Search and shortcut help are additive UI controls. Existing saved themes
  and the ten established agent shortcuts remain supported.
- ADR 030 phase 2 is partially delivered: navigation and copying are present;
  Match system, Reduce motion, and spark animation remain planned.

## Security impact

- No new network call, process launch path, database access, or permission grant.
- Clipboard writes occur only on explicit field clicks. Preview processes
  use synthetic fixtures and exit after capture.
- Source and documentation were reviewed for credential values, machine-local
  paths, unrelated edits, and generated noise. Build logs and the full capture
  matrix remain in ignored `target/`; only selected demo previews are retained.

## Remaining work

- Resolve the pre-existing strict Clippy diagnostics separately before claiming
  a clean release gate. This change does not claim full workspace validation.
- Full workspace tests, nextest, deny, audit, Python checks, and hosted CI were
  not run for this GUI-only change. Rust 1.85/MSRV compatibility is unverified.
- Changes are local and uncommitted. No push, PR, deployment, or installed-app
  replacement was performed.

## Known risks

- Search covers loaded tasks only, not the entire persisted task history.
- Renderer screenshots and simulated input tests do not establish physical
  keyboard, IME, accessibility, or multi-monitor behavior on every device.
- The existing coordinator-chat capability limitation remains visible in the UI.
