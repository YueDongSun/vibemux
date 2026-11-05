//! Literal Unicode matching over one bounded retained title or record.

use regex::RegexBuilder;
use vibemux_types::chat::MAX_CHAT_RECORD_BYTES;

use super::{MAX_SEARCH_PREVIEW_CHARACTERS, MAX_SEARCH_QUERY_BYTES, SearchError, SearchPreview};

/// Context counts Unicode scalar values. Literal ranges remain the regex's
/// original UTF-8 byte ranges, including matches inside a grapheme cluster.
pub fn find_preview(
    text: &str,
    query: &str,
    case_sensitive: bool,
) -> Result<Option<SearchPreview>, SearchError> {
    if query.trim().is_empty() || query.len() > MAX_SEARCH_QUERY_BYTES {
        return Err(SearchError::InvalidRequest);
    }
    if u64::try_from(text.len()).map_err(|_| SearchError::Limit)? > MAX_CHAT_RECORD_BYTES {
        return Err(SearchError::Limit);
    }
    let expression = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(!case_sensitive)
        .unicode(true)
        .build()
        .map_err(|_| SearchError::Limit)?;
    let mut matches = expression.find_iter(text);
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    let mut match_count = 1_u32;
    for _ in matches {
        match_count = match_count.checked_add(1).ok_or(SearchError::Limit)?;
    }
    let start = first.start();
    let end = first.end();
    let prefix = text.get(..start).ok_or(SearchError::InvalidResponse)?;
    let suffix = text.get(end..).ok_or(SearchError::InvalidResponse)?;
    let context_start = prefix
        .char_indices()
        .rev()
        .take(MAX_SEARCH_PREVIEW_CHARACTERS)
        .last()
        .map_or(prefix.len(), |(index, _)| index);
    let suffix_bytes = suffix
        .char_indices()
        .nth(MAX_SEARCH_PREVIEW_CHARACTERS)
        .map_or(suffix.len(), |(index, _)| index);
    let context_end = end.checked_add(suffix_bytes).ok_or(SearchError::Limit)?;
    Ok(Some(SearchPreview {
        before: text
            .get(context_start..start)
            .ok_or(SearchError::InvalidResponse)?
            .to_owned(),
        matched: text
            .get(start..end)
            .ok_or(SearchError::InvalidResponse)?
            .to_owned(),
        after: text
            .get(end..context_end)
            .ok_or(SearchError::InvalidResponse)?
            .to_owned(),
        match_count,
        byte_offset: u32::try_from(start).map_err(|_| SearchError::Limit)?,
        end_byte_offset: u32::try_from(end).map_err(|_| SearchError::Limit)?,
        truncated: context_start != 0 || context_end != text.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn preview(text: &str, query: &str, case_sensitive: bool) -> SearchPreview {
        find_preview(text, query, case_sensitive)
            .expect("bounded literal query")
            .expect("match")
    }

    #[test]
    fn unicode_preview_preserves_original_text_and_reports_utf8_not_utf16_offsets() {
        let value = preview("😀中needle尾👩‍💻", "needle", true);
        assert_eq!(value.byte_offset, 7);
        assert_eq!(value.end_byte_offset, 13);
        assert_eq!("😀中".encode_utf16().count(), 3);
        assert_eq!(value.before, "😀中");
        assert_eq!(value.matched, "needle");
        assert_eq!(value.after, "尾👩‍💻");
        assert_eq!(value.match_count, 1);
        assert!(!value.truncated);
        let chinese = preview("前🙂中文👩‍💻後", "中文", true);
        assert_eq!(chinese.byte_offset, 7);
        assert_eq!(chinese.end_byte_offset, 13);
        assert_eq!(chinese.matched, "中文");
    }

    #[test]
    fn context_is_bounded_at_unicode_scalar_boundaries_on_both_sides() {
        let before = "中🙂".repeat(50);
        let after = "尾🚀".repeat(50);
        let text = format!("{before}needle{after}");
        let value = preview(&text, "needle", true);
        assert_eq!(value.before.chars().count(), MAX_SEARCH_PREVIEW_CHARACTERS);
        assert_eq!(value.after.chars().count(), MAX_SEARCH_PREVIEW_CHARACTERS);
        assert_eq!(value.before, before.chars().skip(20).collect::<String>());
        assert_eq!(value.after, after.chars().take(80).collect::<String>());
        assert_eq!(
            usize::try_from(value.byte_offset).expect("offset"),
            before.len()
        );
        assert!(value.truncated);
        for (prefix, suffix, truncated) in [(80, 80, false), (81, 80, true), (80, 81, true)] {
            let text = format!("{}x{}", "界".repeat(prefix), "🙂".repeat(suffix));
            let value = preview(&text, "x", true);
            assert_eq!(value.before.chars().count(), 80);
            assert_eq!(value.after.chars().count(), 80);
            assert_eq!(value.truncated, truncated);
        }
    }

    #[test]
    fn combining_marks_and_zwj_are_literal_without_normalizing_or_expanding_graphemes() {
        let text = "Cafe\u{301} 👩‍💻";
        let accent = preview(text, "\u{301}", true);
        assert_eq!(accent.before, "Cafe");
        assert_eq!(accent.matched, "\u{301}");
        assert_eq!(accent.after, " 👩‍💻");
        assert_eq!(accent.byte_offset, 4);
        assert_eq!(accent.end_byte_offset, 6);
        assert_eq!(find_preview(text, "Café", true), Ok(None));
        let joiner = preview(text, "\u{200d}", true);
        assert!(joiner.before.ends_with('👩'));
        assert_eq!(joiner.matched, "\u{200d}");
        assert_eq!(joiner.after, "💻");
        assert_eq!(preview(text, "👩‍💻", true).matched, "👩‍💻");
    }

    #[test]
    fn unicode_simple_casefold_matches_sigma_and_kelvin_but_keeps_original_spelling() {
        let sigma = preview("Σ σ ς", "σ", false);
        assert_eq!(sigma.matched, "Σ");
        assert_eq!(sigma.match_count, 3);
        assert_eq!(sigma.byte_offset, 0);
        assert_eq!(sigma.end_byte_offset, 2);
        let kelvin = preview("K k K", "k", false);
        assert_eq!(kelvin.matched, "K");
        assert_eq!(kelvin.match_count, 3);
        let original_kelvin = preview("K", "K", false);
        assert_eq!(original_kelvin.matched, "K");
        assert_eq!(original_kelvin.end_byte_offset, 3);
        assert_eq!(preview("Σ σ ς", "σ", true).match_count, 1);
        let sensitive = preview("K k K", "k", true);
        assert_eq!(sensitive.byte_offset, 2);
        assert_eq!(sensitive.match_count, 1);
        assert_eq!(find_preview("ABC", "abc", true), Ok(None));
    }

    #[test]
    fn simple_casefold_never_expands_a_single_character_into_multiple_characters() {
        let sharp_s = preview("ss SS ß ẞ", "ß", false);
        assert_eq!(sharp_s.matched, "ß");
        assert_eq!(sharp_s.match_count, 2);
        let double_s = preview("ss SS ß ẞ", "ss", false);
        assert_eq!(double_s.matched, "ss");
        assert_eq!(double_s.match_count, 2);
    }

    #[test]
    fn regex_metacharacters_and_control_syntax_remain_literal_query_data() {
        for query in [
            ".*",
            "[a-z]+",
            "(?i)",
            r"\p{L}",
            "^$",
            concat!("$", "{HOME}"),
            "$(echo literal)",
            "<script>",
        ] {
            let text = format!("before {query} after {query}");
            let value = preview(&text, query, true);
            assert_eq!(value.matched, query);
            assert_eq!(value.byte_offset, 7);
            assert_eq!(value.match_count, 2);
        }
        assert_eq!(preview("before a after", " a ", true).matched, " a ");
        assert_eq!(preview("A\nB", "A\nB", false).matched, "A\nB");
    }

    #[test]
    fn occurrences_are_nonoverlapping_and_first_preview_is_not_the_last_match() {
        let value = preview("aaaaa", "aa", true);
        assert_eq!(value.match_count, 2);
        assert_eq!(value.byte_offset, 0);
        assert_eq!(value.end_byte_offset, 2);
        assert_eq!(value.matched, "aa");
        assert_eq!(value.after, "aaa");
        assert!(!value.truncated);
        let value = preview("first needle then needle", "needle", true);
        assert_eq!(value.before, "first ");
        assert_eq!(value.match_count, 2);
    }

    #[test]
    fn query_bound_is_utf8_bytes_and_whitespace_only_queries_are_rejected() {
        for query in ["", " \n\t", "\u{2003}"] {
            assert_eq!(
                find_preview("text", query, false),
                Err(SearchError::InvalidRequest)
            );
        }
        let exact = "é".repeat(MAX_SEARCH_QUERY_BYTES / 2);
        assert_eq!(preview(&exact, &exact, true).matched, exact);
        let oversized = "é".repeat(MAX_SEARCH_QUERY_BYTES / 2 + 1);
        assert_eq!(
            find_preview(&oversized, &oversized, true),
            Err(SearchError::InvalidRequest)
        );
        assert_eq!(find_preview("", "query", true), Ok(None));
    }

    #[test]
    fn retention_cap_is_inclusive_and_counts_are_bounded_without_collecting_all_matches() {
        let maximum = usize::try_from(MAX_CHAT_RECORD_BYTES).expect("record bound");
        let text = "x".repeat(maximum);
        let value = preview(&text, "x", true);
        assert_eq!(usize::try_from(value.match_count).expect("count"), maximum);
        assert_eq!(value.before, "");
        assert_eq!(value.matched, "x");
        assert_eq!(value.after.len(), MAX_SEARCH_PREVIEW_CHARACTERS);
        assert!(value.truncated);
        assert_eq!(
            find_preview(&"x".repeat(maximum + 1), "x", true),
            Err(SearchError::Limit)
        );
        assert!(find_preview(&"t".repeat(512), "t", true).is_ok());
    }

    #[test]
    fn debug_is_redacted_and_the_serialized_result_contains_only_the_bounded_preview_contract() {
        let text = format!(
            "{}PRIVATE_BEFORE needle PRIVATE_AFTER{}",
            "x".repeat(1000),
            "y".repeat(1000)
        );
        let value = preview(&text, "needle", true);
        let debug = format!("{value:?}");
        for private in ["PRIVATE_BEFORE", "PRIVATE_AFTER", "needle"] {
            assert!(!debug.contains(private));
        }
        let encoded = serde_json::to_value(&value).expect("preview DTO");
        let mut fields: Vec<_> = encoded
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect();
        fields.sort();
        assert_eq!(
            fields,
            vec![
                "after",
                "before",
                "byte_offset",
                "end_byte_offset",
                "match_count",
                "matched",
                "truncated"
            ]
        );
        assert_eq!(encoded["matched"], json!("needle"));
        assert!(value.before.chars().count() <= 80 && value.after.chars().count() <= 80);
        assert!(value.before.len() + value.matched.len() + value.after.len() < text.len());
    }
}
