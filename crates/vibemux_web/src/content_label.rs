//! Sanitizers for the short labels the web API passes through from the
//! daemon: identifiers, codes, timestamps, and media types. They keep the
//! JSON content-free: path separators and free text never pass whole.

/// Characters kept by `web_label`.
pub const MAX_LABEL_CHARS: usize = 96;

/// Keeps identifier-like text only: ASCII letters, digits, and `_ - . : @ +`,
/// at most `MAX_LABEL_CHARS`. Separators of paths and free text (`/`, `\`,
/// spaces) never pass.
#[must_use]
pub fn web_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '.' | ':' | '@' | '+')
        })
        .take(MAX_LABEL_CHARS)
        .collect()
}

/// Keeps the characters of a date and time only: digits, `- : . +`, the
/// RFC 3339 `T` and `Z`, and the spaces of the store's display form
/// (`2026-10-22 8:02:50.7330309 +00:00:00`), at most `MAX_LABEL_CHARS`.
#[must_use]
pub fn timestamp_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_digit()
                || matches!(character, '-' | ':' | '.' | '+' | 'T' | 'Z' | ' ')
        })
        .take(MAX_LABEL_CHARS)
        .collect()
}

/// A `type/subtype` media type, or `unknown` for anything else.
#[must_use]
pub fn media_type_label(value: &str) -> String {
    let valid_part = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'+' | b'-')
            })
    };
    match value.split_once('/') {
        Some((kind, subtype))
            if value.len() <= MAX_LABEL_CHARS && valid_part(kind) && valid_part(subtype) =>
        {
            value.to_owned()
        }
        _ => "unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_drop_path_separators_and_free_text() {
        assert_eq!(web_label("aag:worker:gpt-5.1"), "aag:worker:gpt-5.1");
        assert_eq!(
            web_label(r"D:\work\operator\secret file"),
            "D:workoperatorsecretfile"
        );
        assert_eq!(web_label("a/b c\nd\u{1b}[31m"), "abcd31m");
        assert_eq!(web_label(&"x".repeat(200)).len(), MAX_LABEL_CHARS);
        assert_eq!(media_type_label("application/json"), "application/json");
        assert_eq!(media_type_label("text/plain; charset=utf-8"), "unknown");
        assert_eq!(media_type_label("../../etc/passwd"), "unknown");
        assert_eq!(media_type_label("Text/Plain"), "unknown");
    }

    #[test]
    fn timestamps_keep_their_spaces_and_nothing_else() {
        // The store's `OffsetDateTime` display form.
        let display = time::OffsetDateTime::from_unix_timestamp(1_792_656_170)
            .and_then(|time| time.replace_nanosecond(733_030_900))
            .unwrap()
            .to_string();
        assert!(display.contains(' '), "{display}");
        assert_eq!(timestamp_label(&display), display);
        assert_eq!(
            timestamp_label("2026-10-22T08:02:50Z"),
            "2026-10-22T08:02:50Z"
        );
        assert_eq!(timestamp_label(r"2026-10-22 /secret\path"), "2026-10-22 ");
    }
}
