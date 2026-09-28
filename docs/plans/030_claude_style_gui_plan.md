# Claude-style GUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement ADR 030: a Claude-style light and dark theme pair plus Claude Desktop shell elements and functions in the egui Supervisor Chat GUI, delivered as two pull requests.

**Architecture:** Pure logic (contrast math, palettes, appearance resolution, search ranking, shortcut table, greeting, motion timing) lives in egui-free modules or egui-type-only data modules with unit tests. Each egui component owns one file under `crates/vibemux_frontend/src/gui/`; `supervisor_app.rs` keeps orchestration. The shared palette JSON and the Python fallback change in lockstep under the existing parity test.

**Tech Stack:** Rust 1.85.0 (edition 2024), egui/eframe 0.32.3, serde/serde_json, time 0.3.44 (adds the `local-offset` feature), Python 3.12 (pytest, ruff, mypy).

**Spec:** `docs/adr/030_claude_style_gui.md` (Task 1 corrects it before implementation starts).

## Global Constraints

- Rust 1.85.0, edition 2024: no `if let` chains. Every new Rust file starts with `#![forbid(unsafe_code)]`.
- snake_case for every new file, module, function, variable, constant value string, config key, and JSON key. No hyphens in any new project-controlled name. Types use CamelCase.
- Send stays unavailable: `SupervisorUiState::send_enabled()` returns `false`, and `try_send()` returns `false` without changing the draft or enqueuing an action.
- No control without a local function. The greeting never shows a user name, and nothing reads the OS account name.
- The clipboard is written only on an explicit click, with exactly the shown text.
- Fonts are read only from `%WINDIR%\Fonts`. No font file is bundled.
- `config/theme_palettes.json` must equal the compiled palettes (`palette_parity`). `ThemeId::ALL` order equals the JSON array order: `claude, github, vscode, nord, gruvbox, studio, claude_light`.
- `UserConfig.schema_version` stays 1. New fields are `bool` with `#[serde(default)]`.
- Accent-colored text reaches at least 4.5:1 on `bg`, `surface`, and `surface_alt` in every palette. The Claude pair's primary and muted text reach at least 4.5:1 on all three.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Python checks run from the worktree with `PYTHONPATH="$PWD/src"` and the project venv interpreter `../../../.venv/Scripts/python.exe`, because the venv's editable install points at the main checkout.

## Review Focus

1. Very long or CJK task titles in the Recents list must be truncated inside the 260 px sidebar, never overflow it. Test: Task 7 `recents_truncate_long_cjk_titles_inside_the_sidebar`.
2. Enter or a global shortcut pressed during IME composition must do nothing. Tests: Task 8 `enter_during_ime_composition_is_ignored` and Task 14 `global_shortcuts_are_ignored_during_ime_composition`.
3. At the 960×600 minimum with the drawer open, the send button must stay on screen. Test: Task 10 `task_drawer_docks_below_the_header_without_duplicate_chrome`.
4. Match system with an OS that reports no theme must render light. Test: Task 13 `match_system_without_an_os_theme_renders_light`.
5. A config file written before this change (no new fields) must load with its theme preserved, and a whitespace-only draft must not trigger the discard prompt. Tests: Task 7 `config_without_new_fields_keeps_theme`, Task 13 `config_without_appearance_fields_keeps_theme`, Task 6 `whitespace_draft_starts_a_new_task_without_prompting`.

---

# Phase 1 (pull request 1)

### Task 1: Correct ADR 030 before implementation

**Files:**
- Modify: `docs/adr/030_claude_style_gui.md`

**Interfaces:** none (documentation).

- [ ] **Step 1: Replace the clipping sentence in Context**

Replace:

```text
- The task list scrolls underneath the composer and its last card is clipped.
```

with:

```text
- The bordered task list is cut off sharply at the edge of its scroll region,
  above the composer, with no cue that more tasks follow.
```

- [ ] **Step 2: Scope the contrast goal**

Replace:

```text
- WCAG AA contrast: at least 4.5:1 for text and 3:1 for icons and control
  boundaries.
```

with:

```text
- WCAG AA contrast: at least 4.5:1 for text in the two Claude palettes and for
  accent-colored text in every palette, and 3:1 for icons and control
  boundaries. Other text colors in the five older palettes are unchanged.
```

- [ ] **Step 3: Record the ADR 026 invariant amendment at the end of §1**

Append after the paragraph that ends "The Python CLI's default theme stays `claude`.":

```text
This ADR amends ADR 026's palette invariant "surface lighter than background":
the surface must instead differ from the background by at least 5.0 luma units
(Rec. 709 weights on 0 to 255 channels) in either direction, because Claude's
dark sidebar is darker than its canvas. The invariant "muted text darker than
primary text" is evaluated per light or dark palette using §2's luminance rule.
```

- [ ] **Step 4: Add Enter behavior to the composer bullet in §4**

Replace the composer bullet's last sentence ("The send button stays disabled with the existing reason tooltip, and the caption ... is unavailable.") with:

```text
  The send button stays disabled with the existing reason tooltip, and the
  caption "Draft only · coordinator chat is not connected" remains visible
  whenever sending is unavailable. Enter attempts to send and Shift+Enter
  inserts a newline. While sending is unavailable, Enter leaves the draft
  unchanged and shows "Not sent: coordinator chat is not connected" for 2 s.
  Enter during IME composition does nothing.
```

- [ ] **Step 5: Correct the layout fix and drawer bullets in §4**

Replace the "Layout fixes" bullet with:

```text
- **Layout fixes**: task rows are borderless, the conversation's scroll region
  fades out over its last 28 px while more content follows, and 48 px of bottom
  padding lets the last task scroll fully into view.
```

Append to the "Task drawer" bullet:

```text
  The floating overlay previously used below 1400 px is removed; the drawer is
  always docked.
```

Replace in the composer target chip bullet "every harness whose probe state is available" with "every harness whose launcher probe state is verified".

- [ ] **Step 6: Correct the repaint cadence in §5**

Replace:

```text
  task is visible and the window has focus, and about every 500 ms while
  unfocused. With Reduce motion on, the spark is static and no extra repaints
  are requested.
```

with:

```text
  task is visible and the window has focus. Otherwise the GUI keeps its
  existing 100 ms refresh and requests nothing extra. With Reduce motion on,
  the spark is static.
```

- [ ] **Step 7: Complete the module list in §6**

Add these bullets to §6:

```text
- `theme/contrast.rs`: WCAG relative luminance, contrast ratio, and the
  readable text color derivation.
- `gui/shortcuts.rs` also renders the `Ctrl+/` overlay from the same table.
- `quick_search.rs` also builds the search entries from the snapshot and the
  probe view model.
```

- [ ] **Step 8: Verify and commit**

Run: `git diff --check`
Expected: no output.

