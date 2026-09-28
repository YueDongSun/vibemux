# ADR 030: Claude-style GUI with a light and dark Claude theme pair

Status: Proposed. This ADR is the approved design specification; none of it is
implemented yet. It becomes Accepted when phase 1 merges, and PROGRESS.md
records the status of each phase.

Amends ADR 028: the default theme for new profiles becomes `claude_light`
instead of `studio`, and the task drawer, composer, and sidebar change
presentation as described below. ADR 028's coordinator-chat limits, Control v4
reads, and task window model are unchanged. ADR 026's shared palette contract
and ADR 016's native terminal boundary remain in force.

## Context

The Supervisor Chat GUI (ADR 028) opens in the `studio` theme. The existing
`claude` theme is a near-black palette (`#0F0E0C`) with a terracotta accent and
the same layout. A review of the preview screenshots found these defects in
every theme:

- The task drawer is a floating window with two headers ("Task details" with a
  close icon, then "Task details" with a Close button), and it covers the top
  bar's Refresh button.
- The bordered task list is cut off sharply at the edge of its scroll region,
  above the composer, with no cue that more tasks follow.
- The run details in the drawer are not aligned in columns.
- The divider under the top bar is heavy.

The user asked for a more Claude-style design with more Claude Desktop
elements and functions. The GUI has no serif type, uses bordered boxes for
cards and the composer, and shows a letter tile as the assistant avatar.
Four legacy shell files from ADR 027 (`gui/app.rs`, `gui/overview.rs`,
`gui/workbench.rs`, `gui/stub_bank.rs`) are no longer declared as modules and
are not compiled.

## Goals and non-goals

Goals:

- A light and dark Claude theme pair with serif display type, softer surfaces,
  and Claude Desktop shell elements.
- Every new control performs a real local function.
- Fix the drawer, clipping, and alignment defects in every theme.
- WCAG AA contrast: at least 4.5:1 for text in the two Claude palettes and for
  accent-colored text in every palette, and 3:1 for icons and control
  boundaries. Other text colors in the five older palettes are unchanged.

Non-goals:

- Sending coordinator messages. Send stays unavailable, as in ADR 028.
- Attachments, model selection, or any control without a local function.
- Changes to the TUI themes, the Python CLI's default theme, the daemon, or
  any wire protocol.

## Decision

### 1. Palettes

`ThemeId` gains `ClaudeLight` (token `claude_light`), appended after `studio`
so existing indexes and the exported array order do not shift. The cycle order
becomes `claude -> github -> vscode -> nord -> gruvbox -> studio ->
claude_light -> claude`. The existing `claude` palette is retuned to a warm
charcoal. The accent and `accent_alt` of `claude` are unchanged.

| Field | `claude_light` (new) | `claude` (retuned) |
|---|---|---|
| `bg` (canvas) | `#FAF9F5` | `#262624` |
| `surface` (sidebar) | `#F5F4ED` | `#1F1E1D` |
| `surface_alt` (cards, composer) | `#FFFFFF` | `#30302E` |
| `border` (hairline) | `#E6E3DA` | `#3A3935` |
| `text_primary` | `#141413` | `#F5F4EE` |
| `text_muted` | `#6B6960` | `#A8A59B` |
| `accent` | `#C96442` | `#D97757` |
| `accent_alt` | `#F1D8CC` | `#E7C6B4` |
| `success` | `#3F7A4A` | `#8AB487` |
| `warning` | `#8F6414` | `#D9A85F` |
| `danger` | `#B03A2E` | `#E07A6F` |
| `terminal_bg` / `terminal_fg` / `terminal_cursor` | `#1F1E1D` / `#F5F4EE` / `#D97757` | `#1A1918` / `#EDEBE4` / `#D97757` |
| `font_family`, `font_size`, `corner_radius`, `border_width`, `density` | `Cascadia Code`, 13, 8, 1, comfortable | unchanged |

