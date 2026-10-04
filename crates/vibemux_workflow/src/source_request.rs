//! The original request as immutable source text, and the deterministic
//! fact extraction the compiler validator checks a TaskSpec against.
//!
//! The extraction is intentionally simple and conservative: it finds
//! clause boundaries, numbers, prohibition markers, binding-force markers,
//! and literal tokens (backtick, quoted, and path-like text). A false
//! positive makes validation stricter (fail closed); it never admits a
//! candidate that the stricter reading would reject.

use serde::Serialize;
use thiserror::Error;

use crate::{Sha256Digest, task_spec::SourceSpan};

pub const MAX_SOURCE_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SourceError {
    #[error("request must be non-empty UTF-8 without NUL")]
    InvalidText,
    #[error("request exceeds its size limit")]
    TooLarge,
}

impl SourceError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidText => "workflow_request_invalid",
            Self::TooLarge => "workflow_request_too_large",
        }
    }
}

/// A half-open byte range of the request.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

impl ByteRange {
    #[must_use]
    pub const fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRequest {
    text: String,
    digest: Sha256Digest,
}

impl SourceRequest {
    pub fn new(bytes: Vec<u8>) -> Result<Self, SourceError> {
        if bytes.len() > MAX_SOURCE_REQUEST_BYTES {
            return Err(SourceError::TooLarge);
        }
        let digest = Sha256Digest::of(&bytes);
        let text = String::from_utf8(bytes).map_err(|_| SourceError::InvalidText)?;
        if text.trim().is_empty() || text.contains('\0') {
            return Err(SourceError::InvalidText);
        }
        Ok(Self { text, digest })
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn digest(&self) -> Sha256Digest {
        self.digest
    }

    /// The range a span claims, if it lies on character boundaries and its
    /// quoted text is exactly the request text there.
    #[must_use]
    pub fn resolve(&self, span: &SourceSpan) -> Option<ByteRange> {
        let start = usize::try_from(span.start).ok()?;
        let end = usize::try_from(span.end).ok()?;
        let slice = self.text.get(start..end)?;
        (start < end && slice == span.quoted).then_some(ByteRange { start, end })
    }

    /// Non-blank clauses, split at sentence punctuation that is followed by
    /// whitespace or the end of text, and at line breaks. Punctuation inside
    /// literal regions (backticks and quotes) never splits.
    #[must_use]
    pub fn clauses(&self) -> Vec<ByteRange> {
        let literal_regions = literal_regions(&self.text);
        let mut clauses = Vec::new();
        let mut start = 0;
        let mut characters = self.text.char_indices().peekable();
        while let Some((index, character)) = characters.next() {
            let inside_literal = literal_regions
                .iter()
                .any(|region| region.start <= index && index < region.end);
            let end = index + character.len_utf8();
            let next_is_space = characters
                .peek()
                .is_none_or(|(_, next)| next.is_whitespace());
            let splits = !inside_literal
                && match character {
                    '\n' | '。' | '；' | '！' | '？' => true,
                    '.' | ';' | '!' | '?' => next_is_space,
                    _ => false,
                };
            if splits {
                push_clause(&self.text, start, end, &mut clauses);
                start = end;
            }
        }
        push_clause(&self.text, start, self.text.len(), &mut clauses);
        clauses
    }

    /// Prohibition markers outside literal regions.
    #[must_use]
    pub fn prohibition_markers(&self) -> Vec<ByteRange> {
        let regions = literal_regions(&self.text);
        markers(&self.text, PROHIBITION_MARKERS)
            .into_iter()
            .filter(|marker| !regions.iter().any(|region| region.overlaps(*marker)))
            .collect()
    }

    #[must_use]
    pub fn literal_regions(&self) -> Vec<ByteRange> {
        literal_regions(&self.text)
    }
}

fn push_clause(text: &str, start: usize, end: usize, clauses: &mut Vec<ByteRange>) {
    let slice = &text[start..end];
    let leading = slice.len() - slice.trim_start().len();
    let trailing = slice.len() - slice.trim_end().len();
    if leading + trailing < slice.len() {
        clauses.push(ByteRange {
            start: start + leading,
            end: end - trailing,
        });
    }
}

/// English markers are matched case-insensitively on word boundaries;
/// Chinese markers are matched as substrings.
const PROHIBITION_MARKERS: &[&str] = &[
    "must not",
    "mustn't",
    "do not",
    "don't",
    "does not",
    "never",
    "shall not",
    "should not",
    "shouldn't",
    "cannot",
    "can't",
    "may not",
    "not allowed",
    "forbidden",
    "forbid",
    "prohibited",
    "prohibit",
    "without",
    "不得",
    "不要",
    "禁止",
    "不能",
    "不可",
    "不许",
    "严禁",
    "请勿",
    "不允许",
    "不准",
];

/// Markers whose presence makes a clause binding.
const BINDING_MARKERS: &[&str] = &[
    "must",
    "shall",
    "required",
    "require",
    "need to",
    "needs to",
    "have to",
    "has to",
    "only",
    "exactly",
    "at most",
    "at least",
    "必须",
    "务必",
    "需要",
    "一定",
    "只能",
    "至少",
    "最多",
    "不超过",
];

/// Whether `text` contains a binding marker (used to detect weakened force).
#[must_use]
pub fn has_binding_marker(text: &str) -> bool {
    !markers(text, BINDING_MARKERS).is_empty() || !markers(text, PROHIBITION_MARKERS).is_empty()
}

fn markers(text: &str, table: &[&str]) -> Vec<ByteRange> {
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths for some scripts; fall back to
    // exact matching when it does, so offsets stay valid.
    let haystack = if lower.len() == text.len() {
        lower.as_str()
    } else {
        text
    };
    let mut found = Vec::new();
    for marker in table {
        let ascii = marker.is_ascii();
        let mut from = 0;
        while let Some(position) = haystack[from..].find(marker) {
            let start = from + position;
            let end = start + marker.len();
            let bounded = !ascii
                || (is_word_boundary(haystack, start, true)
                    && is_word_boundary(haystack, end, false));
            if bounded {
                found.push(ByteRange { start, end });
            }
            from = start + marker.len().max(1);
        }
    }
    found.sort();
    found.dedup();
    found
}

fn is_word_boundary(text: &str, index: usize, before: bool) -> bool {
    let neighbor = if before {
        text[..index].chars().next_back()
    } else {
        text[index..].chars().next()
    };
    neighbor.is_none_or(|character| {
        !(character.is_alphanumeric() || character == '_' || character == '\'')
    })
}

/// Regions quoted with backticks, straight or curly double quotes, or
/// corner brackets. Unterminated quotes produce no region.
fn literal_regions(text: &str) -> Vec<ByteRange> {
    const PAIRS: &[(char, char)] = &[
        ('`', '`'),
        ('"', '"'),
        ('“', '”'),
        ('「', '」'),
        ('『', '』'),
    ];
    let mut regions = Vec::new();
    let mut position = 0;
    while let Some(character) = text[position..].chars().next() {
        let width = character.len_utf8();
        if let Some(&(_, close)) = PAIRS.iter().find(|(open, _)| *open == character) {
            let content_start = position + width;
            if let Some(offset) = text[content_start..].find(close) {
                let end = content_start + offset + close.len_utf8();
                regions.push(ByteRange {
                    start: position,
                    end,
                });
                position = end;
                continue;
            }
        }
        position += width;
    }
    regions
}

/// Literal tokens inside `range`: the contents of quoted regions, plus
/// unquoted path-like tokens (containing `/` or `\`, or a file extension
/// such as `x.mjs`), plus `@`-mentions and leading-slash command tokens.
#[must_use]
pub fn literal_tokens(text: &str, range: ByteRange) -> Vec<String> {
    let Some(slice) = text.get(range.start..range.end) else {
        return Vec::new();
    };
    let mut tokens = Vec::new();
    let regions = literal_regions(text);
    for region in &regions {
        if range.contains(*region) {
            let inner = &text[region.start..region.end];
            let mut characters = inner.chars();
            let open = characters.next().map_or(0, char::len_utf8);
            let close = characters.next_back().map_or(0, char::len_utf8);
            let content = &inner[open..inner.len() - close];
            if !content.is_empty() {
                tokens.push(content.to_string());
            }
        }
    }
    let mut offset = range.start;
    for word in slice.split_whitespace() {
        let position = text[offset..range.end]
            .find(word)
            .map_or(offset, |found| offset + found);
        offset = position + word.len();
        let word_range = ByteRange {
            start: position,
            end: position + word.len(),
        };
        if regions.iter().any(|region| region.overlaps(word_range)) {
            continue;
        }
        let trimmed =
            word.trim_end_matches([',', ';', ':', '.', ')', '，', '。', '；', '：', '）']);
        let trimmed = trimmed.trim_start_matches(['(', '（']);
        if is_path_like(trimmed) || trimmed.starts_with('@') && trimmed.len() > 1 {
            tokens.push(trimmed.to_string());
        }
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

fn is_path_like(token: &str) -> bool {
    if token.len() < 3 || !token.is_ascii() {
        return false;
    }
    let has_separator = token.contains('/') || token.contains('\\');
    let has_extension = token.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && (1..=5).contains(&extension.len())
            && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
            && extension.bytes().any(|byte| byte.is_ascii_alphabetic())
    });
    (has_separator || has_extension) && !token.starts_with("http")
}

/// Decimal number tokens, with full-width digits folded to ASCII.
#[must_use]
pub fn numeric_tokens(text: &str) -> Vec<String> {
    let folded: String =
        text.chars()
            .map(|character| match character {
                '０'..='９' => char::from_u32(u32::from(character) - 0xFF10 + u32::from('0'))
                    .unwrap_or(character),
                other => other,
            })
            .collect();
    let mut tokens = Vec::new();
    let mut current = String::new();
    let characters: Vec<char> = folded.chars().collect();
    for (index, character) in characters.iter().enumerate() {
        let decimal_point = *character == '.'
            && !current.is_empty()
            && !current.contains('.')
            && characters.get(index + 1).is_some_and(char::is_ascii_digit);
        if character.is_ascii_digit() || decimal_point {
            current.push(*character);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

/// Whether the text contains CJK ideographs, kana, or hangul.
#[must_use]
pub fn contains_cjk(text: &str) -> bool {
    text.chars().any(|character| {
        matches!(u32::from(character),
            0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(text: &str) -> SourceRequest {
        SourceRequest::new(text.as_bytes().to_vec()).expect("valid request")
    }

    fn texts<'a>(text: &'a str, ranges: &[ByteRange]) -> Vec<&'a str> {
        ranges
            .iter()
            .map(|range| &text[range.start..range.end])
            .collect()
    }

    #[test]
    fn requests_must_be_bounded_utf8() {
        assert_eq!(
            SourceRequest::new(vec![0xff]),
            Err(SourceError::InvalidText)
        );
        assert_eq!(
            SourceRequest::new(b"   ".to_vec()),
            Err(SourceError::InvalidText)
        );
        assert_eq!(
            SourceRequest::new(vec![b'a'; MAX_SOURCE_REQUEST_BYTES + 1]),
            Err(SourceError::TooLarge)
        );
    }

    #[test]
    fn clauses_split_on_sentence_ends_but_not_inside_literals_or_file_names() {
        let source = request(
            "Build it. Keep `a. b` and src/store.mjs intact; then stop.\n任务不能为空。最多120个字",
        );
        let clauses = source.clauses();
        assert_eq!(
            texts(source.text(), &clauses),
            vec![
                "Build it.",
                "Keep `a. b` and src/store.mjs intact;",
                "then stop.",
                "任务不能为空。",
                "最多120个字",
            ]
        );
    }

    #[test]
    fn spans_resolve_only_on_exact_text_and_char_boundaries() {
        let source = request("任务 must not be blank");
        let good = SourceSpan {
            start: 0,
            end: 6,
            quoted: "任务".into(),
        };
        assert_eq!(source.resolve(&good), Some(ByteRange { start: 0, end: 6 }));
        let split_char = SourceSpan {
            start: 0,
            end: 4,
            quoted: "任".into(),
        };
        assert_eq!(source.resolve(&split_char), None);
        let mismatch = SourceSpan {
            start: 7,
            end: 11,
            quoted: "MUST".into(),
        };
        assert_eq!(source.resolve(&mismatch), None);
    }

    #[test]
    fn prohibitions_are_found_outside_literals_only() {
        let text =
            "Never edit tests. Show the text `do not` verbatim. 不得修改 package.json. Notable.";
        let source = request(text);
        let found = texts(text, &source.prohibition_markers());
        assert_eq!(found, vec!["Never", "不得"]);
    }

    #[test]
    fn numbers_fold_full_width_digits_and_keep_decimals() {
        assert_eq!(
            numeric_tokens("最多１２０个, 8 KiB, 1.5x, v2"),
            vec!["1.5", "120", "2", "8"]
        );
    }

    #[test]
    fn literal_tokens_cover_quotes_paths_and_mentions() {
        let text =
            "Use `src/store.mjs`, keep \"任务 ✓\" and edit public/app.mjs or @C:/secret.txt now.";
        let tokens = literal_tokens(
            text,
            ByteRange {
                start: 0,
                end: text.len(),
            },
        );
        assert_eq!(
            tokens,
            vec![
                "@C:/secret.txt",
                "public/app.mjs",
                "src/store.mjs",
                "任务 ✓"
            ]
        );
    }

    #[test]
    fn binding_markers_detect_obligations() {
        assert!(has_binding_marker("Titles must be short"));
        assert!(has_binding_marker("标题最多120个字符"));
        assert!(!has_binding_marker("A nice board for tasks"));
        assert!(!has_binding_marker("Mustard is yellow"));
    }

    #[test]
    fn cjk_detection_ignores_latin_and_symbols() {
        assert!(contains_cjk("Return 任务"));
        assert!(!contains_cjk("Return émoji ✓ 😀"));
    }
}