```bash
git add docs/adr/030_claude_style_gui.md
git commit -m "docs(adr): correct ADR 030 before implementation

Scope the contrast goal, amend ADR 026's surface invariant, specify Enter and
Shift+Enter in the composer, describe the real clipping defect and fix, remove
the floating drawer, and keep the existing 100 ms refresh cadence.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Contrast math and luminance-based light detection

**Files:**
- Create: `crates/vibemux_frontend/src/theme/contrast.rs`
- Modify: `crates/vibemux_frontend/src/theme/mod.rs` (declare the module)
- Modify: `crates/vibemux_frontend/src/theme/palette.rs` (add `ThemePalette::is_light`)
- Modify: `crates/vibemux_frontend/src/gui/theme.rs` (use `is_light`)

**Interfaces:**
- Produces: `crate::theme::contrast::{Rgb, TEXT_CONTRAST_MINIMUM, LIGHT_BACKGROUND_LUMINANCE, relative_luminance(Rgb) -> f32, contrast_ratio(Rgb, Rgb) -> f32, blend(Rgb, Rgb, f32) -> Rgb, readable_text_color(Rgb, &[Rgb], Rgb, f32) -> Rgb}` and `ThemePalette::is_light(&self) -> bool`.

- [ ] **Step 1: Write the failing tests**

Create `crates/vibemux_frontend/src/theme/contrast.rs` with only the tests first:

```rust
#![forbid(unsafe_code)]
//! WCAG 2.x relative luminance and contrast math on `(r, g, b)` triples.
//! Pure functions; the GUI converts results to `egui::Color32`.

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgb = (0, 0, 0);
    const WHITE: Rgb = (255, 255, 255);

    #[test]
    fn luminance_spans_zero_to_one() {
        assert!(relative_luminance(BLACK).abs() < 1e-6);
        assert!((relative_luminance(WHITE) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn contrast_is_symmetric_and_bounded() {
        assert!((contrast_ratio(BLACK, WHITE) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(WHITE, BLACK) - 21.0).abs() < 0.01);
        assert!((contrast_ratio((201, 100, 66), (201, 100, 66)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn blend_hits_both_endpoints() {
        assert_eq!(blend((10, 20, 30), (110, 120, 130), 0.0), (10, 20, 30));
        assert_eq!(blend((10, 20, 30), (110, 120, 130), 1.0), (110, 120, 130));
        assert_eq!(blend((0, 0, 0), (100, 100, 100), 0.5), (50, 50, 50));
    }

    #[test]
    fn passing_color_is_returned_unchanged() {
        assert_eq!(
            readable_text_color(WHITE, &[BLACK], (200, 200, 200), TEXT_CONTRAST_MINIMUM),
            WHITE
        );
    }

    #[test]
    fn failing_accent_is_pulled_toward_text_until_readable() {
        let terracotta = (0xC9, 0x64, 0x42);
        let backgrounds = [(0xFA, 0xF9, 0xF5), (0xF5, 0xF4, 0xED), (0xFF, 0xFF, 0xFF)];
        let text = (0x14, 0x14, 0x13);
        let readable = readable_text_color(terracotta, &backgrounds, text, TEXT_CONTRAST_MINIMUM);
        assert_ne!(readable, terracotta);
        for background in backgrounds {
            assert!(contrast_ratio(readable, background) >= TEXT_CONTRAST_MINIMUM);
        }
    }
}
```

Add to `crates/vibemux_frontend/src/theme/mod.rs` below `mod palette;`:

```rust
pub mod contrast;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p vibemux_frontend --lib contrast`
Expected: compile errors such as `cannot find type Rgb` and `cannot find function relative_luminance`.

- [ ] **Step 3: Implement the functions**

Insert above the `#[cfg(test)]` line of `contrast.rs`:

```rust
/// WCAG AA minimum contrast for normal-size text.
pub const TEXT_CONTRAST_MINIMUM: f32 = 4.5;
/// Relative luminance above which a background counts as light.
pub const LIGHT_BACKGROUND_LUMINANCE: f32 = 0.5;
/// Blend step used when deriving a readable text color.
const BLEND_STEP: f32 = 0.05;

pub type Rgb = (u8, u8, u8);

fn channel_to_linear(channel: u8) -> f32 {
    let value = f32::from(channel) / 255.0;
    if value <= 0.039_28 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[must_use]
pub fn relative_luminance(color: Rgb) -> f32 {
    0.2126 * channel_to_linear(color.0)
        + 0.7152 * channel_to_linear(color.1)
        + 0.0722 * channel_to_linear(color.2)
}

#[must_use]
pub fn contrast_ratio(first: Rgb, second: Rgb) -> f32 {
    let first = relative_luminance(first);
    let second = relative_luminance(second);
    let (lighter, darker) = if first >= second {
        (first, second)
    } else {
        (second, first)
    };
    (lighter + 0.05) / (darker + 0.05)
}

#[must_use]
pub fn blend(from: Rgb, toward: Rgb, amount: f32) -> Rgb {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    (mix(from.0, toward.0), mix(from.1, toward.1), mix(from.2, toward.2))
}

/// Return `color` unchanged when it already reaches `minimum` on every
/// background; otherwise blend it toward `toward` (the palette's primary
/// text color, which always passes) in fixed steps until it does.
#[must_use]
pub fn readable_text_color(color: Rgb, backgrounds: &[Rgb], toward: Rgb, minimum: f32) -> Rgb {
    let passes = |candidate: Rgb| {
        backgrounds
            .iter()
            .all(|background| contrast_ratio(candidate, *background) >= minimum)
    };
    let mut amount = 0.0_f32;
    loop {
        let candidate = blend(color, toward, amount);
        if passes(candidate) || amount >= 1.0 {
            return candidate;
        }
        amount = (amount + BLEND_STEP).min(1.0);
    }
}
```

- [ ] **Step 4: Add light detection and use it**

In `crates/vibemux_frontend/src/theme/palette.rs`, add inside `impl ThemePalette` after `rgb`:

```rust
    /// Light when the canvas's relative luminance exceeds 0.5 (ADR 030 §2).
    #[must_use]
    pub fn is_light(&self) -> bool {
        self.rgb("bg").is_some_and(|bg| {
            crate::theme::contrast::relative_luminance(bg)
                > crate::theme::contrast::LIGHT_BACKGROUND_LUMINANCE
        })
    }
```

Add to the palette tests module:

```rust
    #[test]
    fn light_detection_uses_canvas_luminance() {
        assert!(palette_for(ThemeId::Studio).is_light());
        assert!(!palette_for(ThemeId::Github).is_light());
        assert!(!palette_for(ThemeId::Claude).is_light());
    }
```

In `crates/vibemux_frontend/src/gui/theme.rs`, replace `let light = p.name == "studio";` with:

```rust
    let light = p.is_light();
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p vibemux_frontend --lib -- contrast light_detection`
Expected: 6 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/vibemux_frontend/src/theme/contrast.rs crates/vibemux_frontend/src/theme/mod.rs crates/vibemux_frontend/src/theme/palette.rs crates/vibemux_frontend/src/gui/theme.rs
git commit -m "feat(frontend): WCAG contrast math and luminance-based light detection

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Claude palette pair and the shared palette contract

**Files:**
- Modify: `crates/vibemux_frontend/src/theme/mod.rs` (`ThemeId::ClaudeLight`, order, default, tests)
- Modify: `crates/vibemux_frontend/src/theme/palette.rs` (retuned `claude`, new `claude_light`, invariant tests)
- Modify: `config/theme_palettes.json`
- Modify: `src/vibemux/theme.py` (fallback copy)
- Modify: `tests/test_theme.py`

**Interfaces:**
- Consumes: `contrast::{contrast_ratio, readable_text_color, TEXT_CONTRAST_MINIMUM}`, `ThemePalette::is_light` (Task 2).
- Produces: `ThemeId::ClaudeLight` (token `claude_light`), `ThemeId::ALL: [ThemeId; 7]`, `all_palettes() -> [ThemePalette; 7]`, `ThemeId::default() == ThemeId::ClaudeLight`.

- [ ] **Step 1: Write the failing Rust tests**

In `theme/mod.rs` tests, replace `theme_id_cycle_wraps`, `theme_id_as_str_is_lowercase`, `theme_id_default_is_studio`, and `all_palettes_returns_six_in_canonical_order` with:

```rust
    #[test]
    fn theme_id_cycle_wraps() {
        assert_eq!(ThemeId::Claude.cycle(), ThemeId::Github);
        assert_eq!(ThemeId::Github.cycle(), ThemeId::Vscode);
        assert_eq!(ThemeId::Vscode.cycle(), ThemeId::Nord);
        assert_eq!(ThemeId::Nord.cycle(), ThemeId::Gruvbox);
        assert_eq!(ThemeId::Gruvbox.cycle(), ThemeId::Studio);
        assert_eq!(ThemeId::Studio.cycle(), ThemeId::ClaudeLight);
        assert_eq!(ThemeId::ClaudeLight.cycle(), ThemeId::Claude);
    }

    #[test]
    fn theme_id_as_str_is_snake_case() {
        for id in ThemeId::ALL {
            assert!(id.as_str().chars().all(|c| c.is_ascii_lowercase() || c == '_'));
            assert!(!id.as_str().starts_with('_') && !id.as_str().ends_with('_'));
        }
    }

    #[test]
    fn theme_id_default_is_claude_light() {
        assert_eq!(ThemeId::default(), ThemeId::ClaudeLight);
    }

    #[test]
    fn claude_light_serializes_as_snake_case_token() {
        assert_eq!(
            serde_json::to_string(&ThemeId::ClaudeLight).expect("serialize"),
            "\"claude_light\""
        );
    }

    #[test]
    fn all_palettes_returns_seven_in_canonical_order() {
        let names: Vec<String> = all_palettes().into_iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            ["claude", "github", "vscode", "nord", "gruvbox", "studio", "claude_light"]
        );
    }
```

In `theme/palette.rs` tests, replace the whole `themes_share_grayscale_base` test with:

```rust
    /// Minimum luma gap (Rec. 709 weights, 0-255) between canvas and sidebar
    /// surface in either direction (ADR 030 amends ADR 026's direction rule).
    const MINIMUM_SURFACE_SEPARATION: f32 = 5.0;

    fn luma(rgb: (u8, u8, u8)) -> f32 {
        0.2126 * f32::from(rgb.0) + 0.7152 * f32::from(rgb.1) + 0.0722 * f32::from(rgb.2)
    }

    #[test]
    fn themes_share_grayscale_base() {
        for palette in all_palettes() {
            assert_eq!(palette.surface_alt.len(), 7);
            assert_eq!(palette.text_primary.len(), 7);
            assert_eq!(palette.text_muted.len(), 7);
            assert_eq!(palette.terminal_bg.len(), 7);
            assert_eq!(palette.terminal_fg.len(), 7);
            let bg = palette.rgb("bg").expect("bg");
            let surface = palette.rgb("surface").expect("surface");
            assert!(
                (luma(surface) - luma(bg)).abs() >= MINIMUM_SURFACE_SEPARATION,
                "{}: surface must be visibly separated from canvas",
                palette.name
            );
            let primary = luma(palette.rgb("text_primary").expect("primary"));
            let muted = luma(palette.rgb("text_muted").expect("muted"));
            assert!(
                if palette.is_light() {
                    primary < muted
                } else {
                    primary > muted
                },
                "{}: primary text must contrast more strongly than muted text",
                palette.name
            );
        }
    }

    #[test]
    fn light_palettes_are_studio_and_claude_light() {
        let names: Vec<String> = all_palettes()
            .into_iter()
            .filter(ThemePalette::is_light)
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["studio", "claude_light"]);
    }

    #[test]
    fn claude_pair_text_meets_wcag_aa_on_every_surface() {
        use crate::theme::contrast::{TEXT_CONTRAST_MINIMUM, contrast_ratio};
        for id in [ThemeId::Claude, ThemeId::ClaudeLight] {
            let p = palette_for(id);
            for text in ["text_primary", "text_muted"] {
                for surface in ["bg", "surface", "surface_alt"] {
                    let ratio = contrast_ratio(
                        p.rgb(text).expect("text"),
                        p.rgb(surface).expect("surface"),
                    );
                    assert!(
                        ratio >= TEXT_CONTRAST_MINIMUM,
                        "{} {text} on {surface}: {ratio:.2}",
                        p.name
                    );
                }
            }
        }
    }

    #[test]
    fn readable_accent_text_meets_wcag_aa_in_every_palette() {
        use crate::theme::contrast::{TEXT_CONTRAST_MINIMUM, contrast_ratio, readable_text_color};
        for p in all_palettes() {
            let surfaces: Vec<(u8, u8, u8)> = ["bg", "surface", "surface_alt"]
                .into_iter()
                .map(|field| p.rgb(field).expect("surface"))
                .collect();
            let text = readable_text_color(
                p.rgb("accent").expect("accent"),
                &surfaces,
                p.rgb("text_primary").expect("text"),
                TEXT_CONTRAST_MINIMUM,
            );
            for surface in &surfaces {
                assert!(contrast_ratio(text, *surface) >= TEXT_CONTRAST_MINIMUM, "{}", p.name);
            }
        }
    }

    #[test]
    fn claude_pair_uses_the_approved_tokens() {
        let light = palette_for(ThemeId::ClaudeLight);
        assert_eq!(
            (light.bg.as_str(), light.surface.as_str(), light.accent.as_str()),
            ("#FAF9F5", "#F5F4ED", "#C96442")
        );
        let dark = palette_for(ThemeId::Claude);
        assert_eq!(
            (dark.bg.as_str(), dark.surface.as_str(), dark.surface_alt.as_str()),
            ("#262624", "#1F1E1D", "#30302E")
        );
    }
```

In `every_palette_name_matches_id`, add:

```rust
        assert_eq!(palette_for(ThemeId::ClaudeLight).name, "claude_light");
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p vibemux_frontend --lib theme`
Expected: compile error `no variant named ClaudeLight`.

- [ ] **Step 3: Add the theme id**

In `theme/mod.rs`, update the module doc's first line to `//! Theme system: two light palettes (Studio, Claude light) and five dark palettes.` and change the enum and its impls:

```rust
pub enum ThemeId {
    Claude,
    Github,
    Vscode,
    Nord,
    Gruvbox,
    Studio,
    ClaudeLight,
}

impl ThemeId {
    pub const ALL: [Self; 7] = [
        Self::Claude,
        Self::Github,
        Self::Vscode,
        Self::Nord,
        Self::Gruvbox,
        Self::Studio,
        Self::ClaudeLight,
    ];

    /// Cycle to the next theme in the canonical order:
    /// `Claude -> GitHub -> VSCode -> Nord -> Gruvbox -> Studio -> ClaudeLight -> Claude`.
    #[must_use]
    pub const fn cycle(self) -> Self {
        match self {
            Self::Claude => Self::Github,
            Self::Github => Self::Vscode,
            Self::Vscode => Self::Nord,
            Self::Nord => Self::Gruvbox,
            Self::Gruvbox => Self::Studio,
            Self::Studio => Self::ClaudeLight,
            Self::ClaudeLight => Self::Claude,
        }
    }

    /// Stable snake_case token for serialization and on-disk persistence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Github => "github",
            Self::Vscode => "vscode",
            Self::Nord => "nord",
            Self::Gruvbox => "gruvbox",
            Self::Studio => "studio",
            Self::ClaudeLight => "claude_light",
        }
    }
}

impl Default for ThemeId {
    fn default() -> Self {
        // New profiles use Claude light (ADR 030); saved theme IDs are preserved.
        Self::ClaudeLight
    }
}
```

- [ ] **Step 4: Add and retune the palettes**

In `theme/palette.rs`, add `ThemeId::ClaudeLight => claude_light_palette(),` to `palette_for`, change `all_palettes` to return `[ThemePalette; 7]` with `claude_light_palette()` appended last, update its doc to "All seven palettes in canonical order", and replace `claude_palette` and add `claude_light_palette`:

```rust
fn claude_palette() -> ThemePalette {
    ThemePalette {
        name: "claude".to_string(),
        bg: "#262624".to_string(),
        surface: "#1F1E1D".to_string(),
        surface_alt: "#30302E".to_string(),
        border: "#3A3935".to_string(),
        text_primary: "#F5F4EE".to_string(),
        text_muted: "#A8A59B".to_string(),
        accent: "#D97757".to_string(),
        accent_alt: "#E7C6B4".to_string(),
        success: "#8AB487".to_string(),
        warning: "#D9A85F".to_string(),
        danger: "#E07A6F".to_string(),
        terminal_bg: "#1A1918".to_string(),
        terminal_fg: "#EDEBE4".to_string(),
        terminal_cursor: "#D97757".to_string(),
        font_family: "Cascadia Code".to_string(),
        font_size: 13.0,
        corner_radius: 8.0,
        border_width: 1.0,
        density: Density::Comfortable,
    }
}

fn claude_light_palette() -> ThemePalette {
    ThemePalette {
        name: "claude_light".to_string(),
        bg: "#FAF9F5".to_string(),
        surface: "#F5F4ED".to_string(),
        surface_alt: "#FFFFFF".to_string(),
        border: "#E6E3DA".to_string(),
        text_primary: "#141413".to_string(),
        text_muted: "#6B6960".to_string(),
        accent: "#C96442".to_string(),
        accent_alt: "#F1D8CC".to_string(),
        success: "#3F7A4A".to_string(),
        warning: "#8F6414".to_string(),
        danger: "#B03A2E".to_string(),
        // Dark terminal, following Studio: the derived ANSI colors of the
        // terminal generators are tuned for dark backgrounds.
        terminal_bg: "#1F1E1D".to_string(),
        terminal_fg: "#F5F4EE".to_string(),
        terminal_cursor: "#D97757".to_string(),
        font_family: "Cascadia Code".to_string(),
        font_size: 13.0,
        corner_radius: 8.0,
        border_width: 1.0,
        density: Density::Comfortable,
    }
}
```

Run: `rg -n "ThemeId::" crates/vibemux_frontend/src --glob '!gui/app.rs' --glob '!gui/overview.rs' --glob '!gui/workbench.rs' --glob '!gui/stub_bank.rs'`
Expected: no remaining exhaustive `match` on `ThemeId` besides the three updated above; fix any other that the compiler reports.

- [ ] **Step 5: Run the Rust tests**

Run: `cargo test -p vibemux_frontend --lib theme`
Expected: all theme tests pass.

Run: `cargo test -p vibemux_frontend --test palette_parity`
Expected: FAIL with "config/theme_palettes.json drifted from the compiled-in palettes".

- [ ] **Step 6: Regenerate the exported palette file**

In `config/theme_palettes.json`, replace the values of the `claude` object with:

```json
      "name": "claude",
      "bg": "#262624",
      "surface": "#1F1E1D",
      "surface_alt": "#30302E",
      "border": "#3A3935",
      "text_primary": "#F5F4EE",
      "text_muted": "#A8A59B",
      "accent": "#D97757",
      "accent_alt": "#E7C6B4",
      "success": "#8AB487",
      "warning": "#D9A85F",
      "danger": "#E07A6F",
      "terminal_bg": "#1A1918",
      "terminal_fg": "#EDEBE4",
      "terminal_cursor": "#D97757",
      "font_family": "Cascadia Code",
      "font_size": 13.0,
      "corner_radius": 8.0,
      "border_width": 1.0,
      "density": "comfortable"
```

Append after the `studio` object (add a comma after the `studio` object's closing brace):

```json
    {
      "name": "claude_light",
      "bg": "#FAF9F5",
      "surface": "#F5F4ED",
      "surface_alt": "#FFFFFF",
      "border": "#E6E3DA",
      "text_primary": "#141413",
      "text_muted": "#6B6960",
      "accent": "#C96442",
      "accent_alt": "#F1D8CC",
      "success": "#3F7A4A",
      "warning": "#8F6414",
      "danger": "#B03A2E",
      "terminal_bg": "#1F1E1D",
      "terminal_fg": "#F5F4EE",
      "terminal_cursor": "#D97757",
      "font_family": "Cascadia Code",
      "font_size": 13.0,
      "corner_radius": 8.0,
      "border_width": 1.0,
      "density": "comfortable"
    }
```

Run: `cargo test -p vibemux_frontend --test palette_parity`
Expected: 2 passed.

- [ ] **Step 7: Update the Python fallback and tests (failing first)**

In `tests/test_theme.py`, add `_fallback_palette` to the `from vibemux.theme import (...)` block, change the list assertion in `test_repository_palette_file_loads_six_themes` and rename the test:

```python
def test_repository_palette_file_loads_seven_themes() -> None:
    palettes = load_palettes()
    assert list(palettes) == [
        "claude",
        "github",
        "vscode",
        "nord",
        "gruvbox",
        "studio",
        "claude_light",
    ]
    assert palettes["nord"].accent == "#88C0D0"
    assert palettes["nord"].terminal_bg == "#242933"
    assert palettes["gruvbox"].accent == "#FE8019"
    assert palettes["gruvbox"].terminal_bg == "#1D2021"
    assert palettes["claude_light"].bg == "#FAF9F5"


def test_fallback_palette_matches_the_exported_default() -> None:
    assert _fallback_palette() == load_palettes()[DEFAULT_THEME_NAME]
```

(Add `DEFAULT_THEME_NAME` to the import block if it is not already imported.) In `test_cli_theme_list_lists_every_theme`, change the tuple to `("claude", "github", "vscode", "nord", "gruvbox", "studio", "claude_light")`.

Run: `PYTHONPATH="$PWD/src" ../../../.venv/Scripts/python.exe -m pytest tests/test_theme.py -q`
Expected: FAIL in `test_fallback_palette_matches_the_exported_default`.

- [ ] **Step 8: Update the fallback copy**

In `src/vibemux/theme.py` `_fallback_palette`, replace the color arguments with:

```python
bg = ("#262624",)
surface = ("#1F1E1D",)
surface_alt = ("#30302E",)
border = ("#3A3935",)
text_primary = ("#F5F4EE",)
text_muted = ("#A8A59B",)
accent = ("#D97757",)
accent_alt = ("#E7C6B4",)
success = ("#8AB487",)
warning = ("#D9A85F",)
danger = ("#E07A6F",)
terminal_bg = ("#1A1918",)
terminal_fg = ("#EDEBE4",)
terminal_cursor = ("#D97757",)
```

Run: `PYTHONPATH="$PWD/src" ../../../.venv/Scripts/python.exe -m pytest tests/test_theme.py -q`
Expected: all pass.

- [ ] **Step 9: Commit**

```bash
git add crates/vibemux_frontend/src/theme config/theme_palettes.json src/vibemux/theme.py tests/test_theme.py
git commit -m "feat(frontend): claude_light palette and warm charcoal claude palette

claude_light becomes the default for new profiles. The exported palette file
and the Python fallback copy change in the same commit under the parity test.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: GUI color roles and typography

**Files:**
- Create: `crates/vibemux_frontend/src/gui/typography.rs`
- Modify: `crates/vibemux_frontend/src/gui/mod.rs` (`C.accent_text`, module declaration)
- Modify: `crates/vibemux_frontend/src/gui/design.rs` (`state_text_color`)
- Modify: `crates/vibemux_frontend/src/gui/task_detail.rs` (selected tab text color)
- Modify: `crates/vibemux_frontend/src/gui/theme.rs` (readable hyperlink color)
- Modify: `crates/vibemux_frontend/src/gui/sidebar.rs` (serif brand name, the first use of `display_font`)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs` (use `typography::install_fonts`, delete `install_system_cjk_fallback`)

Every item this task adds is used in this task: the crate builds with `clippy -D warnings`, and unused items in private modules are `dead_code` errors. Later tasks add the size constants they need (`GREETING_SIZE` in Task 6; `TITLE_SIZE`, `TASK_TITLE_SIZE`, `PROSE_SIZE`, `PROSE_LINE_HEIGHT` in Task 9) and `C.light` (Task 5).

**Interfaces:**
- Consumes: `contrast::readable_text_color`, `ThemePalette::is_light`.
- Produces: `C { accent_text: Color32, .. }`; `typography::{SERIF_FAMILY_NAME, serif_family() -> FontFamily, display_font(&egui::Context, f32) -> FontId, build_font_definitions(impl FnMut(&str) -> Option<Vec<u8>>) -> FontDefinitions, install_fonts(&egui::Context)}`; `design::state_text_color(&C, &str) -> Color32`.

- [ ] **Step 1: Write the failing tests**

Create `crates/vibemux_frontend/src/gui/typography.rs` containing only the doc comment, `#![forbid(unsafe_code)]`, and this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fake_fonts(available: &'static [&'static str]) -> impl FnMut(&str) -> Option<Vec<u8>> {
        move |name| available.contains(&name).then(|| name.as_bytes().to_vec())
    }

    #[test]
    fn every_face_found_builds_sans_serif_and_cjk_chains() {
        let definitions =
            build_font_definitions(fake_fonts(&["segoeui.ttf", "georgia.ttf", "msyh.ttc"]));
        let proportional = &definitions.families[&egui::FontFamily::Proportional];
        assert_eq!(proportional[0], "vibemux_ui");
        assert_eq!(proportional[1], "vibemux_system_cjk");
        assert_eq!(definitions.families[&egui::FontFamily::Monospace][1], "vibemux_system_cjk");
        let serif = &definitions.families[&serif_family()];
        assert_eq!(serif[0], "vibemux_serif_face");
        assert!(serif.iter().any(|face| face == "vibemux_system_cjk"));
    }

    #[test]
    fn cambria_is_the_serif_fallback() {
        let definitions = build_font_definitions(fake_fonts(&["cambria.ttc"]));
        assert!(definitions.font_data.contains_key("vibemux_serif_face"));
    }

    #[test]
    fn missing_faces_still_bind_the_serif_family_to_the_sans_chain() {
        let definitions = build_font_definitions(fake_fonts(&[]));
        assert_eq!(
            definitions.families[&serif_family()],
            definitions.families[&egui::FontFamily::Proportional]
        );
    }

    #[test]
    fn display_font_falls_back_until_the_serif_family_is_bound() {
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            assert_eq!(display_font(context, 20.0).family, egui::FontFamily::Proportional);
        });
        context.set_fonts(build_font_definitions(fake_fonts(&[])));
        let _ = context.run(egui::RawInput::default(), |context| {
            assert_eq!(display_font(context, 20.0).family, serif_family());
        });
    }
}
```

In `gui/mod.rs`, add `mod typography;` to the module list and this test module at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use crate::theme::{
        all_palettes,
        contrast::{TEXT_CONTRAST_MINIMUM, contrast_ratio},
    };

    fn rgb(color: eframe::egui::Color32) -> (u8, u8, u8) {
        (color.r(), color.g(), color.b())
    }

    #[test]
    fn accent_text_is_readable_on_every_surface_of_every_palette() {
        for palette in all_palettes() {
            let colors = super::pal(&palette);
            for surface in [colors.bg, colors.surf, colors.raised] {
                assert!(
                    contrast_ratio(rgb(colors.accent_text), rgb(surface)) >= TEXT_CONTRAST_MINIMUM,
                    "{}",
                    palette.name
                );
            }
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p vibemux_frontend --lib -- typography accent_text_is_readable`
Expected: compile errors (`build_font_definitions` not found, no field `accent_text`).

- [ ] **Step 3: Implement typography**

Insert above the test module of `typography.rs`:

```rust
//! System font loading: Segoe UI for the interface, a serif family for
//! display text, and a CJK fallback. Fonts are read once from the Windows
//! font directory; nothing is bundled (ADR 030 §3).

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId};

pub const SERIF_FAMILY_NAME: &str = "vibemux_serif";
const SANS_FONT_KEY: &str = "vibemux_ui";
const SERIF_FONT_KEY: &str = "vibemux_serif_face";
const CJK_FONT_KEY: &str = "vibemux_system_cjk";
const SANS_CANDIDATES: [&str; 1] = ["segoeui.ttf"];
const SERIF_CANDIDATES: [&str; 2] = ["georgia.ttf", "cambria.ttc"];
const CJK_CANDIDATES: [&str; 4] = ["msyh.ttc", "simhei.ttf", "meiryo.ttc", "YuGothM.ttc"];

#[must_use]
pub fn serif_family() -> FontFamily {
    FontFamily::Name(SERIF_FAMILY_NAME.into())
}

/// The serif display font, or the sans font while the serif family is not yet
/// bound (egui panics on an unbound family, and `set_fonts` takes effect on
/// the next pass).
#[must_use]
pub fn display_font(ctx: &egui::Context, size: f32) -> FontId {
    let family = serif_family();
    if ctx.fonts(|fonts| fonts.families().contains(&family)) {
        FontId::new(size, family)
    } else {
        FontId::proportional(size)
    }
}

/// Build font definitions from whichever candidate faces `read_font` returns.
/// Each face is optional; the serif family always exists and ends with the
/// proportional chain, so display text never loses glyphs.
#[must_use]
pub fn build_font_definitions(
    mut read_font: impl FnMut(&str) -> Option<Vec<u8>>,
) -> FontDefinitions {
    let mut first_found =
        |candidates: &[&str]| candidates.iter().find_map(|name| read_font(name));
    let sans = first_found(&SANS_CANDIDATES);
    let serif = first_found(&SERIF_CANDIDATES);
    let cjk = first_found(&CJK_CANDIDATES);

    let mut definitions = FontDefinitions::default();
    if let Some(bytes) = sans {
        definitions
            .font_data
            .insert(SANS_FONT_KEY.into(), FontData::from_owned(bytes).into());
        definitions
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, SANS_FONT_KEY.into());
    }
    if let Some(bytes) = cjk {
        definitions
            .font_data
            .insert(CJK_FONT_KEY.into(), FontData::from_owned(bytes).into());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let chain = definitions.families.entry(family).or_default();
            let position = chain.len().min(1);
            chain.insert(position, CJK_FONT_KEY.into());
        }
    }
    let mut serif_chain = Vec::new();
    if let Some(bytes) = serif {
        definitions
            .font_data
            .insert(SERIF_FONT_KEY.into(), FontData::from_owned(bytes).into());
        serif_chain.push(SERIF_FONT_KEY.to_string());
    }
    serif_chain.extend(
        definitions
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    definitions.families.insert(serif_family(), serif_chain);
    definitions
}

#[cfg(windows)]
pub fn install_fonts(ctx: &egui::Context) {
    use std::{env, fs, path::PathBuf};
    let fonts_dir = env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    ctx.set_fonts(build_font_definitions(|name| fs::read(fonts_dir.join(name)).ok()));
}

#[cfg(not(windows))]
pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(build_font_definitions(|_| None));
}
```

In `supervisor_app.rs`, replace `install_system_cjk_fallback(&cc.egui_ctx);` with `super::typography::install_fonts(&cc.egui_ctx);`, and delete both `install_system_cjk_fallback` functions (the `#[cfg(windows)]` one and the `#[cfg(not(windows))]` one).

In `gui/sidebar.rs`, add `typography` to the `use super::{...}` list and change the brand label to:

```rust
                ui.label(
                    RichText::new("VibeMux")
                        .font(typography::display_font(ui.ctx(), 19.0))
                        .color(c.txt),
                );
```

- [ ] **Step 4: Add the readable accent to `C`**

In `gui/mod.rs`, add to `struct C` after `accent`:

```rust
    /// Accent for text: `accent` blended toward `txt` until it reaches 4.5:1
    /// on `bg`, `surf`, and `raised` (ADR 030 §2).
    pub accent_text: Color32,
```

In `pal`, compute and set it:

```rust
    let channel = |field: &str| p.rgb(field).unwrap_or((0, 0, 0));
    let accent_text = crate::theme::contrast::readable_text_color(
        channel("accent"),
        &[channel("bg"), channel("surface"), channel("surface_alt")],
        channel("text_primary"),
        crate::theme::contrast::TEXT_CONTRAST_MINIMUM,
    );
```

and in the struct literal:

```rust
        accent_text: Color32::from_rgb(accent_text.0, accent_text.1, accent_text.2),
```

- [ ] **Step 5: Use the readable accent for text**

In `gui/design.rs`, add below `state_color`:

```rust
/// Text color for a state label; accent states use the readable accent.
pub fn state_text_color(c: &C, state: &str) -> Color32 {
    match state {
        "in_progress" | "running" => c.accent_text,
        _ => state_color(c, state),
    }
}
```

and in `status`, change the label line to:

```rust
        ui.label(RichText::new(state_text(state)).size(12.0).color(state_text_color(c, state)));
```

In `gui/task_detail.rs`, in the tab buttons change `colors.accent` (the selected text color) to `colors.accent_text`.

In `gui/theme.rs` `build_visuals`, replace `visuals.hyperlink_color = color(&p.accent);` with:

```rust
    let channel = |field: &str| p.rgb(field).unwrap_or((0, 0, 0));
    let link = crate::theme::contrast::readable_text_color(
        channel("accent"),
        &[channel("bg"), channel("surface"), channel("surface_alt")],
        channel("text_primary"),
        crate::theme::contrast::TEXT_CONTRAST_MINIMUM,
    );
    visuals.hyperlink_color = Color32::from_rgb(link.0, link.1, link.2);
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p vibemux_frontend --lib`
Expected: all pass, including the 5 new tests.

- [ ] **Step 7: Commit**

```bash
git add crates/vibemux_frontend/src/gui
git commit -m "feat(frontend): serif display family and readable accent text

Font loading moves to gui/typography.rs; each face is optional, fixing the old
behavior where a missing CJK font also skipped Segoe UI.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Spark mark, icon button, and composer frame, adopted in place

**Files:**
- Modify: `crates/vibemux_frontend/src/gui/mod.rs` (`C.light`)
- Modify: `crates/vibemux_frontend/src/gui/design.rs` (spark, `icon_button`, `composer_frame`, `Icon::Refresh`, delete `avatar`)
- Modify: `crates/vibemux_frontend/src/gui/sidebar.rs` (brand spark)
- Modify: `crates/vibemux_frontend/src/gui/chat.rs` (header spark, composer frame)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs` (Refresh icon button)

Each primitive is used in this task, so `clippy -D warnings` stays clean. Later tasks add the icons and helpers they first use: `Icon::{Plus, SidebarToggle}` (Task 7), `Icon::Send` (Task 8), `CARD_RADIUS` and `paint_bottom_fade` (Task 9), `Icon::Close` (Task 10), `Icon::Search` and `popup_frame` (Task 16), `Icon::{Copy, Check}` (Task 18).

**Interfaces:**
- Produces: `C.light: bool`; `design::{BUTTON_RADIUS: u8 = 8, COMPOSER_RADIUS: u8 = 20, spark_segments(Pos2, f32, f32) -> [(Pos2, Pos2); 8], paint_spark(&Painter, Pos2, f32, f32, Color32), spark(&mut Ui, Color32, f32, f32) -> Response, icon_button(&mut Ui, &C, Icon, &str, egui::Id) -> Response, composer_frame(&C) -> egui::Frame}`, private `shadow_color(&C) -> Color32`; `Icon::Refresh`; `supervisor_app::REFRESH_BUTTON_ID`.

- [ ] **Step 1: Write the failing tests**

Add to the end of `design.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_has_eight_rays_alternating_long_and_short() {
        let center = egui::pos2(50.0, 50.0);
        let segments = spark_segments(center, 10.0, 0.0);
        assert_eq!(segments.len(), 8);
        for (index, (inner, outer)) in segments.iter().enumerate() {
            let expected = if index % 2 == 0 { 10.0 } else { 10.0 * SPARK_SHORT_RAY };
            assert!((outer.distance(center) - expected).abs() < 1e-3);
            assert!((inner.distance(center) - 10.0 * SPARK_INNER_GAP).abs() < 1e-3);
        }
        let rotated = spark_segments(center, 10.0, std::f32::consts::FRAC_PI_2);
        assert!((rotated[0].1.y - 60.0).abs() < 1e-3);
    }
}
```

In the `gui/mod.rs` test `accent_text_is_readable_on_every_surface_of_every_palette`, add inside the palette loop:

```rust
            assert_eq!(colors.light, palette.is_light());
```

Run: `cargo test -p vibemux_frontend --lib -- spark_has_eight_rays accent_text_is_readable`
Expected: compile errors (`spark_segments` not found, no field `light`).

- [ ] **Step 2: Add `C.light`**

In `gui/mod.rs`, add to `struct C` after `danger`:

```rust
    /// True for light palettes (canvas luminance above 0.5).
    pub light: bool,
```

and `light: p.is_light(),` to the literal in `pal`.

- [ ] **Step 3: Implement the primitives**

In `design.rs`, extend the `use egui::{...}` line with `Painter, Pos2`, add `Refresh` to `pub enum Icon`, and add this arm to the `match kind` in `icon`:

```rust
        Icon::Refresh => {
            painter.circle_stroke(rect.center(), rect.width() * 0.36, stroke);
            painter.line_segment([position(0.72, 0.05), position(0.86, 0.18)], stroke);
            painter.line_segment([position(0.86, 0.18), position(0.66, 0.26)], stroke);
        }
```

Add below `icon`:

```rust
pub const COMPOSER_RADIUS: u8 = 20;
pub const BUTTON_RADIUS: u8 = 8;
const ICON_BUTTON_SIZE: f32 = 30.0;
const SPARK_SHORT_RAY: f32 = 0.62;
const SPARK_INNER_GAP: f32 = 0.16;
const SPARK_STROKE: f32 = 0.2;

/// Eight rays of a generic spark, alternating long and short, starting at
/// `angle` radians. It is not a reproduction of any trademarked logo.
#[must_use]
pub fn spark_segments(center: Pos2, radius: f32, angle: f32) -> [(Pos2, Pos2); 8] {
    std::array::from_fn(|index| {
        let direction = angle + index as f32 * std::f32::consts::FRAC_PI_4;
        let unit = vec2(direction.cos(), direction.sin());
        let length = if index % 2 == 0 { radius } else { radius * SPARK_SHORT_RAY };
        (center + unit * radius * SPARK_INNER_GAP, center + unit * length)
    })
}

pub fn paint_spark(painter: &Painter, center: Pos2, radius: f32, angle: f32, color: Color32) {
    let width = (radius * SPARK_STROKE).max(1.2);
    for (inner, outer) in spark_segments(center, radius, angle) {
        painter.line_segment([inner, outer], Stroke::new(width, color));
        painter.circle_filled(outer, width / 2.0, color);
    }
}

pub fn spark(ui: &mut Ui, color: Color32, size: f32, angle: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint_spark(ui.painter(), rect.center(), size / 2.0, angle, color);
    response
}

/// A square icon button with a tooltip and an explicit, stable `id`.
pub fn icon_button(ui: &mut Ui, c: &C, kind: Icon, tooltip: &str, id: egui::Id) -> Response {
    let (_, rect) = ui.allocate_space(vec2(ICON_BUTTON_SIZE, ICON_BUTTON_SIZE));
    let response = ui.interact(rect, id, Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, BUTTON_RADIUS, c.raised);
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(6.0)));
    icon(&mut child, kind, if response.hovered() { c.txt } else { c.muted }, 18.0);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tooltip));
    response.on_hover_text(tooltip)
}

fn shadow_color(c: &C) -> Color32 {
    if c.light {
        Color32::from_black_alpha(18)
    } else {
        Color32::from_black_alpha(64)
    }
}

pub fn composer_frame(c: &C) -> egui::Frame {
    egui::Frame::new()
        .fill(c.raised)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(COMPOSER_RADIUS)
        .inner_margin(egui::Margin::symmetric(18, 14))
        .shadow(egui::Shadow {
            offset: [0, 2],
            blur: 12,
            spread: 0,
            color: shadow_color(c),
        })
}
```

- [ ] **Step 4: Adopt them and delete the letter avatar**

In `sidebar.rs`, replace `design::avatar(ui, c, "v", 28.0);` with `design::spark(ui, c.accent, 22.0, 0.0);`.

In `chat.rs` `render_conversation`, replace `design::avatar(ui,c,"v",34.0);` with `design::spark(ui, c.accent, 26.0, 0.0);`, and in `render_composer` replace the frame builder chain (`egui::Frame::new().fill(c.raised).stroke(...).corner_radius(14).inner_margin(egui::Margin::same(16))`) with `design::composer_frame(c)`.

Delete `pub fn avatar` from `design.rs`.

In `supervisor_app.rs`, add `pub(crate) const REFRESH_BUTTON_ID: &str = "topbar_refresh";`, add `design` to the `use super::{...}` list, and in `render_topbar` replace

```rust
                        if ui.button("Refresh").clicked() {
                            self.enqueue_refresh();
                        }
```

with

```rust
                        let refresh = design::icon_button(
                            ui,
                            colors,
                            design::Icon::Refresh,
                            "Refresh",
                            egui::Id::new(REFRESH_BUTTON_ID),
                        );
                        if refresh.clicked() {
                            self.enqueue_refresh();
                        }
```

- [ ] **Step 5: Run the tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/vibemux_frontend/src/gui
git commit -m "feat(frontend): spark mark, icon button, and soft composer frame

Replaces the letter avatar with a generic spark and the Refresh text button
with an icon button.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: New task flow and welcome state

**Files:**
- Create: `crates/vibemux_frontend/src/gui/welcome.rs`
- Modify: `crates/vibemux_frontend/src/gui/mod.rs` (`mod welcome;`, re-export `NewTaskOutcome`)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_state.rs`
- Modify: `crates/vibemux_frontend/src/gui/chat.rs` (focus request, `content_column` visibility)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs` (welcome layout, discard modal, `request_new_task`)
- Modify: `crates/vibemux_frontend/Cargo.toml` (time `local-offset`)
- Test: `crates/vibemux_frontend/tests/supervisor_ui.rs`

**Interfaces:**
- Consumes: `design::spark`, `typography::display_font`.
- Produces: `typography::GREETING_SIZE: f32 = 30.0`; `NewTaskOutcome { Started, NeedsConfirmation }`; `SupervisorUiState::{welcome_active(&self) -> bool, request_new_task(&mut self) -> NewTaskOutcome, discard_prompt_open(&self) -> bool, confirm_discard_draft(&mut self), keep_draft(&mut self), take_composer_focus_request(&mut self) -> bool}`; `welcome::{greeting_for_hour(Option<u8>) -> &'static str, current_local_hour() -> Option<u8>, render_heading(&mut Ui, &C, &str)}`; `SupervisorApp::request_new_task(&mut self)`; `chat::content_column` becomes `pub(crate)`.

- [ ] **Step 1: Write the failing state tests**

Add to `crates/vibemux_frontend/tests/supervisor_ui.rs` (extend the `gui::{...}` import with `NewTaskOutcome`):

```rust
#[test]
fn new_task_on_an_empty_draft_starts_the_welcome_state() {
    let mut state = SupervisorUiState::default();
    assert_eq!(state.request_new_task(), NewTaskOutcome::Started);
    assert!(state.welcome_active());
    assert!(state.take_composer_focus_request());
    assert!(!state.take_composer_focus_request());
}

#[test]
fn whitespace_draft_starts_a_new_task_without_prompting() {
    let mut state = SupervisorUiState::default();
    state.set_composer_draft("  \n\t".to_string());
    assert_eq!(state.request_new_task(), NewTaskOutcome::Started);
    assert!(!state.discard_prompt_open());
    assert_eq!(state.composer_draft(), "");
}

#[test]
fn nonempty_draft_needs_confirmation_and_keep_preserves_it() {
    let mut state = SupervisorUiState::default();
    state.set_composer_draft("请检查 draft".to_string());
    assert_eq!(state.request_new_task(), NewTaskOutcome::NeedsConfirmation);
    assert!(state.discard_prompt_open());
    assert!(!state.welcome_active());
    state.keep_draft();
    assert!(!state.discard_prompt_open());
    assert_eq!(state.composer_draft(), "请检查 draft");
    assert!(!state.try_send());
}

#[test]
fn confirming_discard_clears_the_draft_and_starts_the_welcome_state() {
    let mut state = SupervisorUiState::default();
    state.set_composer_draft("draft".to_string());
    let _ = state.request_new_task();
    state.confirm_discard_draft();
    assert_eq!(state.composer_draft(), "");
    assert!(state.welcome_active());
    assert!(!state.discard_prompt_open());
}

#[test]
fn navigation_leaves_the_welcome_state() {
    let snapshot = snapshot(vec![task("task_1", "First")]);
    let mut state = SupervisorUiState::default();
    let _ = state.request_new_task();
    state.show_agents();
    assert!(!state.welcome_active());
    let _ = state.request_new_task();
    state.show_coordinator_chat();
    assert!(!state.welcome_active());
    let _ = state.request_new_task();
    assert!(state.select_task(&snapshot, "task_1"));
    assert!(!state.welcome_active());
}
```

Run: `cargo test -p vibemux_frontend --test supervisor_ui`
Expected: compile error (`NewTaskOutcome` not found).

- [ ] **Step 2: Implement the state**

In `supervisor_state.rs`, add above `SupervisorUiState`:

```rust
/// Result of asking for a new task (ADR 030 §5).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewTaskOutcome {
    Started,
    NeedsConfirmation,
}
```

Add fields to `SupervisorUiState`:

```rust
    welcome_active: bool,
    discard_prompt_open: bool,
    composer_focus_requested: bool,
```

Add methods to `impl SupervisorUiState`:

```rust
    #[must_use]
    pub fn welcome_active(&self) -> bool {
        self.welcome_active
    }

    /// Start a new task, or ask first when the draft holds visible text.
    pub fn request_new_task(&mut self) -> NewTaskOutcome {
        if self.composer_draft.trim().is_empty() {
            self.start_new_task();
            NewTaskOutcome::Started
        } else {
            self.discard_prompt_open = true;
            NewTaskOutcome::NeedsConfirmation
        }
    }

    #[must_use]
    pub fn discard_prompt_open(&self) -> bool {
        self.discard_prompt_open
    }

    pub fn confirm_discard_draft(&mut self) {
        self.discard_prompt_open = false;
        self.start_new_task();
    }

    pub fn keep_draft(&mut self) {
        self.discard_prompt_open = false;
    }

    /// True once after a new task starts, so the composer takes focus once.
    pub fn take_composer_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.composer_focus_requested)
    }

    fn start_new_task(&mut self) {
        self.composer_draft.clear();
        self.page = MainPage::CoordinatorChat;
        self.selected_agent = None;
        self.close_details();
        self.welcome_active = true;
        self.composer_focus_requested = true;
    }
```

Set `self.welcome_active = false;` as the first statement of `show_coordinator_chat`, `show_agents`, `select_agent` (inside the success path, before `true`), `return_to_agents_list`, and `select_task` (on the path that returns `true`).

In `gui/mod.rs`, extend the `pub use supervisor_state::{...}` list with `NewTaskOutcome`, and add `mod welcome;`.

Run: `cargo test -p vibemux_frontend --test supervisor_ui`
Expected: all pass.

- [ ] **Step 3: Write the failing greeting test**

Create `crates/vibemux_frontend/src/gui/welcome.rs`:

```rust
#![forbid(unsafe_code)]
//! Welcome state: the spark mark and a time-of-day serif greeting above the
//! composer. It never shows a user name (ADR 030 §4).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_changes_at_five_twelve_and_eighteen() {
        assert_eq!(greeting_for_hour(Some(4)), "Good evening");
        assert_eq!(greeting_for_hour(Some(5)), "Good morning");
        assert_eq!(greeting_for_hour(Some(11)), "Good morning");
        assert_eq!(greeting_for_hour(Some(12)), "Good afternoon");
        assert_eq!(greeting_for_hour(Some(17)), "Good afternoon");
        assert_eq!(greeting_for_hour(Some(18)), "Good evening");
        assert_eq!(greeting_for_hour(Some(23)), "Good evening");
        assert_eq!(greeting_for_hour(Some(0)), "Good evening");
    }

    #[test]
    fn unknown_or_invalid_hour_says_hello() {
        assert_eq!(greeting_for_hour(None), "Hello");
        assert_eq!(greeting_for_hour(Some(24)), "Hello");
    }
}
```

Run: `cargo test -p vibemux_frontend --lib greeting`
Expected: compile error (`greeting_for_hour` not found).

- [ ] **Step 4: Implement the greeting and heading**

In `gui/typography.rs`, add below `CJK_CANDIDATES`:

```rust
pub const GREETING_SIZE: f32 = 30.0;
```

In `crates/vibemux_frontend/Cargo.toml`, replace `time.workspace = true` with:

```toml
time = { workspace = true, features = ["local-offset"] }
```

Insert above the test module of `welcome.rs`:

```rust
use eframe::egui::{RichText, Ui};

use super::{C, design, typography};

pub const MORNING_START_HOUR: u8 = 5;
pub const AFTERNOON_START_HOUR: u8 = 12;
pub const EVENING_START_HOUR: u8 = 18;

#[must_use]
pub fn greeting_for_hour(local_hour: Option<u8>) -> &'static str {
    match local_hour {
        Some(hour) if (MORNING_START_HOUR..AFTERNOON_START_HOUR).contains(&hour) => "Good morning",
        Some(hour) if (AFTERNOON_START_HOUR..EVENING_START_HOUR).contains(&hour) => {
            "Good afternoon"
        }
        Some(hour) if hour < 24 => "Good evening",
        _ => "Hello",
    }
}

/// The local hour, or `None` when the local offset is unavailable.
#[must_use]
pub fn current_local_hour() -> Option<u8> {
    time::OffsetDateTime::now_local().ok().map(|now| now.hour())
}

pub fn render_heading(ui: &mut Ui, c: &C, greeting: &str) {
    ui.vertical_centered(|ui| {
        design::spark(ui, c.accent, 34.0, 0.0);
        ui.add_space(10.0);
        ui.label(
            RichText::new(greeting)
                .font(typography::display_font(ui.ctx(), typography::GREETING_SIZE))
                .color(c.txt),
        );
        ui.add_space(22.0);
    });
}
```

Run: `cargo test -p vibemux_frontend --lib greeting`
Expected: 2 passed.

- [ ] **Step 5: Wire the welcome layout, focus request, and discard modal**

In `chat.rs`, change `fn content_column` to `pub(crate) fn content_column`, and inside `render_composer`, directly after `let response = ui.add(...)` for the text edit, add:

```rust
                if state.take_composer_focus_request() {
                    response.request_focus();
                }
```

In `supervisor_app.rs`, add the public method to `impl SupervisorApp`:

```rust
    /// Start a new task (sidebar button, `Ctrl+N`, quick switcher, preview).
    pub fn request_new_task(&mut self) {
        if let Ok(mut state) = self.ui_state.lock() {
            let _ = state.request_new_task();
        }
    }
```

In `render_frame`, after `let page = ...` (the second one, before the composer panel), compute:

```rust
        let welcome = page == MainPage::CoordinatorChat
            && (self.ui_state.lock().is_ok_and(|state| state.welcome_active())
                || snapshot.tasks.is_empty());
```

Change the bottom composer condition from `if page == MainPage::CoordinatorChat {` to `if page == MainPage::CoordinatorChat && !welcome {`, and replace the `MainPage::CoordinatorChat => { ... }` arm of the central panel match with:

```rust
                MainPage::CoordinatorChat if welcome => {
                    let top_space = (ui.available_height() * 0.22).max(24.0);
                    ui.add_space(top_space);
                    chat::content_column(ui, |ui| {
                        super::welcome::render_heading(
                            ui,
                            &colors,
                            super::welcome::greeting_for_hour(super::welcome::current_local_hour()),
                        );
                    });
                    if let Ok(mut state) = self.ui_state.lock() {
                        chat::render_composer(ui, &colors, &mut state);
                    }
                }
                MainPage::CoordinatorChat => {
                    if let Ok(mut state) = self.ui_state.lock() {
                        chat::render_conversation(
                            ui,
                            ctx,
                            &colors,
                            &snapshot,
                            &mut state,
                            &self.actions,
                        );
                    }
                }
```

After the diagnostics block in `render_frame`, add the discard prompt:

```rust
        if self.ui_state.lock().is_ok_and(|state| state.discard_prompt_open()) {
            let modal = egui::Modal::new(egui::Id::new("discard_draft_prompt")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.label(RichText::new("Discard current draft?").size(16.0).color(colors.txt));
                ui.add_space(6.0);
                ui.label(
                    RichText::new("The draft has not been sent and will be removed.")
                        .size(13.0)
                        .color(colors.muted),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let discard = ui.button("Discard").clicked();
                    let keep = ui.button("Keep editing").clicked();
                    (discard, keep)
                })
                .inner
            });
            let (discard, keep) = modal.inner;
            if let Ok(mut state) = self.ui_state.lock() {
                if discard {
                    state.confirm_discard_draft();
                } else if keep || modal.should_close() {
                    state.keep_draft();
                }
            }
        }
```

In `handle_keyboard`, inside the `Escape` branch, add as the first check:

```rust
            if let Ok(mut state) = self.ui_state.lock() {
                if state.discard_prompt_open() {
                    state.keep_draft();
                    return;
                }
            }
```

- [ ] **Step 6: Add the welcome render test**

Add to the `supervisor_app.rs` test module:

```rust
    #[test]
    fn empty_workspace_renders_the_welcome_composer() {
        let mut app = test_app();
        app.snapshot.write().unwrap().tasks.clear();
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = context.run(raw.clone(), |context| app.render_frame(context));
        let output = context.run(raw, |context| app.render_frame(context));
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|text| text.starts_with("Good ") || text == "Hello"));
    }
```

- [ ] **Step 7: Run all frontend tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/vibemux_frontend
git commit -m "feat(frontend): New task flow with discard confirmation and welcome state

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Sidebar shell with collapse, Recents, and workspace menu

**Files:**
- Modify: `crates/vibemux_frontend/src/gui/sidebar.rs` (rewrite)
- Modify: `crates/vibemux_frontend/src/theme/serialize.rs` (`sidebar_collapsed`)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs` (panel width, actions, `set_sidebar_collapsed`)

**Interfaces:**
- Consumes: `design::{spark, icon_button, nav_row, state_color, BUTTON_RADIUS}`, `typography::display_font`, `SupervisorApp::request_new_task`.
- Produces: `sidebar::{SIDEBAR_WIDTH = 260.0, SIDEBAR_RAIL_WIDTH = 56.0, SIDEBAR_PANEL_ID, NEW_TASK_BUTTON_ID, TOGGLE_BUTTON_ID, WORKSPACE_BUTTON_ID, SidebarView<'a>, SidebarActions, width_for(bool) -> f32, render(&mut Ui, &C, &SidebarView) -> SidebarActions}`; `UserConfig.sidebar_collapsed: bool`; `SupervisorApp::set_sidebar_collapsed(&mut self, bool)`.

- [ ] **Step 1: Write the failing config test**

Add to the `serialize.rs` test module:

```rust
    #[test]
    fn config_without_new_fields_keeps_theme() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("frontend.json");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"theme":"claude","window_size":{"width":1280,"height":800}}"#,
        )
        .expect("write");
        let config = load_user_config_at(&path);
        assert_eq!(config.theme, ThemeId::Claude);
        assert!(!config.sidebar_collapsed);
    }

    #[test]
    fn sidebar_collapsed_round_trips() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("frontend.json");
        let config = UserConfig {
            sidebar_collapsed: true,
            ..UserConfig::default()
        };
        save_user_config_at(&config, &path).expect("save");
        assert!(load_user_config_at(&path).sidebar_collapsed);
    }
```

Run: `cargo test -p vibemux_frontend --lib serialize`
Expected: compile error (no field `sidebar_collapsed`).

- [ ] **Step 2: Add the config field**

In `UserConfig`, add after `window_size`:

```rust
    /// Sidebar collapsed to its icon rail (ADR 030 §7).
    #[serde(default)]
    pub sidebar_collapsed: bool,
```

Set `sidebar_collapsed: false,` in `impl Default for UserConfig`. Every other `UserConfig { ... }` literal in the crate (the `serialize.rs` tests and `config()` in `supervisor_app.rs` tests) gets `..UserConfig::default()` as its last line; find them with `rg -n "UserConfig \{" crates/vibemux_frontend/src`.

Run: `cargo test -p vibemux_frontend --lib serialize`
Expected: all pass.

- [ ] **Step 3: Write the failing sidebar render tests**

Add to the `supervisor_app.rs` test module:

```rust
    fn render_at(app: &mut SupervisorApp, width: f32, height: f32) -> (egui::Context, egui::FullOutput) {
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height))),
            ..Default::default()
        };
        let _ = context.run(raw.clone(), |context| app.render_frame(context));
        let output = context.run(raw, |context| app.render_frame(context));
        (context, output)
    }

    #[test]
    fn sidebar_width_follows_the_collapsed_setting() {
        for (collapsed, expected) in [(false, sidebar::SIDEBAR_WIDTH), (true, sidebar::SIDEBAR_RAIL_WIDTH)] {
            let mut app = test_app();
            app.user_config.sidebar_collapsed = collapsed;
            let (context, _) = render_at(&mut app, 1280.0, 800.0);
            let panel = egui::containers::panel::PanelState::load(
                &context,
                egui::Id::new(sidebar::SIDEBAR_PANEL_ID),
            )
            .expect("sidebar panel");
            assert!((panel.rect.width() - expected).abs() < 1.0, "{collapsed}");
        }
    }

    #[test]
    fn recents_truncate_long_cjk_titles_inside_the_sidebar() {
        let long_title = "很长的任务标题需要截断".repeat(8);
        let mut app = test_app();
        app.snapshot.write().unwrap().tasks[0].title = long_title.clone();
        let (_, output) = render_at(&mut app, 1280.0, 800.0);
        let sidebar_titles: Vec<egui::Rect> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.job.text == long_title && text.pos.x < sidebar::SIDEBAR_WIDTH =>
                {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            .collect();
        assert_eq!(sidebar_titles.len(), 1);
        assert!(sidebar_titles[0].right() <= sidebar::SIDEBAR_WIDTH + 0.5);
    }
```

Run: `cargo test -p vibemux_frontend --lib sidebar_width recents_truncate`
Expected: compile errors (`SIDEBAR_PANEL_ID`, `SIDEBAR_RAIL_WIDTH` not found).

- [ ] **Step 4: Rewrite the sidebar**

In `design.rs`, add `Plus` and `SidebarToggle` to `pub enum Icon` and these arms to the `match kind` in `icon`:

```rust
        Icon::Plus => {
            painter.line_segment([position(0.5, 0.15), position(0.5, 0.85)], stroke);
            painter.line_segment([position(0.15, 0.5), position(0.85, 0.5)], stroke);
        }
        Icon::SidebarToggle => {
            painter.rect_stroke(rect, 3, stroke, egui::StrokeKind::Inside);
            painter.line_segment([position(0.38, 0.0), position(0.38, 1.0)], stroke);
        }
```

Replace the whole content of `crates/vibemux_frontend/src/gui/sidebar.rs` with:

```rust
#![forbid(unsafe_code)]
//! Claude Desktop style sidebar: brand row, New task, navigation, Recents,
//! and a workspace menu. Collapses to a 56 px icon rail (ADR 030 §4).
use super::{
    C,
    design::{self, Icon},
    supervisor_state::MainPage,
    typography,
};
use crate::supervisor_model::SupervisorSnapshot;
use egui::{self, RichText, Sense, Ui, vec2};

pub const SIDEBAR_WIDTH: f32 = 260.0;
pub const SIDEBAR_RAIL_WIDTH: f32 = 56.0;
pub const SIDEBAR_PANEL_ID: &str = "supervisor_sidebar";
pub const NEW_TASK_BUTTON_ID: &str = "sidebar_new_task";
pub const TOGGLE_BUTTON_ID: &str = "sidebar_toggle";
pub const WORKSPACE_BUTTON_ID: &str = "sidebar_workspace";
const ROW_HEIGHT: f32 = 34.0;
const RECENT_ROW_HEIGHT: f32 = 30.0;

pub struct SidebarView<'a> {
    pub snapshot: &'a SupervisorSnapshot,
    pub page: MainPage,
    pub welcome_active: bool,
    pub collapsed: bool,
    pub selected_task_id: Option<&'a str>,
}

#[derive(Default)]
pub struct SidebarActions {
    pub page: Option<MainPage>,
    pub open_settings: bool,
    pub open_diagnostics: bool,
    pub task_id: Option<String>,
    pub new_task: bool,
    pub toggle_collapsed: bool,
}

#[must_use]
pub const fn width_for(collapsed: bool) -> f32 {
    if collapsed {
        SIDEBAR_RAIL_WIDTH
    } else {
        SIDEBAR_WIDTH
    }
}

pub fn render(ui: &mut Ui, c: &C, view: &SidebarView<'_>) -> SidebarActions {
    let mut actions = SidebarActions::default();
    if view.collapsed {
        render_rail(ui, c, &mut actions);
    } else {
        render_expanded(ui, c, view, &mut actions);
    }
    actions
}

fn render_expanded(ui: &mut Ui, c: &C, view: &SidebarView<'_>, actions: &mut SidebarActions) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 14))
        .show(ui, |ui| {
            ui.set_min_width(SIDEBAR_WIDTH - 24.0);
            egui::TopBottomPanel::bottom("sidebar_workspace_panel")
                .frame(egui::Frame::NONE)
                .show_separator_line(false)
                .show_inside(ui, |ui| {
                    ui.add_space(8.0);
                    workspace_button(ui, c, view.snapshot, actions);
                });
            ui.horizontal(|ui| {
                design::spark(ui, c.accent, 22.0, 0.0);
                ui.add_space(4.0);
                ui.label(
                    RichText::new("VibeMux")
                        .font(typography::display_font(ui.ctx(), 19.0))
                        .color(c.txt),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let toggle = design::icon_button(
                        ui,
                        c,
                        Icon::SidebarToggle,
                        "Collapse sidebar",
                        egui::Id::new(TOGGLE_BUTTON_ID),
                    );
                    if toggle.clicked() {
                        actions.toggle_collapsed = true;
                    }
                });
            });
            ui.add_space(14.0);
            if new_task_row(ui, c) {
                actions.new_task = true;
            }
            ui.add_space(4.0);
            let conversation = view.page == MainPage::CoordinatorChat && !view.welcome_active;
            if design::nav_row(ui, c, "Conversation", Icon::Chat, conversation) {
                actions.page = Some(MainPage::CoordinatorChat);
            }
            if design::nav_row(ui, c, "Agents", Icon::Grid, view.page == MainPage::Agents) {
                actions.page = Some(MainPage::Agents);
            }
            ui.add_space(18.0);
            ui.label(RichText::new("Recents").size(12.0).color(c.muted));
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("sidebar_recents")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if view.snapshot.tasks.is_empty() {
                        ui.label(
                            RichText::new("Tasks appear here when assigned.")
                                .size(12.0)
                                .color(c.muted),
                        );
                    }
                    for task in &view.snapshot.tasks {
                        let selected = view.selected_task_id == Some(task.task_id.as_str());
                        if recent_row(ui, c, &task.title, &task.state, selected).clicked() {
                            actions.task_id = Some(task.task_id.clone());
                        }
                    }
                });
        });
}

fn render_rail(ui: &mut Ui, c: &C, actions: &mut SidebarActions) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(0, 14))
        .show(ui, |ui| {
            ui.set_min_width(SIDEBAR_RAIL_WIDTH);
            ui.vertical_centered(|ui| {
                let buttons = [
                    (Icon::SidebarToggle, "Expand sidebar", TOGGLE_BUTTON_ID),
                    (Icon::Plus, "New task", NEW_TASK_BUTTON_ID),
                    (Icon::Chat, "Conversation", "sidebar_rail_conversation"),
                    (Icon::Grid, "Agents", "sidebar_rail_agents"),
                    (Icon::Settings, "Settings", "sidebar_rail_settings"),
                ];
                for (kind, tooltip, id) in buttons {
                    if design::icon_button(ui, c, kind, tooltip, egui::Id::new(id)).clicked() {
                        match kind {
                            Icon::SidebarToggle => actions.toggle_collapsed = true,
                            Icon::Plus => actions.new_task = true,
                            Icon::Chat => actions.page = Some(MainPage::CoordinatorChat),
                            Icon::Grid => actions.page = Some(MainPage::Agents),
                            _ => actions.open_settings = true,
                        }
                    }
                    ui.add_space(6.0);
                }
            });
        });
}

fn new_task_row(ui: &mut Ui, c: &C) -> bool {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW_HEIGHT));
    let response = ui.interact(rect, egui::Id::new(NEW_TASK_BUTTON_ID), Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, design::BUTTON_RADIUS, c.raised);
    }
    let circle = egui::pos2(rect.left() + 18.0, rect.center().y);
    ui.painter().circle_filled(circle, 10.0, c.accent);
    let plus_color = if c.light { egui::Color32::WHITE } else { c.bg };
    let plus = egui::Stroke::new(1.6, plus_color);
    ui.painter().line_segment([circle - vec2(4.5, 0.0), circle + vec2(4.5, 0.0)], plus);
    ui.painter().line_segment([circle - vec2(0.0, 4.5), circle + vec2(0.0, 4.5)], plus);
    ui.painter().text(
        egui::pos2(rect.left() + 38.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "New task",
        egui::FontId::proportional(14.0),
        c.txt,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "New task"));
    response.clicked()
}

fn recent_row(ui: &mut Ui, c: &C, title: &str, state: &str, selected: bool) -> egui::Response {
    let title = if title.trim().is_empty() { "Untitled task" } else { title };
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), RECENT_ROW_HEIGHT), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            design::BUTTON_RADIUS,
            if selected { c.accent_bg } else { c.raised },
        );
    }
    if matches!(state, "in_progress" | "running" | "blocked" | "input_required") {
        ui.painter().circle_filled(
            egui::pos2(rect.left() + 9.0, rect.center().y),
            3.0,
            design::state_color(c, state),
        );
    }
    let text_left = rect.left() + 20.0;
    let galley = egui::WidgetText::from(
        RichText::new(title)
            .size(13.0)
            .color(if selected { c.txt } else { c.muted }),
    )
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.right() - 6.0 - text_left).max(0.0),
        egui::TextStyle::Button,
    );
    ui.painter().galley(
        egui::pos2(text_left, rect.center().y - galley.size().y / 2.0),
        galley,
        c.muted,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    response.on_hover_text(title)
}

fn workspace_button(ui: &mut Ui, c: &C, snapshot: &SupervisorSnapshot, actions: &mut SidebarActions) {
    let name = if snapshot.project_name.trim().is_empty() {
        "Local workspace"
    } else {
        snapshot.project_name.as_str()
    };
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 42.0));
    let response = ui.interact(rect, egui::Id::new(WORKSPACE_BUTTON_ID), Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, design::BUTTON_RADIUS, c.raised);
    }
    let initial: String = name.chars().next().map_or_else(|| "V".to_string(), |first| first.to_uppercase().collect());
    let badge = egui::pos2(rect.left() + 18.0, rect.center().y);
    ui.painter().circle_filled(badge, 13.0, c.accent_bg);
    ui.painter().text(badge, egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(13.0), c.accent_text);
    let text_left = rect.left() + 40.0;
    ui.painter().text(egui::pos2(text_left, rect.center().y - 7.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(13.0), c.txt);
    ui.painter().text(egui::pos2(text_left, rect.center().y + 9.0), egui::Align2::LEFT_CENTER, "Local workspace", egui::FontId::proportional(11.0), c.muted);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Workspace menu"));
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(200.0);
        if ui.button("Settings").clicked() {
            actions.open_settings = true;
        }
        if ui.button("Diagnostics").clicked() {
            actions.open_diagnostics = true;
        }
    });
}
```

- [ ] **Step 5: Wire the sidebar in the app**

In `supervisor_app.rs` add to `impl SupervisorApp`:

```rust
    /// Collapse or expand the sidebar and persist the choice.
    pub fn set_sidebar_collapsed(&mut self, collapsed: bool) {
        if self.user_config.sidebar_collapsed != collapsed {
            self.user_config.sidebar_collapsed = collapsed;
            self.pending_write = Some(Instant::now());
        }
    }
```

Replace the sidebar block of `render_frame` (from `let mut sidebar_actions = None;` through the end of `if let Some(actions) = sidebar_actions { ... }`) with:

```rust
        let (welcome_active, selected_task_id) = self.ui_state.lock().map_or((false, None), |state| {
            (state.welcome_active(), state.selected_task_id().map(str::to_owned))
        });
        let collapsed = self.user_config.sidebar_collapsed;
        let mut sidebar_actions = None;
        egui::SidePanel::left(sidebar::SIDEBAR_PANEL_ID)
            .exact_width(sidebar::width_for(collapsed))
            .resizable(false)
            .frame(egui::Frame::new().fill(colors.surf))
            .show(ctx, |ui| {
                let view = sidebar::SidebarView {
                    snapshot: &snapshot,
                    page: active_page,
                    welcome_active,
                    collapsed,
                    selected_task_id: selected_task_id.as_deref(),
                };
                sidebar_actions = Some(sidebar::render(ui, &colors, &view));
            });
        if let Some(actions) = sidebar_actions {
            if let Some(page) = actions.page {
                if let Ok(mut state) = self.ui_state.lock() {
                    match page {
                        MainPage::CoordinatorChat => state.show_coordinator_chat(),
                        MainPage::Agents => state.show_agents(),
                    }
                }
            }
            if let Some(task_id) = actions.task_id {
                self.select_task(&task_id);
            }
            if actions.new_task {
                self.request_new_task();
            }
            if actions.toggle_collapsed {
                self.set_sidebar_collapsed(!collapsed);
            }
            if actions.open_settings {
                self.settings_open = true;
            }
            if actions.open_diagnostics {
                self.diagnostics_open = true;
            }
        }
```

- [ ] **Step 6: Run the tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass, including `sidebar_width_follows_the_collapsed_setting` and `recents_truncate_long_cjk_titles_inside_the_sidebar`.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean. If `design::avatar` is now unused only by the sidebar, it is still used by `chat.rs` until Task 9.

- [ ] **Step 7: Commit**

```bash
git add crates/vibemux_frontend
git commit -m "feat(frontend): collapsible sidebar with New task, Recents, and workspace menu

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Composer with target chip and Enter handling

**Files:**
- Create: `crates/vibemux_frontend/src/gui/composer.rs`
- Modify: `crates/vibemux_frontend/src/gui/mod.rs` (`mod composer;`, re-export `ComposerTarget`)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_state.rs` (target and not-sent hint)
- Modify: `crates/vibemux_frontend/src/gui/chat.rs` (delete `render_composer`)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs` (use `composer::render`, update the render test)

**Interfaces:**
- Consumes: `design::{composer_frame, icon, Icon}`, `SupervisorUiState::{composer_draft, set_composer_draft, send_enabled, send_disabled_reason, try_send, take_composer_focus_request}`, `chat::content_column`.
- Produces: `ComposerTarget { Coordinator, Harness(String) }`; `SupervisorUiState::{composer_target(&self) -> &ComposerTarget, set_composer_target(&mut self, ComposerTarget), show_not_sent_hint(&mut self, f64), not_sent_hint_visible(&self, f64) -> bool}`; `NOT_SENT_HINT_SECONDS: f64 = 2.0`; `composer::{COMPOSER_TEXT_ID, SEND_BUTTON_ID, COMPOSER_PLACEHOLDER, TargetOption, target_options(&ViewModel) -> Vec<TargetOption>, caption_text(&ComposerTarget) -> String, render(&mut Ui, &C, &mut SupervisorUiState, &[TargetOption])}`.

- [ ] **Step 1: Write the failing unit tests**

Create `crates/vibemux_frontend/src/gui/composer.rs` with the header and tests:

```rust
#![forbid(unsafe_code)]
//! Claude-style composer: rounded box, target chip, circular send button.
//! Sending stays unavailable (ADR 028); Enter attempts to send and never
//! inserts a newline, Shift+Enter does (ADR 030 §4).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_model::{AgentView, Health, ViewModel};
    use vibemux_probe::{ProbeState, RouteKind};

    fn agent(name: &str, launcher_state: ProbeState) -> AgentView {
        AgentView {
            name: name.to_string(),
            launcher_state,
            authentication_state: ProbeState::NotRun,
            inference_state: ProbeState::NotRun,
            version: "-".to_string(),
            route: RouteKind::Unknown,
            code: "test".to_string(),
        }
    }

    fn view_model(agents: Vec<AgentView>) -> ViewModel {
        ViewModel {
            title: "VibeMux".to_string(),
            platform: "test".to_string(),
            schema_version: 1,
            observed_at_epoch_seconds: 0,
            observed_at: "1970-01-01T00:00:00Z".to_string(),
            health: Health::Warning,
            overall_status: "test".to_string(),
            agents,
            ..ViewModel::default()
        }
    }

    #[test]
    fn coordinator_comes_first_and_undetected_harnesses_are_disabled() {
        let options = target_options(&view_model(vec![
            agent("Codex", ProbeState::Verified),
            agent("Grok", ProbeState::Unavailable),
        ]));
        assert_eq!(options[0].target, ComposerTarget::Coordinator);
        assert!(options[0].enabled);
        assert!(options[1].enabled);
        assert!(!options[2].enabled);
        assert_eq!(options[2].reason.as_deref(), Some("Not detected on this machine"));
    }

    #[test]
    fn caption_always_says_the_draft_is_not_sent() {
        assert_eq!(
            caption_text(&ComposerTarget::Coordinator),
            "Draft only · coordinator chat is not connected"
        );
        assert_eq!(
            caption_text(&ComposerTarget::Harness("Codex".to_string())),
            "Draft for Codex · coordinator chat is not connected"
        );
    }
}
```

If `ViewModel` has no `Default` implementation, replace `..ViewModel::default()` by listing its remaining fields exactly as `test_view_model()` in `supervisor_app.rs` does (read that function and copy its field list).

Run: `cargo test -p vibemux_frontend --lib composer`
Expected: compile errors (`target_options`, `ComposerTarget` not found).

- [ ] **Step 2: Add the state pieces**

In `supervisor_state.rs`, add:

```rust
/// How long "Not sent" stays visible after an Enter or click attempt.
pub const NOT_SENT_HINT_SECONDS: f64 = 2.0;

/// Who a draft is addressed to. Only changes the caption; never enables Send.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ComposerTarget {
    #[default]
    Coordinator,
    Harness(String),
}
```

Add fields `composer_target: ComposerTarget,` and `not_sent_hint_started: Option<f64>,` to `SupervisorUiState`, and methods:

```rust
    #[must_use]
    pub fn composer_target(&self) -> &ComposerTarget {
        &self.composer_target
    }

    pub fn set_composer_target(&mut self, target: ComposerTarget) {
        self.composer_target = target;
    }

    pub fn show_not_sent_hint(&mut self, now_seconds: f64) {
        self.not_sent_hint_started = Some(now_seconds);
    }

    #[must_use]
    pub fn not_sent_hint_visible(&self, now_seconds: f64) -> bool {
        self.not_sent_hint_started.is_some_and(|started| {
            now_seconds >= started && now_seconds - started < NOT_SENT_HINT_SECONDS
        })
    }
```

In `gui/mod.rs`, add `mod composer;` and extend the `pub use supervisor_state::{...}` list with `ComposerTarget`.

- [ ] **Step 3: Implement the composer**

In `design.rs`, add `Send` to `pub enum Icon` and this arm to the `match kind` in `icon`:

```rust
        Icon::Send => {
            painter.line_segment([position(0.5, 0.85), position(0.5, 0.18)], stroke);
            painter.line_segment([position(0.22, 0.45), position(0.5, 0.18)], stroke);
            painter.line_segment([position(0.5, 0.18), position(0.78, 0.45)], stroke);
        }
```

Insert above the test module of `composer.rs`:

```rust
use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, RichText, Sense, Ui, vec2};
use vibemux_probe::ProbeState;

use super::{
    C,
    design::{self, Icon},
    supervisor_state::{ComposerTarget, SupervisorUiState},
};
use crate::view_model::ViewModel;

pub const COMPOSER_TEXT_ID: &str = "coordinator_composer";
pub const SEND_BUTTON_ID: &str = "coordinator_send_button";
pub const COMPOSER_PLACEHOLDER: &str = "How can I help today?";
const SEND_BUTTON_SIZE: f32 = 32.0;
const NOT_SENT_TEXT: &str = "Not sent: coordinator chat is not connected";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetOption {
    pub target: ComposerTarget,
    pub label: String,
    pub enabled: bool,
    pub reason: Option<String>,
}

#[must_use]
pub fn target_options(view_model: &ViewModel) -> Vec<TargetOption> {
    let mut options = vec![TargetOption {
        target: ComposerTarget::Coordinator,
        label: "Coordinator".to_string(),
        enabled: true,
        reason: None,
    }];
    options.extend(view_model.agents.iter().map(|agent| {
        let detected = agent.launcher_state == ProbeState::Verified;
        TargetOption {
            target: ComposerTarget::Harness(agent.name.clone()),
            label: agent.name.clone(),
            enabled: detected,
            reason: (!detected).then(|| "Not detected on this machine".to_string()),
        }
    }));
    options
}

#[must_use]
pub fn caption_text(target: &ComposerTarget) -> String {
    let prefix = match target {
        ComposerTarget::Coordinator => "Draft only".to_string(),
        ComposerTarget::Harness(name) => format!("Draft for {name}"),
    };
    format!("{prefix} · coordinator chat is not connected")
}

pub fn render(ui: &mut Ui, c: &C, state: &mut SupervisorUiState, options: &[TargetOption]) {
    let now = ui.input(|input| input.time);
    design::composer_frame(c).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let mut draft = state.composer_draft().to_string();
        let response = ui.add(
            egui::TextEdit::multiline(&mut draft)
                .id(egui::Id::new(COMPOSER_TEXT_ID))
                .desired_width(f32::INFINITY)
                .desired_rows(2)
                .hint_text(COMPOSER_PLACEHOLDER)
                .frame(false)
                .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                .font(egui::TextStyle::Body),
        );
        if state.take_composer_focus_request() {
            response.request_focus();
        }
        if response.changed() {
            state.set_composer_draft(draft);
        }
        let enter_pressed = response.has_focus()
            && ui.input(|input| {
                let composing = input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Ime(_)));
                !composing && input.key_pressed(Key::Enter) && !input.modifiers.shift
            });
        if enter_pressed {
            attempt_send(state, now);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            target_chip(ui, c, state, options);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let response = send_button(ui, c, state.send_enabled())
                    .on_hover_text(state.send_disabled_reason());
                if response.clicked() {
                    attempt_send(state, now);
                }
            });
        });
    });
    ui.add_space(6.0);
    let hint = state.not_sent_hint_visible(now);
    ui.label(
        RichText::new(if hint { NOT_SENT_TEXT.to_string() } else { caption_text(state.composer_target()) })
            .size(11.5)
            .color(if hint { c.warn } else { c.muted }),
    );
    if hint {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
}

