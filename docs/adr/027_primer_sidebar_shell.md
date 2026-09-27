# ADR 027: GitHub-Primer-styled sidebar shell for the GUI

Status: Superseded for home screen/navigation by [ADR 028](028_supervisor_chat_frontend.md). Palette compatibility remains supported.

## Context

The first egui shell (topbar with theme chips, editorial overview canvas,
icon-rail workbench) was judged visually poor: the layout mixed three
different navigation idioms, the editorial hero wasted the overview, and
the icon rail duplicated navigation that a persistent list serves better.
The user asked for a complete redesign in the GitHub style, modeled on the
Claude Desktop layout.

## Decision

1. **Layout (Claude Desktop structure).** One persistent left sidebar
   (~232 px) holds the brand, the Overview entry, the ten harness rows
   with live status dots and their `Ctrl+<digit>` accelerators, and a
   pinned footer (theme picker, Settings/Diagnostics toggles, shortcut
   hint). The main content area switches between the Overview page
   (page header with count badge, four bordered summary cards, the
   agent table in a bordered box with hairline row separators) and the
   Seat page (page header with a state pill, bordered transcript card,
   Claude-style rounded composer, right-hand inspector). A thin status
   bar spans the bottom of the main area with probe facts and aggregate
   health.
2. **Skin (GitHub Primer dark).** The `github` palette is now the exact
   GitHub Primer dark token set: canvas `#0d1117`, subtle `#161b22`,
   inset `#21262d`, border `#30363d`, fg `#e6edf3` / muted `#8b949e`,
   accent `#2f81f7`, success/attention/danger `#3fb950`/`#d29922`/
   `#f85149`. Components follow Primer shapes: 6 px corner radius,
   1 px border-colored strokes on boxes and buttons, bordered state
   pills, uppercase mono section labels. **The default theme changes
   from `claude` to `github`** so the out-of-the-box look matches the
   shell; persisted user configs are unaffected.
3. **Preserved contracts.** Ctrl+1..0 opens seats 1..10, Ctrl+0 wraps to
   the tenth seat (issue #5), Escape closes overlays before returning
   from a seat to the overview, Ctrl+T cycles themes, Ctrl+, and Ctrl+;
   toggle the overlays, theme changes persist through `UserConfig`
   debounced writes, `handle_stub_send`/`harness_terminal` keep their
   semantics, and the GUI still shows stub transcripts only (no PTY
   attach). The topbar module is deleted; navigation is sidebar-only.
4. **Palette plumbing.** The shared palette-derived color struct gains
   the mid surface and danger colors so boxes, sidebar, and failure
   states do not need ad-hoc alpha mixtures. All five themes continue to
   drive the same shell; only colors differ.

## Consequences

- The theme picker moved from the topbar into the sidebar footer; the
  theme chips no longer exist.
- The overview hero headline is replaced by a functional dashboard; the
  former hardcoded workspace path label is gone with the topbar.
- The headless render guards were rewritten to mirror the new shell
  (sidebar + overview, sidebar + seat).
- GUI-only change: the TUI, the Python CLI, and the exported palette
  file mechanism are untouched, except that the `github` entry of
  `config/theme_palettes.json` changed values (parity-pinned in the same
  change) and the default `ThemeId` is now `github`.