The light theme keeps a dark terminal, following Studio's precedent, because
the ANSI colors derived for the terminal generators are tuned for dark
backgrounds.

`config/theme_palettes.json` is regenerated in the same change and stays
pinned by `palette_parity`. The Python fallback copy of the `claude` palette in
`src/vibemux/theme.py` is updated to the retuned values. The Python CLI's
default theme stays `claude`.

This ADR amends ADR 026's palette invariant "surface lighter than background":
the surface must instead differ from the background by at least 5.0 luma units
(Rec. 709 weights on 0 to 255 channels) in either direction, because Claude's
dark sidebar is darker than its canvas. The invariant "muted text darker than
primary text" is evaluated per light or dark palette using §2's luminance rule.

### 2. Light detection and readable accent text

The GUI decides light or dark visuals from the relative luminance of `bg`
(light when above 0.5) instead of the current `name == "studio"` check, so no
field is added to the shared schema. `studio` and `claude_light` are light; the
other five palettes are dark.

The brand accent is used for fills, icons, and focus rings, where 3:1 is
required. `#C96442` measures 3.70:1 on `#FAF9F5`, below the 4.5:1 needed for
text, so accent-colored text uses a derived color: the accent blended toward
`text_primary` in small steps until it reaches 4.5:1 on each surface it is
drawn on. The derivation is a pure function with tests for every palette.
It applies to all seven palettes, so accent-colored text in another theme
shifts slightly toward its text color wherever that theme's accent currently
fails 4.5:1; accent fills and icons do not change.

### 3. Typography

- **Serif** (Georgia, then Cambria, then the sans family): the welcome greeting
  (30 px), page and drawer titles (22 px), task titles (17 px), and
  conversation prose (16 px, relaxed line height).
- **Sans** (Segoe UI): all interface text, as today.
- **Monospace**: identifiers, branches, and paths.
- The CJK fallback chain is unchanged and is appended to the serif family, so
  CJK titles still render.

Fonts are read once at startup from the Windows font directory, as the GUI
already does for Segoe UI and CJK fonts. No font files are bundled or
redistributed. Off Windows, the serif family falls back to the sans family.

### 4. Shell layout and components

- **Sidebar** (260 px expanded, 56 px collapsed icon rail): a brand row with a
  spark mark and a collapse button; a `+ New task` button with the plus in an
  accent circle; Conversation and Agents entries; a Recents list with one
  truncated line per task and a status dot only for running or blocked tasks;
  and a workspace button at the bottom that opens a menu. In phase 1 the menu
  holds Settings and Diagnostics; phase 2 adds Appearance and Keyboard
  shortcuts.
- **Spark mark**: a generic eight-ray spark drawn with painter strokes. It is
  not a reproduction of Anthropic's or any other trademarked logo.
- **Top bar**: page title, a connection dot with a text label, the `DEMO` tag
  when applicable, Refresh as an icon button with a tooltip, and a hairline
  divider.
- **Conversation**: a centered column about 720 px wide. The daemon's update
  message uses the spark avatar. Task rows are borderless with a hover tint
  and keep their Details, Open window, and Native terminal actions.
- **Composer**: a rounded box (20 px radius, soft shadow) on `surface_alt` with
  the placeholder "How can I help today?", a target chip on the left, and a
  circular send button on the right. The send button stays disabled with the
  existing reason tooltip, and the caption "Draft only · coordinator chat is
  not connected" remains visible whenever sending is unavailable. Enter
  attempts to send and Shift+Enter inserts a newline. While sending is
  unavailable, Enter leaves the draft unchanged and shows "Not sent:
  coordinator chat is not connected" for 2 s. Enter during IME composition
  does nothing.
- **Welcome state**: shown when the user starts a new task or the workspace has
  no tasks. The spark and a serif greeting sit above a centered composer. The
  greeting is "Good morning" (05:00 to 11:59), "Good afternoon" (12:00 to
  17:59), or "Good evening" (18:00 to 04:59) in local time, and "Hello" when
  the local offset is unavailable. It never shows a user name. Selecting
  Conversation, Agents, or a task in the sidebar leaves the welcome state.