fn attempt_send(state: &mut SupervisorUiState, now: f64) {
    if !state.try_send() {
        state.show_not_sent_hint(now);
    }
}

fn send_button(ui: &mut Ui, c: &C, enabled: bool) -> egui::Response {
    let (_, rect) = ui.allocate_space(vec2(SEND_BUTTON_SIZE, SEND_BUTTON_SIZE));
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, egui::Id::new(SEND_BUTTON_ID), sense);
    let (fill, arrow) = if enabled {
        (c.accent, if c.light { egui::Color32::WHITE } else { c.bg })
    } else {
        (c.border, c.muted)
    };
    ui.painter().circle_filled(rect.center(), SEND_BUTTON_SIZE / 2.0, fill);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)));
    design::icon(&mut child, Icon::Send, arrow, 16.0);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "Send"));
    response
}

fn target_chip(ui: &mut Ui, c: &C, state: &mut SupervisorUiState, options: &[TargetOption]) {
    let current = options
        .iter()
        .find(|option| &option.target == state.composer_target())
        .map_or("Coordinator", |option| option.label.as_str())
        .to_string();
    egui::ComboBox::from_id_salt("composer_target")
        .selected_text(RichText::new(current).size(12.5).color(c.muted))
        .show_ui(ui, |ui| {
            for option in options {
                let selected = &option.target == state.composer_target();
                let response = ui.add_enabled(
                    option.enabled,
                    egui::Button::selectable(selected, option.label.as_str()),
                );
                let response = match option.reason.as_deref() {
                    Some(reason) => response.on_disabled_hover_text(reason),
                    None => response,
                };
                if response.clicked() {
                    state.set_composer_target(option.target.clone());
                }
            }
        });
}
```

Delete `render_composer` from `chat.rs` (and any imports it alone used). In `supervisor_app.rs`, replace both calls `chat::render_composer(ui, &colors, &mut state);` with:

```rust
                        chat::content_column(ui, |ui| {
                            composer::render(ui, &colors, &mut state, &target_options);
                        });
