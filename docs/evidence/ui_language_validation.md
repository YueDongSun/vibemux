# GUI language, rounded controls, and adaptive typography

Local handoff for ADR 035, 2026-10-13. This extends the earlier navigation
slice; its historical evidence is retained separately.

The 2026-10-14 font-density revision follows the user's request for smaller
text. Earlier source hashes and images identify their historical working
state. Current sizes are 24 px greeting, 18 px page heading, 15 px task title,
13 px body, and 12 px controls; compact bodies use 12 px and headings 16 px.
The revision owns typography defaults and their GUI consumers, removes the
duplicate 15 px body override, and preserves language and execution behavior.
The current revision is bound by the
[compact-font source manifest](ui_compact_font_source_manifest.json), with
[English](../frontend_previews/ui_compact_font_english.png) and
[Chinese](../frontend_previews/ui_compact_font_chinese.png) native demo previews.

## Scope

- Task: English-default GUI, optional Chinese interface, Claude-inspired
  rounded controls, and typography that fits each UI region.
- Owned modules: `vibemux_frontend` localization, GUI presentation, local
  settings, synthetic preview runner, tests, and related documentation.
- Baseline: `9e738f12c6dae4d97a5a567a0d261427c4c6fb86`, with the previously
  implemented local navigation changes on `codex/ui_navigation`.

## Implemented

- English default and saved `zh_cn` option in the header and Settings.
- Offline text catalog for interface labels, tooltips, help, and empty states;
  localized search actions remain searchable using their English names.
- English-only demo titles and sample paths. User content retains its language.
- Rounded action pills, accent-tinted New task, 12 px button corners, 16 px
  cards/popups, and a 24 px composer.
- Region-aware display text, sans-serif task updates, consistent Chinese
  heading fonts, and bounded scrolling shortcut help.

## Behavior and invariants

- Switching languages preserves the draft, task selection, and task windows.
- Unknown language values fall back to English while keeping other preferences.
- Template substitutions are single-pass and preserve literal inserted content.
- No translation service, execution control, daemon mutation, or new dependency.
- Disabled Send and existing terminal ownership checks are unchanged.

## Files changed

- New `src/localization.rs`; GUI modules, `src/quick_search.rs`,
  `src/theme/serialize.rs`, `src/preview.rs`, and the screenshot runner.
- ADR 035, ADR 030 cross-reference, README, CHANGELOG, PROGRESS, this receipt,
  and selected synthetic preview images.
- The prior navigation manifest identifies its historical source state and
  is not presented as a fingerprint of the current source.

## Validation executed

Native Windows 10.0.26300.0, Rust 1.95.0/MSVC with locked cached dependencies.
Commands use the process-local `RUSTUP_TOOLCHAIN=1.95-x86_64-pc-windows-msvc`.
Repository toolchain and dependency files remain unchanged.

- `cargo test -p vibemux_frontend --all-features --locked --offline -j 2`:
  the final production typography run passed 142 tests with one intentionally
  ignored fixture generator (125 unit, 5 CLI, 2 palette, 10 UI-state tests).
- `cargo test -p vibemux_frontend --lib --all-features --locked --offline -j 2`:
  after adding two geometry regressions, **127 passed, 0 failed, 1 ignored**.
  The first narrow-region test incorrectly tried to shrink the root panel;
  its fixture was corrected to use an actual 320 px child region. The expected
  compact heading size and sans-family assertions were preserved.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- 2026-10-14 smaller-font revision:
  `cargo test -p vibemux_frontend --all-features --locked --offline -j 2`:
  **144 passed, 0 failed, 1 ignored** (existing fixture generator).
  Four new native light-theme captures cover English/Chinese conversation at
  1280x800 and drawers at 960x600; English full-size and Chinese compact
  frames were visually inspected. Formatting and diff checks passed. Scoped
  strict Clippy still reports the two unchanged diagnostics listed below.
- `cargo clippy -p vibemux_frontend --all-targets --all-features --no-deps
  --locked --offline -j 2 -- -D warnings`: blocked by the same unchanged
  `dsh_tui/mod.rs:251` collapsible-match and `theme/mod.rs:81` derivable-default
  diagnostics. Neither was suppressed or altered for this task.
- Native captures exposed overflow in shortcut help after increasing global
  control height. The global override was removed and the list was bounded.
  The palette's reserved height was also increased to retain its footer.

## Platform coverage

- Windows: native automated tests, simulated input, and synthetic GUI captures.
- Six final native captures: English conversation and compact drawer; Chinese
  conversation, compact dark drawer, compact Settings, and compact dark
  shortcut help. Representative frames were visually inspected after the
  typography correction. The earlier six captures remain historical logs.
- Retained previews: [English](../frontend_previews/ui_language_english.png),
  [Chinese compact drawer](../frontend_previews/ui_language_chinese.png), and
  [Chinese shortcut help](../frontend_previews/ui_language_shortcuts.png).
  All are synthetic DEMO scenes. [Source and image hashes](ui_language_source_manifest.json)
  bind the result to the current working files and screenshot binary.
- Linux/WSL/macOS: unverified. No live terminal backend or provider turn.
- Physical IME candidate windows, screen readers, OS clipboard delivery, and
  physical monitor transitions remain manual validation limits.

## Compatibility / migration

- Optional `language` field in frontend JSON schema 1: `en` or `zh_cn`.
  Missing/unknown values select English. Old binaries ignore the added field.
- No database, IPC, plugin, shared palette, or task-content migration.
- TUI/Python remain their existing English diagnostic interfaces.

## Security impact

- UI-only localization and presentation; no network translation or new access.
- Technical IDs, paths, events, and user-authored text remain data.
- Clipboard buttons retain explicit-click behavior and exact field contents.

## Remaining work

- Changes are local and uncommitted; no push, deployment, or app installation.
- Full-workspace release gates and hosted CI were not run for this UI slice.
- Known baseline Clippy findings prevent a clean strict-lint release claim.

## Known risks

- Fonts depend on installed system faces; other systems may render differently.
- Raw daemon diagnostics and task content are preserved rather than translated.
- The coordinator-chat capability remains unavailable, as shown in both languages.