- **Task drawer**: a docked, resizable right panel (default 420 px, minimum
  360 px, maximum half the window width) below the top bar, with one title row
  and a close icon, underline tabs, and run details in an aligned grid of
  muted labels and monospace values. The detached task window from ADR 028 is
  unchanged. The floating overlay previously used below 1400 px is removed;
  the drawer is always docked.
- **Layout fixes**: task rows are borderless, the conversation's scroll region
  fades out over its last 28 px while more content follows, and 48 px of bottom
  padding lets the last task scroll fully into view.
- **Radii and elevation**: cards 12 px, composer 20 px, buttons 8 px, pills
  fully rounded. Shadows appear only on the composer and popups.

These components are shared by all themes; only the colors differ.

### 5. Functions

- **New task** (sidebar button, and `Ctrl+N` in phase 2) opens the welcome
  state with an empty, focused composer. If the current draft is not empty, it
  first asks "Discard current draft?" with Discard and Keep editing. The draft
  exists only in memory.
- **Composer target chip** lists Coordinator plus every harness whose probe
  state is available. Undetected harnesses are listed disabled with their
  reason. The choice only changes the caption, for example "Draft for Codex ·
  not sent", and is not persisted. It never enables Send.
- **Sidebar collapse** (button, and `Ctrl+B` in phase 2) toggles between the
  expanded sidebar and the icon rail.
- **Quick switcher** (`Ctrl+K`): a centered popup with a focused search field
  and at most 8 results. Up and Down move the selection, Enter opens it, and
  Esc closes. Entries come from the loaded task page (title, task ID, harness),
  the agents (display name), and a fixed action list (New task, Settings,
  Diagnostics, Light, Dark, Match system, Keyboard shortcuts, Toggle sidebar).
  An empty query lists tasks first, then actions, then agents. Matching is
  case-insensitive Unicode; a label prefix ranks above a word-start match,
  which ranks above a substring match, and ties keep source order. Opening a
  task opens its drawer.
- **Shortcuts** come from one table in `gui/shortcuts.rs` that drives both key
  handling and the `Ctrl+/` overlay:

  | Keys | Action |
  |---|---|
  | `Ctrl+K` | Quick switcher |
  | `Ctrl+N` | New task |
  | `Ctrl+B` | Toggle sidebar |
  | `Ctrl+,` | Settings |
  | `Ctrl+/` | Shortcut overlay |
  | `Esc` | Close the top-most overlay (existing) |
  | `Ctrl+1` to `Ctrl+0` | Select an agent (existing) |

  The new chords are consumed before text widgets see them, so they also work
  while the composer has focus. They are ignored during IME composition, as
  the existing handler already does. `Esc` and `Ctrl+digit` keep their current
  rule of not firing while a text field has focus.
- **Appearance**: a Light, Dark, and Match system control in Settings and in
  the workspace menu. Light selects `claude_light` and Dark selects `claude`.
  Match system switches between the pair whenever egui reports an OS theme
  change and uses light when the OS reports nothing. Choosing any other theme
  from the theme list turns Match system off. The control applies only to the
  Claude pair.
- **Reduce motion**: a Settings toggle, off by default.
- **Spark animation**: running tasks show a slowly rotating spark (one turn per
  2.4 s). The GUI requests a repaint about every 50 ms only while a running
  task is visible and the window has focus. Otherwise the GUI keeps its
  existing 100 ms refresh and requests nothing extra. With Reduce motion on,
  the spark is static.
- **Copy buttons** on task titles, task and run IDs, branches, and worktree
  paths copy that text to the local clipboard and show "Copied" for 1.5 s.
  Nothing is sent anywhere.

### 6. Module ownership

- `theme/appearance.rs`: the appearance mode and the pure resolution of the
  effective palette from the stored theme, the follow flag, and the OS theme.