```

and compute once near the top of `render_frame`, after `let colors = ...`:

```rust
        let target_options = composer::target_options(&self.view_model);
```

Add `composer` to the `use super::{...}` list. In the bottom composer panel, drop `.exact_height(204.0)` so the panel sizes to its content.

Run: `cargo test -p vibemux_frontend --lib composer`
Expected: 2 passed.

- [ ] **Step 4: Replace the "Send" text lookup in the render test and add Enter tests**

In `shell_renders_all_themes_sizes_and_scales_without_dropping_the_composer`, replace the `has_send` computation and its assertion with:

```rust
                    let send = context
                        .read_response(egui::Id::new(composer::SEND_BUTTON_ID))
                        .expect("send button rendered");
                    assert!(
                        rect.contains_rect(send.rect),
                        "send button off screen: {theme:?} {width}x{height} scale {scale}"
                    );
                    let _ = output;
```

Add to the same test module:

```rust
    fn composer_key(
        context: &egui::Context,
        app: &mut SupervisorApp,
        modifiers: egui::Modifiers,
        with_ime: bool,
        time: f64,
    ) {
        let mut events = Vec::new();
        if with_ime {
            events.push(egui::Event::Ime(egui::ImeEvent::Preedit("输入".into())));
        }
        events.push(egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
            events,
            modifiers,
            time: Some(time),
            ..Default::default()
        };
        let _ = context.run(raw, |context| app.render_frame(context));
    }

    fn focused_composer_app() -> (egui::Context, SupervisorApp) {
        let mut app = test_app();
        app.ui_state.lock().unwrap().set_composer_draft("draft".to_string());
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| app.render_frame(context));
        context.memory_mut(|memory| memory.request_focus(egui::Id::new(composer::COMPOSER_TEXT_ID)));
        (context, app)
    }

    #[test]
    fn enter_keeps_the_draft_and_shows_not_sent() {
        let (context, mut app) = focused_composer_app();
        composer_key(&context, &mut app, egui::Modifiers::NONE, false, 1.0);
        let state = app.ui_state.lock().unwrap();
        assert_eq!(state.composer_draft(), "draft");
        assert!(state.not_sent_hint_visible(1.5));
        drop(state);
        assert!(app.take_supervisor_actions().iter().all(|action| matches!(action, SupervisorAction::Refresh)));
    }

    #[test]
    fn shift_enter_inserts_a_newline() {
        let (context, mut app) = focused_composer_app();
        composer_key(&context, &mut app, egui::Modifiers::SHIFT, false, 1.0);
        assert!(app.ui_state.lock().unwrap().composer_draft().ends_with('\n'));
    }

    #[test]
    fn enter_during_ime_composition_is_ignored() {
        let (context, mut app) = focused_composer_app();
        composer_key(&context, &mut app, egui::Modifiers::NONE, true, 1.0);
        let state = app.ui_state.lock().unwrap();
        assert_eq!(state.composer_draft(), "draft");
        assert!(!state.not_sent_hint_visible(1.5));
    }
