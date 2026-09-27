# ADR 026: Multi-surface theme palettes (Nord and Gruvbox) with a shared exported color source

Status: Accepted for pre-alpha implementation

## Context

The two-shell frontend (ADR 016) leaves theme ownership split: the egui GUI
persists hex `ThemeId` palettes (`claude`/`github`/`vscode`) in the user
config, while the Ratatui TUI debug view owns its own audited
ratatui-color `Theme` system (`classic`/`high-contrast`/`mono`/`light`).
Neither surface shares palette data with the Python prototype CLI (whose
only color is a hardcoded `[red]error:` label) or with the terminal
backends (WezTerm on Windows, tmux on POSIX, which have no VibeMux-generated
appearance at all), so panes launched by VibeMux never match the console
that supervises them.

Two additional brand themes are wanted — Nord (cool frost blues and storm
grays) and Gruvbox (retro warm olive/orange) — and the styling should cover
every surface VibeMux owns: the GUI, the TUI, the Python CLI output, and
generated terminal backend appearance.

## Decision

1. **GUI palettes.** `ThemeId` gains `Nord` and `Gruvbox` with full
   `ThemePalette` values (background ramp, text pair, accent pair,
   success/warning/danger, terminal colors) following the existing
   palette invariants (surface lighter than background, muted text
   darker than primary). The cycle order is
   `claude -> github -> vscode -> nord -> gruvbox -> claude`.
   (Amended by ADR 027: the default theme is now `github`, not
   `claude`, and the `github` palette carries the exact Primer dark
   tokens.) Adding enum variants is backward compatible for persisted
   configs: old binaries that read a `nord` config fall back to
   defaults, and old configs without the new names keep loading.
2. **Shared exported color source.** `config/theme_palettes.json`
   (schema_version 1, snake_case keys) mirrors the compiled-in GUI
   palettes exactly. Ownership is enforced, not assumed: the Rust
   `palette_parity` integration test compares the serialized
   `all_palettes()` output against the file in the same commit. The
   Python side consumes this file; it never redefines colors.
3. **TUI brand themes.** The TUI `Theme` enum gains `nord` and `gruvbox`
   expressed as xterm-256 `Color::Indexed` values resolved from the
   brand hex ramps. Indexed colors render identically on WezTerm (the
   Windows primary backend) and tmux (the POSIX backend) without a
   truecolor dependency, which is why they were chosen over `Rgb` for
   these themes. The nearest 256-color index to Nord's brand red measures
   only 4.64:1 against black — inside the WCAG AA gate but too close to
   trust across terminal palettes — so the failure accent takes the
   brighter index 167. The existing WCAG AA audit test now covers every
   new accent (state, health, title, route, muted) using a full
   xterm-256 index-to-RGB resolver, and golden fixtures pin the rendered
   layout for the new themes at 80x24 and 120x32.
4. **Python CLI theming.** `src/vibemux/theme.py` loads and validates
   the exported file (schema version, unique names, `#RRGGBB` fields),
   resolves the active palette from `VIBEMUX_THEME` (unknown values fall
   back to `claude` with a stderr warning, mirroring the Rust fallback
   semantics), and maps the roles onto rich styles (`vm.error`,
   `vm.success`, `vm.warning`, `vm.accent`, `vm.border`, `vm.muted`).
   A broken palette file degrades to a built-in copy of the default
   palette instead of failing every command. Human-facing output
   (tables, error lines, init/switch/stop status) is role-styled;
   machine-readable `--json` output stays theme-independent, matching
   the frontend rule.
5. **Terminal backend appearance is generated, never applied.**
   `src/vibemux/terminal_theme.py` renders a palette as a WezTerm
   `config.colors` lua table (hex values) or a tmux `set -g` option
   block (`colour<N>` values computed as the nearest xterm-256 entries).
   `vibemux theme --list` prints the palette names; `vibemux theme
   --backend wezterm|tmux --theme <name>` prints the snippet to stdout.
   VibeMux never writes terminal configuration files, never mutates a
   running terminal's palette, and never touches a user-owned
   WezTerm/tmux config: adopting the appearance is an explicit user
   action. The generated snippets style pane chrome only (foreground,
   background, cursor, selection, split/border); the ANSI 16-color ramp
   is intentionally left to the terminal's base scheme so one snippet
   stays consistent across backends.

## Consequences

- Every themed surface now draws Nord and Gruvbox from the same declared
  colors: GUI hex values are canonical, TUI indexed values are their
  documented 256-color approximations, and generated terminal snippets
  use hex (WezTerm) or nearest-color-index (tmux) forms of the same
  values.
- The exported JSON is a public-in-repo artifact: any palette change
  must update `config/theme_palettes.json` in the same commit or the
  parity test fails. Layout-only palette fields (font, radius, density)
  are tolerated but ignored by the Python consumer.
- Wheel installs of the Python prototype resolve the palette file
  relative to a repository checkout; a missing file degrades to the
  built-in default with a stderr note instead of failing.
- Brand fidelity on the TUI is bounded by the 256-color palette: indexed
  approximations are visibly close but not exact matches to the official
  Nord/Gruvbox hex values. Truecolor rendering remains available through
  the GUI and the generated WezTerm snippet.
- tmux snippets are generated but not live-validated on POSIX in this
  change (Windows-first repository); the `colour<N>` values are derived
  from the standard xterm-256 palette and pinned by unit tests.