- `theme/palette.rs`: the new and retuned palettes and luminance-based light
  detection.
- `quick_search.rs` (crate level, no egui dependency): entry model, matching,
  and ranking.
- `gui/typography.rs`: system font loading moved from `supervisor_app.rs`, plus
  the serif family.
- `gui/shortcuts.rs`: the shortcut table.
- `gui/composer.rs`, `gui/welcome.rs`, `gui/quick_switcher.rs`, `gui/motion.rs`:
  one component each.
- `gui/sidebar.rs` and `gui/task_detail.rs` are extended in place;
  `gui/supervisor_app.rs` keeps orchestration and loses the font code.
- The four uncompiled legacy files are deleted in a separate commit.
- `theme/contrast.rs`: WCAG relative luminance, contrast ratio, and the
  readable text color derivation.
- `gui/shortcuts.rs` also renders the `Ctrl+/` overlay from the same table.
- `quick_search.rs` also builds the search entries from the snapshot and the
  probe view model.

### 7. Persisted settings

`UserConfig` gains `follow_system_appearance`, `reduce_motion`, and
`sidebar_collapsed`, all booleans marked `#[serde(default)]` and defaulting to
false. `schema_version` stays 1. New profiles default to `claude_light`.
Existing saved themes are kept, so a current `claude` user stays on the dark
theme.

### 8. Delivery

One implementation plan, delivered as two pull requests:

1. Phase 1: palettes, light detection, readable accent text, typography, the
   sidebar shell with collapse and New task, the welcome state and composer
   with the target chip, the drawer, clipping, and grid fixes, the legacy file
   deletion, and new preview scenarios.
2. Phase 2: the shortcut table and overlay, the quick switcher, the Appearance
   control with Match system and Reduce motion, the spark animation, and the
   copy buttons.

## Compatibility and rollback

- The exported palette file gains one entry at the end; consumers load
  palettes by name. Python tests that pin the palette list and fallback values
  are updated in the same change.
- Old binaries ignore the three new config fields. An old binary that reads
  the `claude_light` token cannot parse it and falls back to its defaults, the
  same trade-off ADR 026 accepted for `nord`.
- The daemon, the control protocol, the plugin protocol, and the database are
  untouched. Rolling back is reverting the GUI change; no migration is needed.

## Security and privacy

- Send remains unavailable and never produces a fake message or response.
- The greeting shows no user name, and nothing reads the OS account name.
- The clipboard is written only on an explicit click and only with the shown
  text. No new data leaves the process.
- The GUI reads system fonts only from the Windows font directory and never
  from paths supplied by project files.

## Verification

- Unit tests: switcher ranking (ASCII, CJK, IDs, empty query, result cap);
  appearance resolution for every theme, follow flag, and OS theme
  combination; light detection for all seven palettes; readable accent text of
  at least 4.5:1 on `bg`, `surface`, and `surface_alt` for every palette; greeting
  hour boundaries; no duplicate chords in the shortcut table; loading a config
  saved before this change preserves its theme.
- UI state tests: sidebar collapse, the discard-draft confirmation, opening a
  drawer from the switcher, and Send staying disabled.
- `palette_parity` and the Python theme tests.
- Preview screenshots for the scenarios `chat`, `drawer`, `task`, `welcome`,
  `switcher`, `shortcuts`, and `collapsed`, in `claude_light` and `claude`, at
  1280×800, at the 960×600 minimum, and at 1.5× scale. The committed previews
  in `docs/frontend_previews/` are regenerated.
- Gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace --all-features`, and
  the Python pytest, ruff, and mypy checks.

Screenshots prove rendering at the tested sizes. They do not prove behavior
with other system fonts, other display scales, IME input, or screen readers.

## Alternatives

- Palettes and fonts only: cheapest, but the drawer defects and the boxed look
  remain.
- A full Claude Desktop restructure with a chat history sidebar and an
  artifacts pane: rejected because it reopens ADR 028's layout decisions and
  adds surfaces the daemon cannot feed yet.