```

- [ ] **Step 5: Run the tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass. If `shift_enter_inserts_a_newline` fails because the cursor starts before the text, it still ends with a newline only when the cursor is at the end; in that case assert `contains('\n')` instead, which is the behavior under test.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/vibemux_frontend
git commit -m "feat(frontend): rounded composer with target chip and honest Enter handling

Enter never sends while chat is unavailable; it keeps the draft and shows
'Not sent'. Shift+Enter inserts a newline. The render guard now checks the
send button's rect instead of a 'Send' text label.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Conversation rows, scroll fade, and top bar

**Files:**
- Modify: `crates/vibemux_frontend/src/gui/chat.rs`
- Modify: `crates/vibemux_frontend/src/gui/design.rs` (`CARD_RADIUS`, `paint_bottom_fade`)
- Modify: `crates/vibemux_frontend/src/gui/typography.rs` (display sizes)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs` (`render_topbar`, `connection_badge`)

**Interfaces:**
- Consumes: `design::{spark, icon_button}`, `typography::display_font`, `supervisor_app::REFRESH_BUTTON_ID` (Task 5).
- Produces: `chat::CONTENT_WIDTH: f32 = 720.0`; `design::{CARD_RADIUS: u8 = 12, paint_bottom_fade(&Painter, Rect, Color32)}`; `typography::{TITLE_SIZE = 22.0, TASK_TITLE_SIZE = 17.0, PROSE_SIZE = 16.0, PROSE_LINE_HEIGHT = 24.0}`.

- [ ] **Step 1: Write the failing test**

Add to the `supervisor_app.rs` test module:

```rust
    #[test]
    fn task_titles_use_the_serif_family() {
        let mut app = test_app();
        let context = egui::Context::default();
        context.set_fonts(super::super::typography::build_font_definitions(|_| None));
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
            ..Default::default()
        };
        let _ = context.run(raw.clone(), |context| app.render_frame(context));
        let output = context.run(raw, |context| app.render_frame(context));
        let serif_title = output.shapes.iter().any(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) => {
                text.galley.job.text == "Second task"
                    && text.galley.job.sections.first().is_some_and(|section| {
                        section.format.font_id.family == super::super::typography::serif_family()
                    })
            }
            _ => false,
        });
        assert!(serif_title);
    }
```

Run: `cargo test -p vibemux_frontend --lib task_titles_use_the_serif_family`
Expected: FAIL (titles use the proportional family).

- [ ] **Step 2: Rewrite the conversation**

In `typography.rs`, add below `GREETING_SIZE`:

```rust
pub const TITLE_SIZE: f32 = 22.0;
pub const TASK_TITLE_SIZE: f32 = 17.0;
pub const PROSE_SIZE: f32 = 16.0;
pub const PROSE_LINE_HEIGHT: f32 = 24.0;
```

In `design.rs`, extend the `use egui::{...}` line with `Rect` and add below `BUTTON_RADIUS`:

```rust
pub const CARD_RADIUS: u8 = 12;

/// Fade `rect` from transparent at the top to `color` at the bottom.
pub fn paint_bottom_fade(painter: &Painter, rect: Rect, color: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), Color32::TRANSPARENT);
    mesh.colored_vertex(rect.right_top(), Color32::TRANSPARENT);
    mesh.colored_vertex(rect.left_bottom(), color);
    mesh.colored_vertex(rect.right_bottom(), color);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(egui::Shape::mesh(mesh));
}
```

In `chat.rs`, add `use super::typography;`, add `pub const CONTENT_WIDTH: f32 = 720.0; const FADE_HEIGHT: f32 = 28.0; const BOTTOM_PADDING: f32 = 48.0;`, change `content_column` to use `ui.available_width().min(CONTENT_WIDTH)`, and replace `render_conversation` with:

```rust
pub fn render_conversation(
    ui: &mut Ui,
    ctx: &egui::Context,
    c: &C,
    snapshot: &SupervisorSnapshot,
    state: &mut SupervisorUiState,
    actions: &UiActionQueue,
) {
    let output = egui::ScrollArea::vertical()
        .id_salt("coordinator_conversation")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            content_column(ui, |ui| {
                ui.add_space(28.0);
                ui.horizontal(|ui| {
                    design::spark(ui, c.accent, 26.0, 0.0);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Workspace updates").size(15.0).strong().color(c.txt));
                        ui.label(
                            RichText::new("Task activity · reported by the daemon")
                                .size(12.0)
                                .color(c.muted),
                        );
                    });
                    if snapshot.mode == SnapshotMode::Demo {
                        demo_badge(ui, c);
                    }
                });
                ui.add_space(22.0);
                ui.label(
                    RichText::new(format!("{} tasks in this workspace", snapshot.tasks.len()))
                        .font(typography::display_font(ui.ctx(), typography::TITLE_SIZE))
                        .color(c.txt),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Open a task to follow its execution. The conversation stays here.")
                        .size(13.0)
                        .color(c.muted),
                );
                ui.add_space(14.0);
                for task in &snapshot.tasks {
                    task_row(ui, ctx, c, snapshot, task, state, actions);
                    ui.add_space(4.0);
                }
                if let Some(cursor) = snapshot.next_cursor.as_ref() {
                    ui.add_space(12.0);
                    if design::quiet_button(ui, c, "Load more tasks").clicked() {
                        actions.enqueue(SupervisorAction::LoadMoreTasks { cursor: cursor.clone() });
                    }
                }
                ui.add_space(BOTTOM_PADDING);
            });
        });
    let viewport = output.inner_rect;
    let more_below = output.state.offset.y + viewport.height() < output.content_size.y - 1.0;
    if more_below {
        let fade = egui::Rect::from_min_max(
            egui::pos2(viewport.left(), viewport.bottom() - FADE_HEIGHT),
            viewport.max,
        );
        design::paint_bottom_fade(ui.painter(), fade, c.bg);
    }
}
```

In `task_row`, wrap the whole body in a hover-tinted borderless frame and use the serif title. Replace the beginning of `task_row` (`ui.add_space(14.0);`) through the end with:

```rust
    let background = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            task_row_contents(ui, ctx, c, snapshot, task, state, actions);
        });
    let rect = inner.response.rect;
    if ui.rect_contains_pointer(rect) {
        ui.painter().set(
            background,
            egui::epaint::RectShape::filled(rect, design::CARD_RADIUS, c.raised),
        );
    }
}

fn task_row_contents(
    ui: &mut Ui,
    ctx: &egui::Context,
    c: &C,
    snapshot: &SupervisorSnapshot,
    task: &TaskView,
    state: &mut SupervisorUiState,
    actions: &UiActionQueue,
) {
```

and inside `task_row_contents` keep the existing body (status line, title button, latest update, actions row) except: remove the leading and trailing `ui.add_space(...)` calls, and change the title button's text to

```rust
            egui::Button::new(
                RichText::new(title)
                    .font(typography::display_font(ui.ctx(), typography::TASK_TITLE_SIZE))
                    .color(c.txt),
            )
```

Also in `task_row_contents`, set the latest update in serif prose:

```rust
    wrapped_label(
        ui,
        RichText::new(&task.latest_update)
            .font(typography::display_font(ui.ctx(), typography::PROSE_SIZE))
            .line_height(Some(typography::PROSE_LINE_HEIGHT))
            .color(c.muted),
    );
```

Delete the now-unreachable empty-state branch (an empty task list shows the welcome state, Task 6).

- [ ] **Step 3: Rewrite the top bar**

In `supervisor_app.rs`, add `const TOPBAR_MIN_HEIGHT: f32 = 52.0;` (`REFRESH_BUTTON_ID` already exists from Task 5) and replace `render_topbar` with:

```rust
    fn render_topbar(&mut self, ctx: &egui::Context, colors: &C, snapshot: &SupervisorSnapshot) {
        egui::TopBottomPanel::top("supervisor_header")
            .show_separator_line(true)
            .min_height(TOPBAR_MIN_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(colors.bg)
                    .inner_margin(egui::Margin::symmetric(20, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let title = snapshot
                        .coordinator
                        .as_deref()
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or("Coordinator");
                    ui.label(RichText::new("Main conversation").size(15.0).strong().color(colors.txt));
                    ui.label(RichText::new(format!("/  {title}")).size(12.0).color(colors.muted));
                    ui.add_space(12.0);
                    connection_indicator(ui, colors, snapshot.connection.status);
                    if snapshot.mode == SnapshotMode::Demo {
                        ui.add_space(10.0);
                        chat::demo_badge(ui, colors);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let refresh = design::icon_button(
                            ui,
                            colors,
                            design::Icon::Refresh,
                            "Refresh",
                            egui::Id::new(REFRESH_BUTTON_ID),
                        );
                        if refresh.clicked() {
                            self.enqueue_refresh();
                        }
                    });
                });
                if let Some(detail) = snapshot.connection.detail.as_deref() {
                    ui.add_space(4.0);
                    chat::wrapped_label(ui, RichText::new(detail).size(12.0).color(colors.muted));
                }
            });
    }
```

Replace `connection_badge` with:

```rust
fn connection_indicator(ui: &mut egui::Ui, colors: &C, status: ConnectionStatus) {
    let (label, color) = match status {
        ConnectionStatus::Connected => ("Connected", colors.ok),
        ConnectionStatus::Connecting => ("Connecting", colors.warn),
        ConnectionStatus::Disconnected => ("Disconnected", colors.muted),
        ConnectionStatus::Unavailable => ("Unavailable", colors.muted),
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
    ui.label(RichText::new(label).size(12.0).color(colors.muted));
}
```

Set `.show_separator_line(false)` on the bottom composer `TopBottomPanel` (the fade replaces its line).

- [ ] **Step 4: Run the tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass, including `task_titles_use_the_serif_family`.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 5: Check the header rule visually**

Run:

```bash
cargo build -p vibemux_frontend --features gui_screenshot --bin vibemux_frontend_preview
mkdir -p ../ui_check
target_dir=$(cargo metadata --format-version 1 --no-deps | ../../../.venv/Scripts/python.exe -c "import json,sys; print(json.load(sys.stdin)['target_directory'])")
"$target_dir/debug/vibemux_frontend_preview.exe" chat claude_light 1280 800 1 "$(cd .. && pwd -W)/ui_check/chat_light.png"
```

Open `../ui_check/chat_light.png`. Expected: exactly one 1 px rule in the border color under the top bar, borderless task rows, and a fade above the composer when rows continue. If a second, darker rule remains under the header, locate its panel by temporarily giving each `TopBottomPanel`/`CentralPanel` frame a distinct `fill` (do not commit), then set `show_separator_line(false)` or remove the stroke on that panel. Delete `../ui_check` afterward.

- [ ] **Step 6: Commit**

```bash
git add crates/vibemux_frontend
git commit -m "feat(frontend): borderless serif task rows, scroll fade, and quiet top bar

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Docked task drawer with one header and an aligned grid

**Files:**
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs` (`render_drawer`, `drawer_contents`, constants, detached window call)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_state.rs` (delete `CHAT_DRAWER_WIDTH`, `CHAT_DRAWER_DOCK_THRESHOLD`)
- Modify: `crates/vibemux_frontend/src/gui/task_detail.rs` (`TitleDisplay`, underline tabs, grid)

**Interfaces:**
- Consumes: `design::icon_button`, `typography::{display_font, TITLE_SIZE}`.
- Adds: `Icon::Close` in `design.rs`.
- Produces: `task_detail::TitleDisplay { Shown, InHeader }` as the last parameter of `task_detail::render`; constants `DRAWER_PANEL_ID = "task_details_drawer"`, `DRAWER_CLOSE_ID`, `DRAWER_DEFAULT_WIDTH = 420.0`, `DRAWER_MIN_WIDTH = 360.0` in `supervisor_app.rs`.

- [ ] **Step 1: Write the failing test**

Add to the `supervisor_app.rs` test module:

```rust
    #[test]
    fn task_drawer_docks_below_the_header_without_duplicate_chrome() {
        for (width, height) in [(960.0, 600.0), (1280.0, 800.0), (1920.0, 1080.0)] {
            let mut app = test_app();
            assert!(app.select_task("task_1"));
            let (context, output) = render_at(&mut app, width, height);
            let drawer = egui::containers::panel::PanelState::load(&context, egui::Id::new(DRAWER_PANEL_ID))
                .expect("docked drawer");
            let header = egui::containers::panel::PanelState::load(&context, egui::Id::new("supervisor_header"))
                .expect("header");
            assert!((drawer.rect.right() - width).abs() < 1.0, "{width}");
            assert!(drawer.rect.top() >= header.rect.bottom() - 0.5, "{width}");
            let texts: Vec<String> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                    _ => None,
                })
                .collect();
            assert!(!texts.iter().any(|text| text == "Task details" || text == "Close"));
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
            let send = context
                .read_response(egui::Id::new(composer::SEND_BUTTON_ID))
                .expect("send button");
            assert!(screen.contains_rect(send.rect), "{width}");
        }
    }
```

Run: `cargo test -p vibemux_frontend --lib task_drawer_docks`
Expected: compile error (`DRAWER_PANEL_ID` not found).

- [ ] **Step 2: Rewrite the drawer**

In `design.rs`, add `Close` to `pub enum Icon` and this arm to the `match kind` in `icon`:

```rust
        Icon::Close => {
            painter.line_segment([position(0.2, 0.2), position(0.8, 0.8)], stroke);
            painter.line_segment([position(0.8, 0.2), position(0.2, 0.8)], stroke);
        }
```

In `supervisor_app.rs`, add constants:

```rust
pub(crate) const DRAWER_PANEL_ID: &str = "task_details_drawer";
const DRAWER_CLOSE_ID: &str = "task_details_close";
const DRAWER_DEFAULT_WIDTH: f32 = 420.0;
const DRAWER_MIN_WIDTH: f32 = 360.0;
```

Replace `render_drawer` with:

```rust
    fn render_drawer(
        &mut self,
        ctx: &egui::Context,
        colors: &C,
        snapshot: &SupervisorSnapshot,
        viewport_width: f32,
    ) {
        let selected_task = self
            .ui_state
            .lock()
            .ok()
            .and_then(|state| state.selected_task(snapshot).cloned());
        if !self.ui_state.lock().is_ok_and(|state| state.details_open()) {
            return;
        }
        let Some(task) = selected_task else {
            return;
        };
        let max_width = (viewport_width * 0.5).max(DRAWER_MIN_WIDTH);
        let mut close_clicked = false;
        egui::SidePanel::right(DRAWER_PANEL_ID)
            .resizable(true)
            .default_width(DRAWER_DEFAULT_WIDTH.min(max_width))
            .width_range(DRAWER_MIN_WIDTH..=max_width)
            .frame(
                egui::Frame::new()
                    .fill(colors.bg)
                    .inner_margin(egui::Margin::symmetric(18, 14)),
            )
            .show(ctx, |ui| {
                drawer_contents(
                    ui,
                    colors,
                    snapshot,
                    &task,
                    &self.ui_state,
                    &self.actions,
                    &mut close_clicked,
                );
            });
        if close_clicked {
            if let Ok(mut state) = self.ui_state.lock() {
                state.close_details();
            }
        }
    }
```

Replace `drawer_contents` with:

```rust
fn drawer_contents(
    ui: &mut egui::Ui,
    colors: &C,
    snapshot: &SupervisorSnapshot,
    task: &TaskView,
    state: &Arc<Mutex<SupervisorUiState>>,
    actions: &UiActionQueue,
    close_clicked: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close = design::icon_button(
                ui,
                colors,
                design::Icon::Close,
                "Close task details",
                egui::Id::new(DRAWER_CLOSE_ID),
            );
            if close.clicked() {
                *close_clicked = true;
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(window_task_title(task))
                            .font(super::typography::display_font(ui.ctx(), super::typography::TITLE_SIZE))
                            .color(colors.txt),
                    )
                    .truncate(),
                );
            });
        });
    });
    ui.add_space(6.0);
    if let Ok(mut state) = state.lock() {
        task_detail::render(
            ui,
            colors,
            snapshot,
            task,
            state.details_selection_mut(),
            actions,
            task_detail::TitleDisplay::InHeader,
        );
    }
}
```

In `window_task_title`, return `"Untitled task"` instead of `"Task details"` for an empty title. Delete `CHAT_DRAWER_WIDTH` and `CHAT_DRAWER_DOCK_THRESHOLD` from `supervisor_state.rs` and from the `use` list in `supervisor_app.rs`; remove `Align2` from the `eframe::egui` import if it is now unused. In `render_task_viewports`, pass `task_detail::TitleDisplay::Shown` as the new last argument.

- [ ] **Step 3: Title display, underline tabs, and grid in `task_detail.rs`**

Add:

```rust
/// Whether the detail view draws the task title itself (detached window) or
/// leaves it to the drawer header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TitleDisplay {
    Shown,
    InHeader,
}

const DETAIL_LABEL_WIDTH: f32 = 76.0;
```

Add `title: TitleDisplay` as the last parameter of `render`, and wrap the existing title label and its following `ui.add_space(3.0);` in `if title == TitleDisplay::Shown { ... }`, changing that label's text to `RichText::new(task_title(task)).font(super::typography::display_font(ui.ctx(), super::typography::TITLE_SIZE)).color(colors.txt)`.

Replace the tab row (`ui.horizontal_wrapped(|ui| { for (tab, label) in [...] ... });`) with:

```rust
            ui.horizontal_wrapped(|ui| {
                for (tab, label) in [
                    (TaskDetailTab::Overview, "Overview"),
                    (TaskDetailTab::Activity, "Activity"),
                    (TaskDetailTab::Artifacts, "Artifacts"),
                    (TaskDetailTab::Terminal, "Terminal"),
                ] {
                    let selected = selection.tab == tab;
                    let response = ui.add(
                        egui::Button::new(RichText::new(label).size(13.0).color(if selected {
                            colors.accent_text
                        } else {
                            colors.muted
                        }))
                        .frame(false),
                    );
                    if selected {
                        ui.painter().hline(
                            response.rect.x_range(),
                            response.rect.bottom() + 3.0,
                            egui::Stroke::new(2.0, colors.accent),
                        );
                    }
                    if response.clicked() {
                        selection.tab = tab;
                    }
                }
            });
```

Replace `detail_row` with a grid helper and use it in `render_overview`:

```rust
fn detail_grid(ui: &mut Ui, colors: &C, id: &str, rows: &[(&str, String)]) {
    let value_width = (ui.available_width() - DETAIL_LABEL_WIDTH - 16.0).max(80.0);
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([16.0, 7.0])
        .min_col_width(DETAIL_LABEL_WIDTH)
        .max_col_width(value_width)
        .show(ui, |ui| {
            for (label, value) in rows {
                ui.label(RichText::new(*label).size(12.0).color(colors.muted));
                wrapped_label(ui, RichText::new(value.as_str()).size(12.0).color(colors.txt).monospace());
                ui.end_row();
            }
        });
}
```

In `render_overview`, replace the five `detail_row` calls with:

```rust
        detail_grid(
            ui,
            colors,
            "run_details_grid",
            &[
                ("Harness", nonempty(&run.harness, "Unknown").to_string()),
                ("Role", nonempty(&run.role, "Not recorded").to_string()),
                ("Run state", design::state_text(&run.state)),
                ("Branch", nonempty(&run.branch, "Not recorded").to_string()),
                ("Worktree", nonempty(&run.worktree, "Not recorded").to_string()),
            ],
        );
```

and inside the "Identity & references" header replace the two `detail_row` calls with:

```rust
        let mut rows = vec![("Task", task.task_id.clone())];
        if let Some(run) = run {
            rows.push(("Run", run.run_id.clone()));
        }
        detail_grid(ui, colors, "identity_grid", &rows);
```

Fix any other `detail_row` call the compiler reports the same way (build the rows, call `detail_grid`).

- [ ] **Step 4: Run the tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass, including `task_drawer_docks_below_the_header_without_duplicate_chrome`.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_frontend
git commit -m "fix(frontend): always dock the task drawer with one header and aligned details

Removes the floating overlay that covered the top bar below 1400 px and the
duplicated 'Task details' header.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Delete the uncompiled legacy shell files

**Files:**
- Delete: `crates/vibemux_frontend/src/gui/app.rs`, `crates/vibemux_frontend/src/gui/overview.rs`, `crates/vibemux_frontend/src/gui/workbench.rs`, `crates/vibemux_frontend/src/gui/stub_bank.rs`

**Interfaces:** none.

- [ ] **Step 1: Confirm they are not modules**

Run: `rg -n "mod (app|overview|workbench|stub_bank)\b|#\[path" crates/vibemux_frontend/src`
Expected: no output.

- [ ] **Step 2: Delete and check references**

```bash
git rm crates/vibemux_frontend/src/gui/app.rs crates/vibemux_frontend/src/gui/overview.rs crates/vibemux_frontend/src/gui/workbench.rs crates/vibemux_frontend/src/gui/stub_bank.rs
rg -n "stub_bank|gui/workbench|gui/overview|gui/app\.rs" --glob '!target' --glob '!CHANGELOG.md' --glob '!PROGRESS.md' --glob '!docs/adr/**'
```

Expected: no output from `rg` (historical mentions in the changelog, progress log, and ADRs stay as history). If a current doc presents one of these files as live code, reword that sentence to past tense.

- [ ] **Step 3: Build and test**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass (identical results to Task 10).

- [ ] **Step 4: Commit**

```bash
git commit -m "chore(frontend): delete four uncompiled ADR 027 shell files

