#![forbid(unsafe_code)]
//! Shared display-text sanitization: every string that reaches a renderer
//! passes through [`safe_text`] first.

use vibemux_harness::terminal_text::display_character;

pub const MAX_DISPLAY_CHARACTERS: usize = 160;

/// One display line: every whitespace or terminal-unsafe character becomes a
/// space, and the text is cut at `MAX_DISPLAY_CHARACTERS`.
pub fn safe_text(value: &str) -> String {
    value
        .chars()
        .take(MAX_DISPLAY_CHARACTERS)
        .map(display_character)
        .collect()
}
