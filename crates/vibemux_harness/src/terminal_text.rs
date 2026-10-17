//! Characters a terminal must never receive raw from untrusted text, such as
//! a vendor CLI's `--version` output, a resolved launcher path, or a branch
//! name. One definition keeps the probe, the CLI's JSON output, and the
//! dashboard in agreement about what is unsafe.

/// Whether `character` can change how a terminal draws the text around it:
/// a C0 or C1 control or DEL (ESC starts an ANSI sequence), the Arabic
/// letter mark, a left-to-right or right-to-left mark, a line or paragraph
/// separator, or a bidirectional embedding, override, or isolate.
#[must_use]
pub fn is_terminal_unsafe(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

#[cfg(test)]
mod tests {
    use super::is_terminal_unsafe;

    #[test]
    fn controls_and_bidirectional_marks_are_unsafe() {
        for unsafe_character in [
            '\u{0}', '\t', '\n', '\u{1b}', '\u{7f}', '\u{9b}', '\u{061c}', '\u{200e}', '\u{200f}',
            '\u{2028}', '\u{2029}', '\u{202a}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2069}',
        ] {
            assert!(
                is_terminal_unsafe(unsafe_character),
                "U+{:04X}",
                u32::from(unsafe_character)
            );
        }
    }

    #[test]
    fn ordinary_text_and_neighbouring_code_points_are_safe() {
        // The zero-width joiner builds emoji sequences, and U+202F and
        // U+2065 sit just outside the unsafe ranges.
        for safe_character in [
            'a', ' ', '.', 'é', '中', '\u{200d}', '\u{202f}', '\u{2065}', '\u{206a}',
        ] {
            assert!(
                !is_terminal_unsafe(safe_character),
                "U+{:04X}",
                u32::from(safe_character)
            );
        }
    }
}