gui/app.rs, overview.rs, workbench.rs, and stub_bank.rs were no longer
declared as modules after ADR 028.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Phase 1 previews, docs, verification, and pull request

**Files:**
- Modify: `crates/vibemux_frontend/src/bin/vibemux_frontend_preview.rs` (`welcome`, `collapsed` scenarios)
- Modify: `README.md`, `docs/architecture.md`, `CHANGELOG.md`, `PROGRESS.md`, `docs/adr/030_claude_style_gui.md` (status)
- Modify: `docs/frontend_previews/*.png` (regenerated), add `supervisor_welcome.png`, `supervisor_collapsed.png`

**Interfaces:**
- Consumes: `SupervisorApp::{request_new_task, set_sidebar_collapsed}`.

- [ ] **Step 1: Add the preview scenarios**

In `vibemux_frontend_preview.rs`, after `app.set_supervisor_snapshot(snapshot);`, add:

```rust
            match scenario {
                "welcome" => app.request_new_task(),
                "collapsed" => app.set_sidebar_collapsed(true),
                _ => {}
            }
```

- [ ] **Step 2: Capture and review screenshots**

```bash
cargo build -p vibemux_frontend --features gui_screenshot --bin vibemux_frontend_preview
target_dir=$(cargo metadata --format-version 1 --no-deps | ../../../.venv/Scripts/python.exe -c "import json,sys; print(json.load(sys.stdin)['target_directory'])")
mkdir -p ../ui_check
for theme in claude_light claude; do
  for scenario in chat drawer welcome collapsed; do
    for size in "1280 800 1" "960 600 1" "1280 800 1.5"; do
      set -- $size
      "$target_dir/debug/vibemux_frontend_preview.exe" $scenario $theme $1 $2 $3 "$(cd .. && pwd -W)/ui_check/${scenario}_${theme}_$1x$2_$3.png" || echo "FAILED $scenario $theme $size"
    done
  done
done
```

Expected: no `FAILED` lines. Open each image and check: serif greeting and titles, borderless rows, fade above the composer, docked drawer with one header and a close icon, aligned run details, collapsed rail at 56 px, no clipped send button, and no second dark rule under the header. Fix and re-run anything that fails before continuing.

- [ ] **Step 3: Regenerate the committed previews**

```bash
out="$(pwd -W)/docs/frontend_previews"
for scenario in chat drawer task task_terminal disconnected welcome collapsed; do
  "$target_dir/debug/vibemux_frontend_preview.exe" $scenario claude_light 1440 900 1 "$out/supervisor_${scenario}.png"
done
rm -r ../ui_check
```

- [ ] **Step 4: Update the docs**

In `README.md`, replace the paragraph that begins "The redesigned GUI defaults to **Studio**" with:

```text
The GUI defaults to **Claude light**: a cream canvas, terracotta accent, serif
display type from the system's Georgia or Cambria, and a Claude Desktop style
sidebar with **New task**, Conversation, Agents, a Recents list, and a
workspace menu. The sidebar collapses to an icon rail. Seven themes are
available in **Settings → Appearance**: `claude_light`, `claude` (warm
charcoal), `studio`, `github`, `vscode`, `nord`, and `gruvbox`. Existing saved
appearance preferences are preserved. Sending from the composer is not
available yet: Enter keeps the draft and says it was not sent, and Shift+Enter
inserts a newline.
```

Change the scene list sentence to "Scenes are `chat`, `drawer`, `task`, `task_terminal`, `disconnected`, `welcome`, and `collapsed`." and the example command's theme argument from `studio` to `claude_light`.

In `docs/architecture.md`, replace "The Studio light theme is the default for new profiles; five existing dark themes remain compatible." with "The Claude light theme is the default for new profiles (ADR 030); Studio and the five dark themes remain available."

Add to the top of `CHANGELOG.md` "Unreleased":

```text
- Claude-style GUI, phase 1 (ADR 030): new `claude_light` theme (default for new profiles) and a warm charcoal `claude` theme, serif display type from system fonts, readable accent text in every theme, a collapsible Claude Desktop style sidebar with New task, Recents, and a workspace menu, a welcome state with a time-of-day greeting, a rounded composer with a target chip where Enter keeps the draft and reports "Not sent" while chat is unavailable, borderless task rows with a scroll fade, and a task drawer that is always docked with one header and aligned details. The exported palette file and the Python fallback copy follow the palette change. Four uncompiled ADR 027 shell files are deleted.
```

Add a `PROGRESS.md` entry after the latest dated entry (use today's date and the next free entry number; the heading below assumes 2026-09-28 (3)):

```text
### 2026-09-28 (3) - Claude-style GUI phase 1 (ADR 030, `feat/claude_style_gui`)

**Status change**
- M5.0 frontend stays `PARTIAL`. ADR 030 phase 1 is implemented; phase 2 (shortcuts, quick switcher, appearance modes, motion, copy buttons) is not.

**Implemented**
- `claude_light` palette as the default for new profiles and a retuned `claude` palette; `config/theme_palettes.json` and the Python fallback updated under the parity test; ADR 026's surface invariant amended to a 5.0 luma gap in either direction.
- WCAG contrast math, luminance-based light detection, and readable accent text in every palette.
- Serif display family from Georgia or Cambria with independent sans and CJK loading.
- Collapsible sidebar with New task, Recents, and a workspace menu; welcome state; rounded composer with a target chip; borderless task rows with a scroll fade; always-docked task drawer with one header and an aligned details grid; deletion of four uncompiled shell files.

**Evidence**
- Commands and results: record the exact `cargo fmt`, `cargo clippy`, `cargo test --workspace --all-features`, and Python pytest/ruff/mypy results from Step 5 here.
- Screenshots: `claude_light` and `claude` at 1280×800, 960×600, and 1.5× scale for chat, drawer, welcome, and collapsed were reviewed; committed previews regenerated.

**Remaining**
- ADR 030 phase 2. Live Windows visual check on a real display scale other than 1.0 and 1.5.
```

In `docs/adr/030_claude_style_gui.md`, replace the status paragraph with:

```text
Status: Accepted. Phase 1 (palettes, typography, sidebar, welcome state,
composer, conversation rows, and the docked drawer) is implemented. Phase 2
(shortcuts, quick switcher, appearance modes, motion, and copy buttons) is not
implemented yet. PROGRESS.md records the status of each phase.
```

- [ ] **Step 5: Run the full gates**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
PYTHONPATH="$PWD/src" ../../../.venv/Scripts/python.exe -m pytest -q
PYTHONPATH="$PWD/src" ../../../.venv/Scripts/python.exe -m ruff check .
PYTHONPATH="$PWD/src" ../../../.venv/Scripts/python.exe -m ruff format --check .
PYTHONPATH="$PWD/src" ../../../.venv/Scripts/python.exe -m mypy .
git diff --check
```

Expected: every command exits 0. Write the actual counts into the `PROGRESS.md` evidence line.

- [ ] **Step 6: Commit, push, and open the pull request**

```bash
git add -A crates/vibemux_frontend/src/bin/vibemux_frontend_preview.rs README.md docs CHANGELOG.md PROGRESS.md
git status --short
git diff --cached > "$TMPDIR/staged_review.diff"   # scan this with the operator's private-term list, then delete it
git commit -m "docs(frontend): ADR 030 phase 1 previews, README, changelog, and progress

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git ls-remote origin refs/heads/feat/claude_style_gui
git push origin refs/heads/feat/claude_style_gui:refs/heads/feat/claude_style_gui
git ls-remote origin refs/heads/feat/claude_style_gui
git rev-parse HEAD
```

Expected: `git status --short` shows no `.kilo/` or `.zcode/` entries staged; the private-term scan of the staged diff finds nothing (delete the diff file afterward); the remote ref after the push equals `HEAD`. Then open the pull request into `main` with `gh pr create --repo YueDongSun/vibemux --base main --head feat/claude_style_gui` and a body that summarizes phase 1, lists the gates with their results, and ends with the attribution line. Bind it with the ccd_pr tools (`get_status`, then `bind_pr` if needed).

---

# Phase 2 (pull request 2, branch from `main` after PR 1 merges)

Create a new branch `feat/claude_style_gui_phase_two` from the updated `main` in a new worktree before Task 13, and unset its upstream.

### Task 13: Appearance modes and Reduce motion

**Files:**
- Create: `crates/vibemux_frontend/src/theme/appearance.rs`
- Modify: `crates/vibemux_frontend/src/theme/mod.rs` (`pub mod appearance;`)
- Modify: `crates/vibemux_frontend/src/theme/serialize.rs` (two fields)
- Modify: `crates/vibemux_frontend/src/gui/settings.rs` (appearance control, reduce motion)
- Modify: `crates/vibemux_frontend/src/gui/sidebar.rs` (menu entries)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs` (effective theme, setters)

**Interfaces:**
- Produces: `appearance::{AppearanceMode { Light, Dark, MatchSystem }, AppearanceMode::ALL, AppearanceMode::label(self) -> &'static str, SystemTheme { Light, Dark }, is_claude_pair(ThemeId) -> bool, effective_theme(ThemeId, bool, Option<SystemTheme>) -> ThemeId, current_mode(ThemeId, bool) -> Option<AppearanceMode>, choose_mode(ThemeId, AppearanceMode) -> (ThemeId, bool)}`; `UserConfig.{follow_system_appearance, reduce_motion}: bool`; `SupervisorApp::{set_appearance_mode(&mut self, AppearanceMode), set_reduce_motion(&mut self, bool)}`; `SidebarView.appearance_mode: Option<AppearanceMode>`; `SidebarActions.appearance_mode: Option<AppearanceMode>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/vibemux_frontend/src/theme/appearance.rs` with its header and tests:

```rust
#![forbid(unsafe_code)]
//! Light / Dark / Match system for the Claude theme pair (ADR 030 §5).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_system_only_applies_to_the_claude_pair() {
        use SystemTheme::{Dark, Light};
        for stored in [ThemeId::Claude, ThemeId::ClaudeLight] {
            assert_eq!(effective_theme(stored, true, Some(Dark)), ThemeId::Claude);
            assert_eq!(effective_theme(stored, true, Some(Light)), ThemeId::ClaudeLight);
            assert_eq!(effective_theme(stored, true, None), ThemeId::ClaudeLight);
            assert_eq!(effective_theme(stored, false, Some(Dark)), stored);
        }
        assert_eq!(effective_theme(ThemeId::Nord, true, Some(Light)), ThemeId::Nord);
    }

    #[test]
    fn current_mode_reflects_the_stored_choice() {
        assert_eq!(current_mode(ThemeId::ClaudeLight, false), Some(AppearanceMode::Light));
        assert_eq!(current_mode(ThemeId::Claude, false), Some(AppearanceMode::Dark));
        assert_eq!(current_mode(ThemeId::Claude, true), Some(AppearanceMode::MatchSystem));
        assert_eq!(current_mode(ThemeId::Studio, true), None);
    }

    #[test]
    fn choosing_a_mode_stores_theme_and_follow_flag() {
        assert_eq!(choose_mode(ThemeId::Nord, AppearanceMode::Light), (ThemeId::ClaudeLight, false));
        assert_eq!(choose_mode(ThemeId::Nord, AppearanceMode::Dark), (ThemeId::Claude, false));
        assert_eq!(choose_mode(ThemeId::Claude, AppearanceMode::MatchSystem), (ThemeId::Claude, true));
        assert_eq!(choose_mode(ThemeId::Nord, AppearanceMode::MatchSystem), (ThemeId::ClaudeLight, true));
    }
}
```

Add `pub mod appearance;` to `theme/mod.rs`. Add to the `serialize.rs` tests:

```rust
    #[test]
    fn config_without_appearance_fields_keeps_theme() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("frontend.json");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"theme":"claude","window_size":{"width":1280,"height":800},"sidebar_collapsed":true}"#,
        )
        .expect("write");
        let config = load_user_config_at(&path);
        assert_eq!(config.theme, ThemeId::Claude);
        assert!(config.sidebar_collapsed);
        assert!(!config.follow_system_appearance);
        assert!(!config.reduce_motion);
    }
```

Run: `cargo test -p vibemux_frontend --lib -- appearance serialize`
Expected: compile errors.

- [ ] **Step 2: Implement appearance resolution and the config fields**

Insert above the tests in `appearance.rs`:

```rust
use super::ThemeId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppearanceMode {
    Light,
    Dark,
    MatchSystem,
}

impl AppearanceMode {
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::MatchSystem];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::MatchSystem => "Match system",
        }
    }
}

/// The OS theme reported by the windowing layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemTheme {
    Light,
    Dark,
}

#[must_use]
pub const fn is_claude_pair(theme: ThemeId) -> bool {
    matches!(theme, ThemeId::Claude | ThemeId::ClaudeLight)
}

/// The palette to render. Match system applies only to the Claude pair and
/// uses light when the OS reports nothing.
#[must_use]
pub fn effective_theme(stored: ThemeId, follow_system: bool, system: Option<SystemTheme>) -> ThemeId {
    if !(follow_system && is_claude_pair(stored)) {
        return stored;
    }
    match system {
        Some(SystemTheme::Dark) => ThemeId::Claude,
        Some(SystemTheme::Light) | None => ThemeId::ClaudeLight,
    }
}

/// The control's selection, or `None` while a non-Claude theme is active.
#[must_use]
pub fn current_mode(stored: ThemeId, follow_system: bool) -> Option<AppearanceMode> {
    if !is_claude_pair(stored) {
        return None;
    }
    Some(if follow_system {
        AppearanceMode::MatchSystem
    } else if stored == ThemeId::Claude {
        AppearanceMode::Dark
    } else {
        AppearanceMode::Light
    })
}

/// Stored `(theme, follow_system)` after the user picks a mode.
#[must_use]
pub fn choose_mode(stored: ThemeId, mode: AppearanceMode) -> (ThemeId, bool) {
    match mode {
        AppearanceMode::Light => (ThemeId::ClaudeLight, false),
        AppearanceMode::Dark => (ThemeId::Claude, false),
        AppearanceMode::MatchSystem => (
            if is_claude_pair(stored) { stored } else { ThemeId::ClaudeLight },
            true,
        ),
    }
}
```

In `UserConfig`, add after `sidebar_collapsed`:

```rust
    /// Follow the OS light or dark setting within the Claude pair.
    #[serde(default)]
    pub follow_system_appearance: bool,
    /// Keep the spark static and request no animation repaints.
    #[serde(default)]
    pub reduce_motion: bool,
```

and set both to `false` in `Default`.

Run: `cargo test -p vibemux_frontend --lib -- appearance serialize`
Expected: all pass.

- [ ] **Step 3: Write the failing render test**

Add to the `supervisor_app.rs` tests:

```rust
    fn render_with_system_theme(app: &mut SupervisorApp, system: Option<egui::Theme>) -> egui::Context {
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
            system_theme: system,
            ..Default::default()
        };
        let _ = context.run(raw.clone(), |context| app.render_frame(context));
        let _ = context.run(raw, |context| app.render_frame(context));
        context
    }

    #[test]
    fn match_system_follows_a_dark_os() {
        let mut app = test_app();
        app.set_appearance_mode(crate::theme::appearance::AppearanceMode::MatchSystem);
        let context = render_with_system_theme(&mut app, Some(egui::Theme::Dark));
        assert!(context.style().visuals.dark_mode);
    }

    #[test]
    fn match_system_without_an_os_theme_renders_light() {
        let mut app = test_app();
        app.set_appearance_mode(crate::theme::appearance::AppearanceMode::MatchSystem);
        let context = render_with_system_theme(&mut app, None);
        assert!(!context.style().visuals.dark_mode);
    }

    #[test]
    fn picking_another_theme_turns_match_system_off() {
        let mut app = test_app();
        app.set_appearance_mode(crate::theme::appearance::AppearanceMode::MatchSystem);
        app.set_theme(ThemeId::Nord);
        assert!(!app.user_config.follow_system_appearance);
    }
```

Run: `cargo test -p vibemux_frontend --lib match_system picking_another_theme`
Expected: compile error (`set_appearance_mode` not found).

- [ ] **Step 4: Wire the app, settings, and menu**

In `supervisor_app.rs`, change `set_theme` to also clear the follow flag:

```rust
    fn set_theme(&mut self, theme: ThemeId) {
        if self.user_config.theme != theme || self.user_config.follow_system_appearance {
            self.user_config.theme = theme;
            self.user_config.follow_system_appearance = false;
            self.pending_write = Some(Instant::now());
        }
    }

    /// Apply a Light, Dark, or Match system choice and persist it.
    pub fn set_appearance_mode(&mut self, mode: crate::theme::appearance::AppearanceMode) {
        let (theme, follow) = crate::theme::appearance::choose_mode(self.user_config.theme, mode);
        if self.user_config.theme != theme || self.user_config.follow_system_appearance != follow {
            self.user_config.theme = theme;
            self.user_config.follow_system_appearance = follow;
            self.pending_write = Some(Instant::now());
        }
    }

    pub fn set_reduce_motion(&mut self, reduce_motion: bool) {
        if self.user_config.reduce_motion != reduce_motion {
            self.user_config.reduce_motion = reduce_motion;
            self.pending_write = Some(Instant::now());
        }
    }
```

In `render_frame`, replace `let palette = palette_for(self.user_config.theme);` with:

```rust
        let system_theme = ctx.system_theme().map(|theme| match theme {
            egui::Theme::Dark => crate::theme::appearance::SystemTheme::Dark,
            egui::Theme::Light => crate::theme::appearance::SystemTheme::Light,
        });
        let theme = crate::theme::appearance::effective_theme(
            self.user_config.theme,
            self.user_config.follow_system_appearance,
            system_theme,
        );
        let palette = palette_for(theme);
```

and pass `theme` (not `self.user_config.theme`) to `render_task_viewports`.

Replace `settings.rs` `render` signature and body. New types:

```rust
pub struct SettingsView {
    pub theme: ThemeId,
    pub follow_system: bool,
    pub reduce_motion: bool,
    pub window_size: WindowSize,
    pub demo_mode: bool,
}

#[derive(Default)]
pub struct SettingsChanges {
    pub theme: Option<ThemeId>,
    pub appearance_mode: Option<AppearanceMode>,
    pub reduce_motion: Option<bool>,
    pub window_size: Option<WindowSize>,
}
```

`pub fn render(ctx: &egui::Context, colors: &C, open: &mut bool, view: &SettingsView) -> SettingsChanges`, keeping the demo label and window size section as they are, and replacing the Appearance section with:

```rust
            ui.label(RichText::new("Appearance").size(14.0).strong().color(colors.txt));
            ui.add_space(6.0);
            let current = appearance::current_mode(view.theme, view.follow_system);
            ui.horizontal(|ui| {
                for mode in AppearanceMode::ALL {
                    if ui
                        .add(egui::Button::selectable(current == Some(mode), mode.label()))
                        .clicked()
                    {
                        changes.appearance_mode = Some(mode);
                    }
                }
            });
            ui.label(
                RichText::new("Light and Dark use the Claude themes. Match system follows Windows.")
                    .size(12.0)
                    .color(colors.muted),
            );
            ui.add_space(10.0);
            ui.label(RichText::new("Theme").size(13.0).color(colors.txt));
            ui.horizontal_wrapped(|ui| {
                for theme in ThemeId::ALL {
                    if ui.add(egui::Button::selectable(theme == view.theme, theme.as_str())).clicked() {
                        changes.theme = Some(theme);
                    }
                }
            });
            ui.add_space(8.0);
            let mut reduce_motion = view.reduce_motion;
            if ui.checkbox(&mut reduce_motion, "Reduce motion").changed() {
                changes.reduce_motion = Some(reduce_motion);
            }
            ui.add_space(18.0);
```

with imports `use crate::theme::{ThemeId, WindowSize, appearance::{self, AppearanceMode}};`. Update the call in `render_frame`:

```rust
            let changes = settings::render(
                ctx,
                &colors,
                &mut self.settings_open,
                &settings::SettingsView {
                    theme: self.user_config.theme,
                    follow_system: self.user_config.follow_system_appearance,
                    reduce_motion: self.user_config.reduce_motion,
                    window_size: self.user_config.window_size,
                    demo_mode: snapshot.mode == SnapshotMode::Demo,
                },
            );
            if let Some(theme) = changes.theme {
                self.set_theme(theme);
            }
            if let Some(mode) = changes.appearance_mode {
                self.set_appearance_mode(mode);
            }
            if let Some(reduce_motion) = changes.reduce_motion {
                self.set_reduce_motion(reduce_motion);
            }
            if let Some(size) = changes.window_size {
                self.set_window_size(size);
            }
```

In `sidebar.rs`, add `pub appearance_mode: Option<AppearanceMode>,` to both `SidebarView` and `SidebarActions`, import `crate::theme::appearance::AppearanceMode`, pass `view` into `workspace_button` (change its `snapshot` parameter to `view: &SidebarView<'_>` and use `view.snapshot`), and add to the popup after Diagnostics:

```rust
        ui.separator();
        ui.label(RichText::new("Appearance").size(11.0).color(c.muted));
        for mode in AppearanceMode::ALL {
            if ui.add(egui::Button::selectable(view.appearance_mode == Some(mode), mode.label())).clicked() {
                actions.appearance_mode = Some(mode);
            }
        }
```

In `render_frame`, set `appearance_mode: crate::theme::appearance::current_mode(self.user_config.theme, self.user_config.follow_system_appearance)` in the `SidebarView` literal, and after the other sidebar actions add:

```rust
            if let Some(mode) = actions.appearance_mode {
                self.set_appearance_mode(mode);
            }
```

- [ ] **Step 5: Run the tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/vibemux_frontend
git commit -m "feat(frontend): Light, Dark, and Match system appearance with Reduce motion

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: Shortcut table, handler, and overlay

**Files:**
- Create: `crates/vibemux_frontend/src/gui/shortcuts.rs`
- Modify: `crates/vibemux_frontend/src/gui/mod.rs` (`mod shortcuts;`)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs` (`handle_keyboard`, overlay flag, `open_shortcut_overlay`)
- Modify: `crates/vibemux_frontend/src/gui/sidebar.rs` (menu entry)

**Interfaces:**
- Consumes: `SupervisorApp::{request_new_task, set_sidebar_collapsed}`.
- Produces: `shortcuts::{ShortcutAction, ShortcutSpec, SHORTCUTS: &[ShortcutSpec], consume_global_chord(&mut egui::InputState) -> Option<ShortcutAction>, render_overlay(&egui::Context, &C, &mut bool)}`; `SupervisorApp::open_shortcut_overlay(&mut self)`; `SidebarActions.open_shortcuts: bool`.

- [ ] **Step 1: Make the key helper realistic and expose the Ctrl+digit bug**

egui-winit reports Ctrl on Windows and Linux as `ctrl: true, command: true`. The current handler accepts only modifiers exactly equal to `Modifiers::CTRL` or `Modifiers::COMMAND`, so `Ctrl+1` to `Ctrl+0` never fire with real Windows input; the existing test hides this because its helper sets only `ctrl`. In the `supervisor_app.rs` test module, change the modifiers in `key_event` to:

```rust
        let modifiers = egui::Modifiers {
            ctrl,
            command: ctrl,
            ..Default::default()
        };
```

Run: `cargo test -p vibemux_frontend --lib all_ten_harness_shortcuts_and_escape_precedence_work`
Expected: FAIL (`selected_agent()` stays `None`), reproducing the bug. Step 4 fixes it.

- [ ] **Step 2: Write the failing shortcut tests**

Create `shortcuts.rs` with the header and tests:

```rust
#![forbid(unsafe_code)]
//! The one shortcut table: key handling and the `Ctrl+/` overlay both read
//! it, so they cannot drift apart (ADR 030 §5).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_two_actions_share_a_chord() {
        let chords: Vec<_> = SHORTCUTS.iter().filter_map(|spec| spec.chord).collect();
        for (index, chord) in chords.iter().enumerate() {
            assert!(!chords[index + 1..].contains(chord), "{chord:?}");
        }
    }

    #[test]
    fn overlay_lists_every_shortcut() {
        let context = egui::Context::default();
        let palette = crate::theme::palette_for(crate::theme::ThemeId::ClaudeLight);
        let colors = super::super::pal(&palette);
        let mut open = true;
        let _ = context.run(egui::RawInput::default(), |context| render_overlay(context, &colors, &mut open));
        let output = context.run(egui::RawInput::default(), |context| render_overlay(context, &colors, &mut open));
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        for spec in SHORTCUTS {
            assert!(texts.iter().any(|text| text == spec.keys), "{}", spec.keys);
        }
    }
}
```

Add `mod shortcuts;` to `gui/mod.rs`. In the `supervisor_app.rs` tests, add:

```rust
    #[test]
    fn global_shortcuts_run_their_actions() {
        let mut app = test_app();
        let context = egui::Context::default();
        key_event(&context, &mut app, Key::B, true, false);
        assert!(app.user_config.sidebar_collapsed);
        key_event(&context, &mut app, Key::Comma, true, false);
        assert!(app.settings_open);
        key_event(&context, &mut app, Key::Slash, true, false);
        assert!(app.shortcut_overlay_open);
        key_event(&context, &mut app, Key::N, true, false);
        assert!(app.ui_state.lock().unwrap().welcome_active());
    }

    #[test]
    fn global_shortcuts_are_ignored_during_ime_composition() {
        let mut app = test_app();
        key_event(&egui::Context::default(), &mut app, Key::B, true, true);
        assert!(!app.user_config.sidebar_collapsed);
    }

    #[test]
    fn new_task_shortcut_works_while_the_composer_has_focus() {
        let (context, mut app) = focused_composer_app();
        app.ui_state.lock().unwrap().set_composer_draft(String::new());
        key_event(&context, &mut app, Key::N, true, false);
        assert!(app.ui_state.lock().unwrap().welcome_active());
    }
```

Run: `cargo test -p vibemux_frontend --lib -- shortcut global_shortcuts new_task_shortcut`
Expected: compile errors.

- [ ] **Step 3: Implement the table and overlay**

Insert above the tests in `shortcuts.rs`:

```rust
use eframe::egui::{self, InputState, Key, KeyboardShortcut, Modifiers, RichText};

use super::C;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShortcutAction {
    NewTask,
    ToggleSidebar,
    OpenSettings,
    ShortcutOverlay,
    CloseOverlay,
    SelectAgent,
}

#[derive(Clone, Copy, Debug)]
pub struct ShortcutSpec {
    pub action: ShortcutAction,
    pub keys: &'static str,
    pub description: &'static str,
    /// `None` for rows handled by the focus-guarded legacy path.
    pub chord: Option<KeyboardShortcut>,
}

pub const SHORTCUTS: &[ShortcutSpec] = &[
    ShortcutSpec {
        action: ShortcutAction::NewTask,
        keys: "Ctrl+N",
        description: "Start a new task",
        chord: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::N)),
    },
    ShortcutSpec {
        action: ShortcutAction::ToggleSidebar,
        keys: "Ctrl+B",
        description: "Collapse or expand the sidebar",
        chord: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::B)),
    },
    ShortcutSpec {
        action: ShortcutAction::OpenSettings,
        keys: "Ctrl+,",
        description: "Open Settings",
        chord: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma)),
    },
    ShortcutSpec {
        action: ShortcutAction::ShortcutOverlay,
        keys: "Ctrl+/",
        description: "Show keyboard shortcuts",
        chord: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Slash)),
    },
    ShortcutSpec {
        action: ShortcutAction::CloseOverlay,
        keys: "Esc",
        description: "Close the top-most overlay",
        chord: None,
    },
    ShortcutSpec {
        action: ShortcutAction::SelectAgent,
        keys: "Ctrl+1 to Ctrl+0",
        description: "Open an agent's diagnostics",
        chord: None,
    },
];

/// Consume the first global chord pressed this frame. Called before any
/// widget runs, so text fields never see these chords.
pub fn consume_global_chord(input: &mut InputState) -> Option<ShortcutAction> {
    SHORTCUTS.iter().find_map(|spec| {
        let chord = spec.chord?;
        input.consume_shortcut(&chord).then_some(spec.action)
    })
}

pub fn render_overlay(ctx: &egui::Context, c: &C, open: &mut bool) {
    if !*open {
        return;
    }
    let modal = egui::Modal::new(egui::Id::new("shortcut_overlay")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.label(RichText::new("Keyboard shortcuts").size(16.0).color(c.txt));
        ui.add_space(10.0);
        egui::Grid::new("shortcut_overlay_grid")
            .num_columns(2)
            .spacing([18.0, 8.0])
            .show(ui, |ui| {
                for spec in SHORTCUTS {
                    ui.label(RichText::new(spec.keys).monospace().size(12.5).color(c.txt));
                    ui.label(RichText::new(spec.description).size(13.0).color(c.muted));
                    ui.end_row();
                }
            });
    });
    if modal.should_close() {
        *open = false;
    }
}
```

- [ ] **Step 4: Rewire `handle_keyboard` and fix Ctrl detection**

Add the field `shortcut_overlay_open: bool` to `SupervisorApp` (initialize `false` in `new` and `test_app`), the method:

```rust
    pub fn open_shortcut_overlay(&mut self) {
        self.shortcut_overlay_open = true;
    }

    fn run_shortcut(&mut self, action: shortcuts::ShortcutAction) {
        match action {
            shortcuts::ShortcutAction::NewTask => self.request_new_task(),
            shortcuts::ShortcutAction::ToggleSidebar => {
                self.set_sidebar_collapsed(!self.user_config.sidebar_collapsed);
            }
            shortcuts::ShortcutAction::OpenSettings => self.settings_open = true,
            shortcuts::ShortcutAction::ShortcutOverlay => self.shortcut_overlay_open = true,
            shortcuts::ShortcutAction::CloseOverlay | shortcuts::ShortcutAction::SelectAgent => {}
        }
    }
```

and replace the start of `handle_keyboard` (the first `if ctx.wants_keyboard_input() || ime_composition_active(ctx) { return; }`) with:

```rust
        if ime_composition_active(ctx) {
            return;
        }
        if let Some(action) = ctx.input_mut(shortcuts::consume_global_chord) {
            self.run_shortcut(action);
            return;
        }
        if ctx.wants_keyboard_input() {
            return;
        }
```

Fix the Ctrl detection: replace

```rust
        let ctrl = input.modifiers == Modifiers::CTRL || input.modifiers == Modifiers::COMMAND;
```

with

```rust
        // egui-winit sets both `ctrl` and `command` for Ctrl on Windows and Linux.
        let ctrl = input.modifiers.command_only();
```

and remove `Modifiers` from the `eframe::egui` import if it becomes unused.

In the `Escape` branch, after the discard prompt check, add:

```rust
            if self.shortcut_overlay_open {
                self.shortcut_overlay_open = false;
                return;
            }
```

In `render_frame`, after the diagnostics block, add `shortcuts::render_overlay(ctx, &colors, &mut self.shortcut_overlay_open);`. Add `shortcuts` to the `use super::{...}` list.

In `sidebar.rs`, add `pub open_shortcuts: bool,` to `SidebarActions` and a menu entry after Diagnostics:

```rust
        if ui.button("Keyboard shortcuts").clicked() {
            actions.open_shortcuts = true;
        }
```

and in `render_frame` handle `if actions.open_shortcuts { self.shortcut_overlay_open = true; }`.

- [ ] **Step 5: Run the tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass, including the existing `all_ten_harness_shortcuts_and_escape_precedence_work` and `ime_preedit_does_not_trigger_harness_shortcuts`.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/vibemux_frontend
git commit -m "feat(frontend): one shortcut table for Ctrl+N/B/,// and the shortcut overlay

Also fixes Ctrl+1 to Ctrl+0, which never fired with real Windows input because
the handler required modifiers exactly equal to CTRL or COMMAND.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: Quick search ranking (pure)

**Files:**
- Create: `crates/vibemux_frontend/src/quick_search.rs`
- Modify: `crates/vibemux_frontend/src/lib.rs` (`pub mod quick_search;`)

**Interfaces:**
- Produces: `quick_search::{MAX_RESULTS: usize = 8, SearchAction, SearchTarget, SearchEntry, rank(&str, &[SearchEntry], usize) -> Vec<usize>, build_entries(&SupervisorSnapshot, &ViewModel) -> Vec<SearchEntry>}` with `SearchAction { NewTask, OpenSettings, OpenDiagnostics, AppearanceLight, AppearanceDark, AppearanceMatchSystem, KeyboardShortcuts, ToggleSidebar }` and `SearchTarget { Task(String), Agent(usize), Action(SearchAction) }`.

- [ ] **Step 1: Write the failing tests**

Create `quick_search.rs` with the header and tests:

```rust
#![forbid(unsafe_code)]
//! Pure matching and ranking for the `Ctrl+K` quick switcher. No egui types.

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str, detail: &str) -> SearchEntry {
        SearchEntry {
            label: label.to_string(),
            detail: detail.to_string(),
            keywords: Vec::new(),
            target: SearchTarget::Action(SearchAction::NewTask),
        }
    }

    #[test]
    fn prefix_beats_word_start_beats_substring() {
        let entries = [entry("Subtask list", ""), entry("Verify task", ""), entry("Task review", "")];
        assert_eq!(rank("task", &entries, MAX_RESULTS), vec![2, 1, 0]);
    }

    #[test]
    fn ties_keep_source_order() {
        let entries = [entry("Alpha task", ""), entry("Beta task", "")];
        assert_eq!(rank("task", &entries, MAX_RESULTS), vec![0, 1]);
    }

    #[test]
    fn cjk_titles_match_by_substring() {
        let entries = [entry("Build the supervisor workspace · 主控工作台", "")];
        assert_eq!(rank("工作", &entries, MAX_RESULTS), vec![0]);
    }

    #[test]
    fn task_ids_match_through_the_detail() {
        let entries = [entry("Verify ownership", "task_ui · Codex")];
        assert_eq!(rank("task_ui", &entries, MAX_RESULTS), vec![0]);
    }

    #[test]
    fn matching_is_unicode_case_insensitive() {
        let entries = [entry("école", "")];
        assert_eq!(rank("ÉCOLE", &entries, MAX_RESULTS), vec![0]);
    }

    #[test]
    fn empty_query_lists_entries_in_order_up_to_the_limit() {
        let entries: Vec<SearchEntry> = (0..12).map(|index| entry(&format!("Entry {index}"), "")).collect();
        assert_eq!(rank("  ", &entries, MAX_RESULTS), (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn results_are_capped_and_misses_are_empty() {
        let entries: Vec<SearchEntry> = (0..20).map(|index| entry(&format!("task {index}"), "")).collect();
        assert_eq!(rank("task", &entries, MAX_RESULTS).len(), MAX_RESULTS);
        assert!(rank("zzz", &entries, MAX_RESULTS).is_empty());
    }

    #[test]
    fn entries_list_tasks_then_actions_then_agents() {
        let snapshot = crate::SupervisorSnapshot {
            tasks: vec![crate::TaskView {
                task_id: "task_1".to_string(),
                title: String::new(),
                ..crate::TaskView::default()
            }],
            ..crate::SupervisorSnapshot::default()
        };
        let view_model = crate::view_model::ViewModel::default();
        let entries = build_entries(&snapshot, &view_model);
        assert_eq!(entries[0].label, "Untitled task");
        assert_eq!(entries[0].target, SearchTarget::Task("task_1".to_string()));
        assert_eq!(entries[1].target, SearchTarget::Action(SearchAction::NewTask));
    }
}
```

If `ViewModel` has no `Default`, build it the way `test_view_model()` in `supervisor_app.rs` does, with `agents: Vec::new()`.

Add `pub mod quick_search;` to `lib.rs`.

Run: `cargo test -p vibemux_frontend --lib quick_search`
Expected: compile errors.

- [ ] **Step 2: Implement**

Insert above the tests:

```rust
use crate::{supervisor_model::SupervisorSnapshot, view_model::ViewModel};

pub const MAX_RESULTS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchAction {
    NewTask,
    OpenSettings,
    OpenDiagnostics,
    AppearanceLight,
    AppearanceDark,
    AppearanceMatchSystem,
    KeyboardShortcuts,
    ToggleSidebar,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchTarget {
    Task(String),
    Agent(usize),
    Action(SearchAction),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchEntry {
    pub label: String,
    pub detail: String,
    pub keywords: Vec<String>,
    pub target: SearchTarget,
}

const ACTIONS: [(SearchAction, &str, &[&str]); 8] = [
    (SearchAction::NewTask, "New task", &["draft", "compose"]),
    (SearchAction::OpenSettings, "Settings", &["preferences", "theme", "window"]),
    (SearchAction::OpenDiagnostics, "Diagnostics", &["probe", "health"]),
    (SearchAction::AppearanceLight, "Appearance: Light", &["theme", "claude"]),
    (SearchAction::AppearanceDark, "Appearance: Dark", &["theme", "claude"]),
    (SearchAction::AppearanceMatchSystem, "Appearance: Match system", &["theme", "os", "auto"]),
    (SearchAction::KeyboardShortcuts, "Keyboard shortcuts", &["keys", "help"]),
    (SearchAction::ToggleSidebar, "Toggle sidebar", &["collapse", "expand"]),
];

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum MatchStrength {
    Substring,
    WordStart,
    Prefix,
}

fn label_strength(query: &str, label: &str) -> Option<MatchStrength> {
    let label = label.to_lowercase();
    if label.starts_with(query) {
        return Some(MatchStrength::Prefix);
    }
    let word_start = label.char_indices().any(|(index, _)| {
        index > 0
            && label[..index]
                .chars()
                .next_back()
                .is_some_and(|previous| !previous.is_alphanumeric())
            && label[index..].starts_with(query)
    });
    if word_start {
        return Some(MatchStrength::WordStart);
    }
    label.contains(query).then_some(MatchStrength::Substring)
}

fn entry_strength(query: &str, entry: &SearchEntry) -> Option<MatchStrength> {
    let other = std::iter::once(&entry.detail)
        .chain(&entry.keywords)
        .any(|text| text.to_lowercase().contains(query))
        .then_some(MatchStrength::Substring);
    label_strength(query, &entry.label).max(other)
}

/// Indexes of the best matches, strongest first, ties in source order.
#[must_use]
pub fn rank(query: &str, entries: &[SearchEntry], limit: usize) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..entries.len().min(limit)).collect();
    }
    let mut scored: Vec<(MatchStrength, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry_strength(&query, entry).map(|strength| (strength, index)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, index)| index).collect()
}

/// Tasks first (snapshot order), then actions, then agents.
#[must_use]
pub fn build_entries(snapshot: &SupervisorSnapshot, view_model: &ViewModel) -> Vec<SearchEntry> {
    let tasks = snapshot.tasks.iter().map(|task| SearchEntry {
        label: if task.title.trim().is_empty() {
            "Untitled task".to_string()
        } else {
            task.title.clone()
        },
        detail: format!("{} · {}", task.task_id, task.executor),
        keywords: vec![task.state.clone()],
        target: SearchTarget::Task(task.task_id.clone()),
    });
    let actions = ACTIONS.iter().map(|(action, label, keywords)| SearchEntry {
        label: (*label).to_string(),
        detail: "Action".to_string(),
        keywords: keywords.iter().map(|keyword| (*keyword).to_string()).collect(),
        target: SearchTarget::Action(*action),
    });
    let agents = view_model.agents.iter().enumerate().map(|(index, agent)| SearchEntry {
        label: agent.name.clone(),
        detail: "Agent diagnostics".to_string(),
        keywords: Vec::new(),
        target: SearchTarget::Agent(index),
    });
    tasks.chain(actions).chain(agents).collect()
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p vibemux_frontend --lib quick_search`
Expected: 8 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/vibemux_frontend/src/quick_search.rs crates/vibemux_frontend/src/lib.rs
git commit -m "feat(frontend): pure quick search ranking over tasks, actions, and agents

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: Quick switcher popup and Ctrl+K

**Files:**
- Create: `crates/vibemux_frontend/src/gui/quick_switcher.rs`
- Modify: `crates/vibemux_frontend/src/gui/mod.rs` (`mod quick_switcher;`)
- Modify: `crates/vibemux_frontend/src/gui/shortcuts.rs` (Ctrl+K row and action)
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs` (state, rendering, actions)

**Interfaces:**
- Consumes: `quick_search::{rank, build_entries, MAX_RESULTS, SearchEntry, SearchTarget, SearchAction}`, `design::{icon, CARD_RADIUS, BUTTON_RADIUS}`, `SupervisorApp::{select_task, request_new_task, set_appearance_mode, set_sidebar_collapsed}`.
- Adds: `Icon::Search` and `design::popup_frame(&C) -> egui::Frame`.
- Produces: `quick_switcher::{QuickSwitcherState, SWITCHER_INPUT_ID, render(&egui::Context, &C, &mut QuickSwitcherState, &[SearchEntry]) -> Option<SearchTarget>}`; `ShortcutAction::QuickSwitcher`; `SupervisorApp::open_quick_switcher(&mut self, &str)`.

- [ ] **Step 1: Write the failing tests**

Create `quick_switcher.rs` with the header and state tests:

```rust
#![forbid(unsafe_code)]
//! `Ctrl+K` quick switcher popup over `quick_search` results.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_clamps_to_the_results() {
        let mut state = QuickSwitcherState::default();
        state.open_with("");
        state.move_selection(1, 3);
        state.move_selection(1, 3);
        state.move_selection(1, 3);
        assert_eq!(state.selected, 2);
        state.move_selection(-5, 3);
        assert_eq!(state.selected, 0);
        state.move_selection(1, 0);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn closing_resets_the_query() {
        let mut state = QuickSwitcherState::default();
        state.open_with("verify");
        state.close();
        assert!(!state.open);
        assert!(state.query.is_empty());
    }
}
```

In the `supervisor_app.rs` tests add:

```rust
    #[test]
    fn enter_in_the_switcher_opens_the_matching_task_drawer() {
        let mut app = test_app();
        app.open_quick_switcher("second");
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| app.render_frame(context));
        let raw = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = context.run(raw, |context| app.render_frame(context));
        let state = app.ui_state.lock().unwrap();
        assert!(state.details_open());
        assert_eq!(state.selected_task_id(), Some("task_2"));
        drop(state);
        assert!(!app.quick_switcher.open);
    }

    #[test]
    fn ctrl_k_opens_the_switcher() {
        let mut app = test_app();
        key_event(&egui::Context::default(), &mut app, Key::K, true, false);
        assert!(app.quick_switcher.open);
    }
```

Run: `cargo test -p vibemux_frontend --lib -- selection_clamps closing_resets switcher ctrl_k`
Expected: compile errors.

- [ ] **Step 2: Implement the switcher**

In `design.rs`, add `Search` to `pub enum Icon`, this arm to the `match kind` in `icon`:

```rust
        Icon::Search => {
            painter.circle_stroke(position(0.42, 0.42), rect.width() * 0.3, stroke);
            painter.line_segment([position(0.64, 0.64), position(0.92, 0.92)], stroke);
        }
```

and below `composer_frame`:

```rust
pub fn popup_frame(c: &C) -> egui::Frame {
    egui::Frame::new()
        .fill(c.raised)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(CARD_RADIUS)
        .inner_margin(egui::Margin::same(12))
        .shadow(egui::Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: shadow_color(c),
        })
}
```

Insert above the tests in `quick_switcher.rs`:

```rust
use eframe::egui::{self, Align2, Id, Key, Modifiers, RichText, Sense, Ui, vec2};

use super::{
    C,
    design::{self, Icon},
};
use crate::quick_search::{self, SearchEntry, SearchTarget};

pub const SWITCHER_INPUT_ID: &str = "quick_switcher_input";
const SWITCHER_WIDTH: f32 = 560.0;
const SWITCHER_TOP_OFFSET: f32 = 96.0;
const RESULT_ROW_HEIGHT: f32 = 40.0;

#[derive(Clone, Debug, Default)]
pub struct QuickSwitcherState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    focus_requested: bool,
}

impl QuickSwitcherState {
    pub fn open_with(&mut self, query: &str) {
        self.open = true;
        self.query = query.to_string();
        self.selected = 0;
        self.focus_requested = true;
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    pub fn move_selection(&mut self, delta: isize, result_count: usize) {
        if result_count == 0 {
            self.selected = 0;
            return;
        }
        let last = result_count - 1;
        self.selected = if delta < 0 {
            self.selected.saturating_sub(delta.unsigned_abs())
        } else {
            self.selected.saturating_add(delta.unsigned_abs()).min(last)
        };
    }
}

/// Render the popup; returns the target chosen with Enter or a click.
pub fn render(
    ctx: &egui::Context,
    c: &C,
    state: &mut QuickSwitcherState,
    entries: &[SearchEntry],
) -> Option<SearchTarget> {
    if !state.open {
        return None;
    }
    let results = quick_search::rank(&state.query, entries, quick_search::MAX_RESULTS);
    let (up, down, enter, escape) = ctx.input_mut(|input| {
        (
            input.consume_key(Modifiers::NONE, Key::ArrowUp),
            input.consume_key(Modifiers::NONE, Key::ArrowDown),
            input.consume_key(Modifiers::NONE, Key::Enter),
            input.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if escape {
        state.close();
        return None;
    }
    if up {
        state.move_selection(-1, results.len());
    }
    if down {
        state.move_selection(1, results.len());
    }
    let mut chosen = if enter {
        results.get(state.selected).map(|&index| entries[index].target.clone())
    } else {
        None
    };
    egui::Area::new(Id::new("quick_switcher"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_TOP, vec2(0.0, SWITCHER_TOP_OFFSET))
        .show(ctx, |ui| {
            design::popup_frame(c).show(ui, |ui| {
                ui.set_width(SWITCHER_WIDTH);
                ui.horizontal(|ui| {
                    design::icon(ui, Icon::Search, c.muted, 18.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut state.query)
                            .id(Id::new(SWITCHER_INPUT_ID))
                            .hint_text("Search tasks, agents, and actions")
                            .frame(false)
                            .desired_width(f32::INFINITY),
                    );
                    if state.focus_requested {
                        response.request_focus();
                        state.focus_requested = false;
                    }
                    if response.changed() {
                        state.selected = 0;
                    }
                });
                ui.separator();
                if results.is_empty() {
                    ui.label(RichText::new("No matches").size(13.0).color(c.muted));
                }
                for (row, &index) in results.iter().enumerate() {
                    let entry = &entries[index];
                    if result_row(ui, c, entry, row == state.selected).clicked() {
                        chosen = Some(entry.target.clone());
                    }
                }
            });
        });
    if chosen.is_some() {
        state.close();
    }
    chosen
}

fn result_row(ui: &mut Ui, c: &C, entry: &SearchEntry, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), RESULT_ROW_HEIGHT), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(rect, design::BUTTON_RADIUS, c.accent_bg);
    }
    ui.painter().text(
        egui::pos2(rect.left() + 10.0, rect.center().y - 8.0),
        Align2::LEFT_CENTER,
        &entry.label,
        egui::FontId::proportional(14.0),
        c.txt,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 10.0, rect.center().y + 9.0),
        Align2::LEFT_CENTER,
        &entry.detail,
        egui::FontId::proportional(11.5),
        c.muted,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &entry.label));
    response
}
```

Add `mod quick_switcher;` to `gui/mod.rs`.

- [ ] **Step 3: Add Ctrl+K and wire the app**

In `shortcuts.rs`, add `QuickSwitcher` as the first variant of `ShortcutAction` and this row as the first element of `SHORTCUTS`:

```rust
    ShortcutSpec {
        action: ShortcutAction::QuickSwitcher,
        keys: "Ctrl+K",
        description: "Search tasks, agents, and actions",
        chord: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::K)),
    },
```

In `supervisor_app.rs`, add the field `quick_switcher: quick_switcher::QuickSwitcherState` (initialize with `Default::default()` in `new` and `test_app`), add `quick_switcher` to the `use super::{...}` list, and:

```rust
    pub fn open_quick_switcher(&mut self, query: &str) {
        self.quick_switcher.open_with(query);
    }

    fn run_search_target(&mut self, target: crate::quick_search::SearchTarget) {
        use crate::{quick_search::{SearchAction, SearchTarget}, theme::appearance::AppearanceMode};
        match target {
            SearchTarget::Task(task_id) => {
                self.select_task(&task_id);
            }
            SearchTarget::Agent(index) => {
                if let Ok(mut state) = self.ui_state.lock() {
                    state.select_agent(index, self.view_model.agents.len());
                }
            }
            SearchTarget::Action(action) => match action {
                SearchAction::NewTask => self.request_new_task(),
                SearchAction::OpenSettings => self.settings_open = true,
                SearchAction::OpenDiagnostics => self.diagnostics_open = true,
                SearchAction::AppearanceLight => self.set_appearance_mode(AppearanceMode::Light),
                SearchAction::AppearanceDark => self.set_appearance_mode(AppearanceMode::Dark),
                SearchAction::AppearanceMatchSystem => {
                    self.set_appearance_mode(AppearanceMode::MatchSystem);
                }
                SearchAction::KeyboardShortcuts => self.shortcut_overlay_open = true,
                SearchAction::ToggleSidebar => {
                    self.set_sidebar_collapsed(!self.user_config.sidebar_collapsed);
                }
            },
        }
    }
```

Add the arm `shortcuts::ShortcutAction::QuickSwitcher => self.open_quick_switcher(""),` to `run_shortcut`. In `handle_keyboard`'s `Escape` branch, after the shortcut overlay check, add:

```rust
            if self.quick_switcher.open {
                self.quick_switcher.close();
                return;
            }
```

In `render_frame`, after the shortcut overlay call, add:

```rust
        if self.quick_switcher.open {
            let entries = crate::quick_search::build_entries(&snapshot, &self.view_model);
            if let Some(target) = quick_switcher::render(ctx, &colors, &mut self.quick_switcher, &entries) {
                self.run_search_target(target);
            }
        }
```

- [ ] **Step 4: Run the tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_frontend
git commit -m "feat(frontend): Ctrl+K quick switcher over tasks, actions, and agents

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 17: Spark animation for running tasks

**Files:**
- Create: `crates/vibemux_frontend/src/gui/motion.rs`
- Modify: `crates/vibemux_frontend/src/gui/mod.rs` (`mod motion;`)
- Modify: `crates/vibemux_frontend/src/gui/design.rs` (`status` gains `spark_angle`)
- Modify: `crates/vibemux_frontend/src/gui/chat.rs`, `crates/vibemux_frontend/src/gui/task_detail.rs`, `crates/vibemux_frontend/src/gui/sidebar.rs`, `crates/vibemux_frontend/src/gui/supervisor_app.rs`

**Interfaces:**
- Consumes: `design::spark`, `UserConfig.reduce_motion`.
- Produces: `motion::{SPARK_TURN_SECONDS, ANIMATION_REPAINT_INTERVAL, spark_angle(f64, bool) -> f32, animation_repaint_interval(bool, bool, bool) -> Option<Duration>, is_running_state(&str) -> bool}`; `design::status(&mut Ui, &C, &str, f32)`; `chat::render_conversation(.., spark_angle: f32)`; `task_detail::render(.., spark_angle: f32)` inserted before `title`; `SidebarView.spark_angle: f32`.

- [ ] **Step 1: Write the failing tests**

Create `motion.rs`:

```rust
#![forbid(unsafe_code)]
//! Spark animation timing, as pure functions over egui's input time.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_turns_once_every_period() {
        assert!(spark_angle(0.0, false).abs() < 1e-6);
        assert!((spark_angle(SPARK_TURN_SECONDS / 2.0, false) - std::f32::consts::PI).abs() < 1e-4);
        assert!(spark_angle(SPARK_TURN_SECONDS, false).abs() < 1e-4);
    }

    #[test]
    fn reduce_motion_keeps_the_spark_still() {
        assert_eq!(spark_angle(1.3, true), 0.0);
    }

    #[test]
    fn repaints_only_while_animating_in_a_focused_window() {
        assert_eq!(animation_repaint_interval(true, true, false), Some(ANIMATION_REPAINT_INTERVAL));
        assert_eq!(animation_repaint_interval(true, false, false), None);
        assert_eq!(animation_repaint_interval(false, true, false), None);
        assert_eq!(animation_repaint_interval(true, true, true), None);
    }

    #[test]
    fn running_states_are_recognized() {
        assert!(is_running_state("running"));
        assert!(is_running_state("in_progress"));
        assert!(!is_running_state("blocked"));
    }
}
```

Add `mod motion;` to `gui/mod.rs`.

Run: `cargo test -p vibemux_frontend --lib motion`
Expected: compile errors.

- [ ] **Step 2: Implement the timing functions**

Insert above the tests:

```rust
use std::time::Duration;

pub const SPARK_TURN_SECONDS: f64 = 2.4;
pub const ANIMATION_REPAINT_INTERVAL: Duration = Duration::from_millis(50);

#[must_use]
pub fn spark_angle(time_seconds: f64, reduce_motion: bool) -> f32 {
    if reduce_motion {
        return 0.0;
    }
    let turns = (time_seconds / SPARK_TURN_SECONDS).fract();
    (turns * std::f64::consts::TAU) as f32
}

/// Extra repaint cadence while a spark animates; `None` leaves the app's
/// existing 100 ms refresh in charge.
#[must_use]
pub fn animation_repaint_interval(
    running_task_visible: bool,
    window_focused: bool,
    reduce_motion: bool,
) -> Option<Duration> {
    (running_task_visible && window_focused && !reduce_motion).then_some(ANIMATION_REPAINT_INTERVAL)
}

#[must_use]
pub fn is_running_state(state: &str) -> bool {
    matches!(state, "in_progress" | "running")
}
```

Run: `cargo test -p vibemux_frontend --lib motion`
Expected: 4 passed. The `spark_angle(SPARK_TURN_SECONDS, false)` case can land just below `TAU` because of float rounding; if it fails, assert `angle.abs() < 1e-4 || (angle - TAU).abs() < 1e-4` instead.

- [ ] **Step 3: Animate running markers**

In `design.rs`, change `status` to:

```rust
pub fn status(ui: &mut Ui, c: &C, state: &str, spark_angle: f32) {
    let color = state_color(c, state);
    ui.horizontal(|ui| {
        if super::motion::is_running_state(state) {
            spark(ui, color, 11.0, spark_angle);
        } else {
            let (rect, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 3.0, color);
        }
        ui.label(RichText::new(state_text(state)).size(12.0).color(state_text_color(c, state)));
    });
}
```

Add a `spark_angle: f32` parameter to `chat::render_conversation` and `task_row`/`task_row_contents` (pass it to `design::status`), to `task_detail::render` (inserted before `title`, passed to its `design::status` call), and `pub spark_angle: f32` to `SidebarView`; in `recent_row`, draw `design::paint_spark(ui.painter(), egui::pos2(rect.left() + 9.0, rect.center().y), 5.0, spark_angle, design::state_color(c, state))` instead of the dot when `super::motion::is_running_state(state)`.

In `render_frame`, compute after `let colors = ...`:

```rust
        let spark_angle = super::motion::spark_angle(ctx.input(|input| input.time), self.user_config.reduce_motion);
```

pass it to the sidebar view and `render_conversation`, and add a `spark_angle: f32` parameter to `render_drawer` and `drawer_contents` so the drawer's `task_detail::render` call receives it. In `render_task_viewports`, capture `let reduce_motion = self.user_config.reduce_motion;` before the loop and compute `super::motion::spark_angle(child_ctx.input(|input| input.time), reduce_motion)` inside the viewport closure. At the end of `render_frame`, before the existing `request_repaint_after`, add:

```rust
        let running_visible = snapshot.tasks.iter().any(|task| super::motion::is_running_state(&task.state));
        if let Some(interval) = super::motion::animation_repaint_interval(
            running_visible,
            ctx.input(|input| input.focused),
            self.user_config.reduce_motion,
        ) {
            ctx.request_repaint_after(interval);
        }
```

- [ ] **Step 4: Run the tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_frontend
git commit -m "feat(frontend): rotating spark for running tasks with Reduce motion

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 18: Copy buttons

**Files:**
- Modify: `crates/vibemux_frontend/src/gui/design.rs` (`copy_button`, `copied_feedback_visible`)
- Modify: `crates/vibemux_frontend/src/gui/chat.rs` (copy task title)
- Modify: `crates/vibemux_frontend/src/gui/task_detail.rs` (grid copy column)
- Test: `crates/vibemux_frontend/src/gui/supervisor_app.rs`

**Interfaces:**
- Consumes: `design::icon_button`.
- Adds: `Icon::{Copy, Check}` in `design.rs`.
- Produces: `design::{COPIED_FEEDBACK_SECONDS: f64 = 1.5, copied_feedback_visible(Option<f64>, f64) -> bool, copy_button(&mut Ui, &C, &str, egui::Id) -> Response}`; `task_detail::DetailRow<'a> { label: &'a str, value: String, copy_id: Option<egui::Id> }`; `task_detail::copy_id(kind: &str, value: &str) -> egui::Id`.

- [ ] **Step 1: Write the failing tests**

Add to the `design.rs` tests:

```rust
    #[test]
    fn copied_feedback_lasts_one_and_a_half_seconds() {
        assert!(!copied_feedback_visible(None, 5.0));
        assert!(copied_feedback_visible(Some(5.0), 5.0));
        assert!(copied_feedback_visible(Some(5.0), 6.4));
        assert!(!copied_feedback_visible(Some(5.0), 6.5));
        assert!(!copied_feedback_visible(Some(5.0), 4.0));
    }
```

Add to the `supervisor_app.rs` tests:

```rust
    #[test]
    fn copy_button_copies_the_task_id() {
        let mut app = test_app();
        assert!(app.select_task("task_1"));
        let context = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
        let frame = |events: Vec<egui::Event>, time: f64| egui::RawInput {
            screen_rect: Some(screen),
            events,
            time: Some(time),
            ..Default::default()
        };
        let _ = context.run(frame(Vec::new(), 0.0), |context| app.render_frame(context));
        let _ = context.run(frame(Vec::new(), 0.1), |context| app.render_frame(context));
        let id = task_detail::copy_id("task", "task_1");
        let target = context.read_response(id).expect("copy button").rect.center();
        let press = |pressed: bool| egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = context.run(frame(vec![egui::Event::PointerMoved(target), press(true)], 0.2), |context| {
            app.render_frame(context)
        });
        let output = context.run(frame(vec![press(false)], 0.3), |context| app.render_frame(context));
        assert!(output.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if text == "task_1")
        }));
    }
```

The drawer shows the "Identity & references" section collapsed by default; the copy button for the Task ID is therefore placed in the always-visible run details grid (see Step 3), keyed by `copy_id("task", task_id)`.

Run: `cargo test -p vibemux_frontend --lib -- copied_feedback copy_button_copies`
Expected: compile errors.

- [ ] **Step 2: Implement the copy button**

In `design.rs`, add `Copy` and `Check` to `pub enum Icon` and these arms to the `match kind` in `icon`:

```rust
        Icon::Copy => {
            let back = egui::Rect::from_min_max(position(0.3, 0.0), position(1.0, 0.7));
            let front = egui::Rect::from_min_max(position(0.0, 0.3), position(0.7, 1.0));
            painter.rect_stroke(back, 2, stroke, egui::StrokeKind::Inside);
            painter.rect_stroke(front, 2, stroke, egui::StrokeKind::Inside);
        }
        Icon::Check => {
            painter.line_segment([position(0.15, 0.55), position(0.4, 0.8)], stroke);
            painter.line_segment([position(0.4, 0.8), position(0.88, 0.22)], stroke);
        }
```

Add to `design.rs`:

```rust
pub const COPIED_FEEDBACK_SECONDS: f64 = 1.5;

#[must_use]
pub fn copied_feedback_visible(copied_at: Option<f64>, now: f64) -> bool {
    copied_at.is_some_and(|at| now >= at && now - at < COPIED_FEEDBACK_SECONDS)
}

/// Copy `text` to the clipboard on click and show "Copied" for 1.5 s.
pub fn copy_button(ui: &mut Ui, c: &C, text: &str, id: egui::Id) -> Response {
    let now = ui.input(|input| input.time);
    let copied_at: Option<f64> = ui.data(|data| data.get_temp(id));
    let showing = copied_feedback_visible(copied_at, now);
    let response = icon_button(
        ui,
        c,
        if showing { Icon::Check } else { Icon::Copy },
        if showing { "Copied" } else { "Copy" },
        id,
    );
    if response.clicked() {
        ui.ctx().copy_text(text.to_string());
        ui.data_mut(|data| data.insert_temp(id, now));
    }
    if showing {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
    response
}
```

- [ ] **Step 3: Place the buttons**

In `task_detail.rs`, replace `detail_grid` with a three-column version:

```rust
pub struct DetailRow<'a> {
    pub label: &'a str,
    pub value: String,
    pub copy_id: Option<egui::Id>,
}

/// Stable id for a copy button, unique per copied value.
#[must_use]
pub fn copy_id(kind: &str, value: &str) -> egui::Id {
    egui::Id::new(("detail_copy", kind, value))
}

fn detail_grid(ui: &mut Ui, colors: &C, id: &str, rows: &[DetailRow<'_>]) {
    let value_width = (ui.available_width() - DETAIL_LABEL_WIDTH - 60.0).max(80.0);
    egui::Grid::new(id)
        .num_columns(3)
        .spacing([12.0, 5.0])
        .min_col_width(DETAIL_LABEL_WIDTH.min(30.0))
        .max_col_width(value_width)
        .show(ui, |ui| {
            for row in rows {
                ui.label(RichText::new(row.label).size(12.0).color(colors.muted));
                wrapped_label(ui, RichText::new(row.value.as_str()).size(12.0).color(colors.txt).monospace());
                match row.copy_id {
                    Some(copy_id) => {
                        design::copy_button(ui, colors, &row.value, copy_id);
                    }
                    None => {
                        ui.label("");
                    }
                }
                ui.end_row();
            }
        });
}
```

In `render_overview`, build the run grid rows with `DetailRow` values: `Harness`, `Role`, and `Run state` get `copy_id: None`; add a first row `DetailRow { label: "Task", value: task.task_id.clone(), copy_id: Some(copy_id("task", &task.task_id)) }`, and give `Branch` and `Worktree` `Some(copy_id("branch", &run.branch))` and `Some(copy_id("worktree", &run.worktree))` when the value is recorded (use `None` for the "Not recorded" fallback). In the "Identity & references" grid, give the Run row `Some(copy_id("run", &run.run_id))` and drop the Task row there (it moved to the top grid). When there is no run, still render a one-row grid with the Task row so the Task ID stays copyable.

In `chat.rs` `task_row_contents`, wrap the title button in `ui.horizontal(|ui| { ... })` and add after it:

```rust
        design::copy_button(ui, c, title, egui::Id::new(("task_title_copy", task.task_id.as_str())));
```

- [ ] **Step 4: Run the tests and clippy**

Run: `cargo test -p vibemux_frontend --all-features`
Expected: all pass, including `copy_button_copies_the_task_id`.

Run: `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_frontend
git commit -m "feat(frontend): copy buttons for task titles, IDs, branches, and worktrees

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 19: Phase 2 previews, docs, verification, and pull request

**Files:**
- Modify: `crates/vibemux_frontend/src/bin/vibemux_frontend_preview.rs` (`switcher`, `shortcuts` scenarios)
- Modify: `README.md`, `CHANGELOG.md`, `PROGRESS.md`, `docs/adr/030_claude_style_gui.md` (status)
- Modify: `docs/frontend_previews/*.png` (regenerated), add `supervisor_switcher.png`, `supervisor_shortcuts.png`

**Interfaces:**
- Consumes: `SupervisorApp::{open_quick_switcher, open_shortcut_overlay}`.

- [ ] **Step 1: Add the scenarios**

Extend the scenario `match` in the preview binary:

```rust
                "switcher" => app.open_quick_switcher("verify"),
                "shortcuts" => app.open_shortcut_overlay(),
```

- [ ] **Step 2: Capture and review screenshots**

Run the Task 12 Step 2 loop with scenarios `chat drawer welcome collapsed switcher shortcuts` and both Claude themes. Also capture `chat claude 1280 800 1` with Reduce motion on by temporarily editing nothing: Reduce motion only changes the spark's rotation, which a still screenshot cannot show, so check it in Step 3 instead. Expected: switcher popup centered near the top with the search icon and ranked rows; shortcut overlay listing all seven rows; appearance control visible in Settings (open it with the `settings` path manually in the live GUI if needed).

- [ ] **Step 3: Live check in the real GUI**

Run: `cargo run -p vibemux_frontend --bin vibemux_frontend`
Check by hand, then close the window: `Ctrl+K` opens the switcher and Enter opens a task; `Ctrl+N`, `Ctrl+B`, `Ctrl+,`, `Ctrl+/` work while typing in the composer; Settings → Appearance → Match system follows the Windows light/dark setting when toggled; Reduce motion stops the spark; a copy button puts the text on the clipboard. Record what was checked in `PROGRESS.md`. Do not click Send or submit anything to a harness.

- [ ] **Step 4: Update the docs**

In `README.md`, after the GUI paragraph, add:

```text
Keyboard shortcuts: `Ctrl+K` quick switcher (tasks, agents, and actions),
`Ctrl+N` new task, `Ctrl+B` collapse or expand the sidebar, `Ctrl+,` Settings,
`Ctrl+/` shortcut overlay, `Esc` closes the top-most overlay, and `Ctrl+1` to
`Ctrl+0` open agent diagnostics. **Settings → Appearance** offers Light, Dark,
and Match system for the Claude themes, plus Reduce motion.
```

and add `switcher` and `shortcuts` to the scene list. Add a `CHANGELOG.md` "Unreleased" entry for phase 2 (shortcut table and overlay, quick switcher, appearance modes with Match system, Reduce motion, spark animation, copy buttons, and the fix for `Ctrl+1` to `Ctrl+0`, which never fired with real Windows input). Add a `PROGRESS.md` entry for phase 2 in the same structure as Task 12's entry, and replace the ADR 030 status paragraph with:

```text
Status: Accepted and implemented (phase 1 and phase 2). PROGRESS.md records
the evidence for each phase.
```

- [ ] **Step 5: Run the full gates**

Run the same command list as Task 12 Step 5. Expected: every command exits 0; record the counts in `PROGRESS.md`.

- [ ] **Step 6: Commit, push, and open the pull request**

Follow Task 12 Step 6 with branch `feat/claude_style_gui_phase_two`: stage only the intended files, run the privacy scan, commit with the attribution trailer, `ls-remote` before and after an explicit-refspec push, confirm the remote equals `HEAD`, open the pull request into `main`, and bind it with the ccd_pr tools.
