//! Paging of captured transcript records for the Control output operation
//! (ADR 029 §7).
//!
//! A page carries fragments of byte-exact vendor records. A record larger
//! than a page is split on UTF-8 boundaries and reassembled by the client
//! from `offset` and `total_bytes`. The builder tracks the JSON-encoded size
//! of every fragment exactly, so a page never exceeds the encoded budget the
//! caller passes, whatever the vendor printed; every page still makes
//! progress. Fragments hold raw vendor content, so their `Debug` output is
//! redacted.

use std::fmt;

use serde::{Deserialize, Serialize};
use vibemux_harness::dispatch::{DispatchError, ObservationKind, ObservedRecord};

/// Raw record bytes per page.
pub const MAX_OUTPUT_PAGE_RAW_BYTES: usize = 24 * 1024;
/// Fragments per page.
pub const MAX_OUTPUT_PAGE_FRAGMENTS: usize = 256;

/// Where a read resumes.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputCursor {
    /// Every record up to this sequence has been delivered in full.
    pub after_sequence: u64,
    /// Bytes already delivered of the first record after `after_sequence`.
    pub cursor_offset: u64,
}

/// One slice of a captured record.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchOutputFragment {
    pub sequence: u64,
    pub kind: ObservationKind,
    /// Byte length of the whole record.
    pub total_bytes: u64,
    /// Byte offset of `fragment` in the record.
    pub offset: u64,
    /// Raw record bytes, split only on UTF-8 boundaries.
    pub fragment: String,
}

impl fmt::Debug for DispatchOutputFragment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DispatchOutputFragment")
            .field("sequence", &self.sequence)
            .field("kind", &self.kind)
            .field("total_bytes", &self.total_bytes)
            .field("offset", &self.offset)
            .field("fragment_bytes", &self.fragment.len())
            .finish()
    }
}

/// One read of a transcript.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchOutputPage {
    pub fragments: Vec<DispatchOutputFragment>,
    /// Cursor for the next read.
    pub next: OutputCursor,
    /// The attempt finished capturing and every record was delivered.
    pub complete: bool,
}

/// Size limits of one page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputPageLimits {
    pub raw_bytes: usize,
    pub fragments: usize,
    /// Upper bound on the page's JSON encoding.
    pub encoded_bytes: usize,
}

impl OutputPageLimits {
    /// The default raw and fragment limits within `encoded_bytes`.
    #[must_use]
    pub const fn within(encoded_bytes: usize) -> Self {
        Self {
            raw_bytes: MAX_OUTPUT_PAGE_RAW_BYTES,
            fragments: MAX_OUTPUT_PAGE_FRAGMENTS,
            encoded_bytes,
        }
    }
}

/// Builds the page that starts at `cursor`. `records` are one transcript in
/// capture order; `finished` says no record will be added. A cursor offset
/// that is not inside the next record on a UTF-8 boundary is
/// `harness_dispatch_invalid_request`.
pub fn build_output_page(
    records: &[ObservedRecord],
    finished: bool,
    cursor: OutputCursor,
    limits: OutputPageLimits,
) -> Result<DispatchOutputPage, DispatchError> {
    let start = records.partition_point(|record| record.sequence() <= cursor.after_sequence);
    let mut offset =
        usize::try_from(cursor.cursor_offset).map_err(|_| DispatchError::InvalidRequest)?;
    if offset > 0 {
        let first = records.get(start).ok_or(DispatchError::InvalidRequest)?;
        if offset >= first.byte_count() || !first.raw_json().is_char_boundary(offset) {
            return Err(DispatchError::InvalidRequest);
        }
    }
    let mut next = OutputCursor {
        after_sequence: cursor.after_sequence,
        cursor_offset: 0,
    };
    let mut fragments = Vec::new();
    let mut raw_bytes = 0;
    let mut encoded_bytes = empty_page_bytes()?;
    for record in &records[start..] {
        if fragments.len() == limits.fragments {
            break;
        }
        let separator = usize::from(!fragments.is_empty());
        let overhead = fragment_overhead(record, offset)? + separator;
        let text = &record.raw_json()[offset..];
        let (length, escaped) = fitting_prefix(
            text,
            limits.raw_bytes.saturating_sub(raw_bytes),
            limits
                .encoded_bytes
                .saturating_sub(encoded_bytes.saturating_add(overhead)),
        );
        if length == 0 {
            break;
        }
        fragments.push(DispatchOutputFragment {
            sequence: record.sequence(),
            kind: record.kind(),
            total_bytes: record.byte_count() as u64,
            offset: offset as u64,
            fragment: text[..length].to_string(),
        });
        raw_bytes += length;
        encoded_bytes += overhead + escaped;
        if length < text.len() {
            next.cursor_offset = (offset + length) as u64;
            break;
        }
        next.after_sequence = record.sequence();
        offset = 0;
    }
    if fragments.is_empty() && start < records.len() {
        // The limits cannot hold even one character of the next record.
        return Err(DispatchError::Internal);
    }
    let delivered = next.cursor_offset == 0
        && records
            .last()
            .is_none_or(|last| last.sequence() <= next.after_sequence);
    Ok(DispatchOutputPage {
        fragments,
        next,
        complete: finished && delivered,
    })
}

/// A complete record rebuilt from fragments.
#[derive(Clone, Eq, PartialEq)]
pub struct ReassembledRecord {
    pub sequence: u64,
    pub kind: ObservationKind,
    pub raw_json: String,
}

impl fmt::Debug for ReassembledRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReassembledRecord")
            .field("sequence", &self.sequence)
            .field("kind", &self.kind)
            .field("raw_bytes", &self.raw_json.len())
            .finish()
    }
}

/// A page that does not continue the stream the reassembler has seen.
#[derive(Clone, Copy, Debug, Eq, thiserror::Error, PartialEq)]
#[error("harness dispatch output page is inconsistent")]
pub struct OutputReassemblyError;

/// Client-side reassembly of pages into whole records. It checks that every
/// fragment continues the stream and that each page's cursor matches what
/// it delivered.
#[derive(Debug)]
pub struct OutputReassembler {
    after_sequence: u64,
    partial: Option<ReassembledRecord>,
    partial_total: usize,
}

impl OutputReassembler {
    /// Starts after `after_sequence`, at the beginning of the next record.
    #[must_use]
    pub const fn starting_after(after_sequence: u64) -> Self {
        Self {
            after_sequence,
            partial: None,
            partial_total: 0,
        }
    }

    /// Cursor for the next read.
    #[must_use]
    pub fn cursor(&self) -> OutputCursor {
        OutputCursor {
            after_sequence: self.after_sequence,
            cursor_offset: self
                .partial
                .as_ref()
                .map_or(0, |partial| partial.raw_json.len() as u64),
        }
    }

    /// Consumes one page and returns the records it completed.
    pub fn accept(
        &mut self,
        page: DispatchOutputPage,
    ) -> Result<Vec<ReassembledRecord>, OutputReassemblyError> {
        let mut completed = Vec::new();
        for fragment in page.fragments {
            let total = usize::try_from(fragment.total_bytes).map_err(|_| OutputReassemblyError)?;
            let offset = usize::try_from(fragment.offset).map_err(|_| OutputReassemblyError)?;
            let continues = match &self.partial {
                Some(partial) => {
                    partial.sequence == fragment.sequence
                        && partial.kind == fragment.kind
                        && self.partial_total == total
                        && partial.raw_json.len() == offset
                }
                None => fragment.sequence > self.after_sequence && offset == 0,
            };
            if !continues || fragment.fragment.is_empty() {
                return Err(OutputReassemblyError);
            }
            let partial = self.partial.get_or_insert_with(|| ReassembledRecord {
                sequence: fragment.sequence,
                kind: fragment.kind,
                raw_json: String::new(),
            });
            partial.raw_json.push_str(&fragment.fragment);
            self.partial_total = total;
            if partial.raw_json.len() > total {
                return Err(OutputReassemblyError);
            }
            if partial.raw_json.len() == total {
                if let Some(record) = self.partial.take() {
                    self.after_sequence = record.sequence;
                    completed.push(record);
                }
            }
        }
        if page.next != self.cursor() {
            return Err(OutputReassemblyError);
        }
        Ok(completed)
    }
}

/// Length of a JSON string's content after serde_json escaping: two bytes
/// for `"`, `\`, and the short control escapes, six for other control
/// characters, and the UTF-8 length otherwise.
const fn escaped_length(character: char) -> usize {
    match character {
        '"' | '\\' | '\u{08}' | '\t' | '\n' | '\u{0C}' | '\r' => 2,
        '\u{00}'..='\u{1F}' => 6,
        _ => character.len_utf8(),
    }
}

/// The longest prefix of `text` within both budgets, as its byte length and
/// its escaped length.
fn fitting_prefix(text: &str, raw_budget: usize, encoded_budget: usize) -> (usize, usize) {
    let mut length = 0;
    let mut escaped = 0;
    for character in text.chars() {
        let width = character.len_utf8();
        let cost = escaped_length(character);
        if length + width > raw_budget || escaped + cost > encoded_budget {
            break;
        }
        length += width;
        escaped += cost;
    }
    (length, escaped)
}

/// Encoded size of a fragment of `record` at `offset` with empty content.
fn fragment_overhead(record: &ObservedRecord, offset: usize) -> Result<usize, DispatchError> {
    encoded_length(&DispatchOutputFragment {
        sequence: record.sequence(),
        kind: record.kind(),
        total_bytes: record.byte_count() as u64,
        offset: offset as u64,
        fragment: String::new(),
    })
}

/// Encoded size of a page without fragments, with the widest cursor and
/// flag, so it bounds every real page.
fn empty_page_bytes() -> Result<usize, DispatchError> {
    encoded_length(&DispatchOutputPage {
        fragments: Vec::new(),
        next: OutputCursor {
            after_sequence: u64::MAX,
            cursor_offset: u64::MAX,
        },
        complete: false,
    })
}

fn encoded_length(value: &impl Serialize) -> Result<usize, DispatchError> {
    serde_json::to_vec(value)
        .map(|encoded| encoded.len())
        .map_err(|_| DispatchError::Internal)
}

#[cfg(test)]
mod tests {
    use vibemux_harness::dispatch::NativeProtocol;

    use super::*;

    const ENCODED_BUDGET: usize = 62 * 1024;

    fn record(sequence: u64, raw: &str) -> ObservedRecord {
        ObservedRecord::parse(NativeProtocol::CodexExec, sequence, raw.to_string())
            .expect("record")
            .0
    }

    fn text_record(sequence: u64, text: &str) -> ObservedRecord {
        let raw = serde_json::json!({"type": "item.updated", "text": text}).to_string();
        record(sequence, &raw)
    }

    /// Reads the whole transcript page by page, checking each page's
    /// encoded size, and returns the records and the page count.
    fn read_all(
        records: &[ObservedRecord],
        limits: OutputPageLimits,
    ) -> (Vec<ReassembledRecord>, usize) {
        let mut reassembler = OutputReassembler::starting_after(0);
        let mut output = Vec::new();
        let mut pages = 0;
        loop {
            let page =
                build_output_page(records, true, reassembler.cursor(), limits).expect("page");
            let encoded = serde_json::to_vec(&page).expect("encode page").len();
            assert!(encoded <= limits.encoded_bytes, "page of {encoded} bytes");
            assert!(page.fragments.len() <= limits.fragments);
            let raw: usize = page
                .fragments
                .iter()
                .map(|fragment| fragment.fragment.len())
                .sum();
            assert!(raw <= limits.raw_bytes);
            let complete = page.complete;
            pages += 1;
            output.extend(reassembler.accept(page).expect("consistent page"));
            if complete {
                return (output, pages);
            }
            assert!(pages < 10_000, "the reader made no progress");
        }
    }

    fn assert_round_trip(records: &[ObservedRecord], limits: OutputPageLimits) -> usize {
        let (output, pages) = read_all(records, limits);
        assert_eq!(output.len(), records.len());
        for (rebuilt, original) in output.iter().zip(records) {
            assert_eq!(rebuilt.sequence, original.sequence());
            assert_eq!(rebuilt.kind, original.kind());
            assert_eq!(rebuilt.raw_json, original.raw_json());
        }
        pages
    }

    #[test]
    fn escaped_length_matches_serde_json_for_every_ascii_character_and_beyond() {
        let samples = (0_u8..=0x7F)
            .map(char::from)
            .chain(['\u{80}', 'é', '\u{2028}', '😀', '\u{FFFD}']);
        for character in samples {
            let encoded = serde_json::to_string(&character.to_string()).expect("encode");
            assert_eq!(
                escaped_length(character),
                encoded.len() - 2,
                "{:?}",
                character
            );
        }
    }

    #[test]
    fn small_records_arrive_whole_on_one_page() {
        let records: Vec<ObservedRecord> = (1..=3)
            .map(|sequence| text_record(sequence, "chunk"))
            .collect();
        let limits = OutputPageLimits::within(ENCODED_BUDGET);
        let page =
            build_output_page(&records, false, OutputCursor::default(), limits).expect("page");
        assert_eq!(page.fragments.len(), 3);
        assert!(page.fragments.iter().all(|fragment| fragment.offset == 0));
        assert_eq!(
            page.next,
            OutputCursor {
                after_sequence: 3,
                cursor_offset: 0
            }
        );
        assert!(!page.complete, "a live transcript is never complete");
        let caught_up = build_output_page(&records, false, page.next, limits).expect("tail");
        assert!(caught_up.fragments.is_empty() && !caught_up.complete);
        let finished = build_output_page(&records, true, page.next, limits).expect("finished");
        assert!(finished.fragments.is_empty() && finished.complete);
        assert!(
            build_output_page(&[], true, OutputCursor::default(), limits)
                .expect("empty")
                .complete
        );
    }

    #[test]
    fn a_large_record_spans_pages_and_reassembles_byte_exact() {
        let text = "é😀 plain ".repeat(12 * 1024);
        let records = [
            text_record(1, "before"),
            text_record(2, &text),
            text_record(3, "after"),
        ];
        let pages = assert_round_trip(&records, OutputPageLimits::within(ENCODED_BUDGET));
        assert!(pages >= 5, "{pages} pages");
    }

    #[test]
    fn maximal_escaping_stays_within_the_encoded_budget() {
        // Quotes and backslashes double in the record and again in the page;
        // tab and CR are raw whitespace between tokens.
        let text = "\"\\".repeat(20 * 1024);
        let spaced = format!("{{\t\"text\":\r{}}}", serde_json::to_string(&text).unwrap());
        let records = [record(1, &spaced), text_record(2, &"\\".repeat(30 * 1024))];
        let limits = OutputPageLimits::within(ENCODED_BUDGET);
        assert_round_trip(&records, limits);
        let tight = OutputPageLimits {
            raw_bytes: MAX_OUTPUT_PAGE_RAW_BYTES,
            fragments: MAX_OUTPUT_PAGE_FRAGMENTS,
            encoded_bytes: 4 * 1024,
        };
        assert_round_trip(&records, tight);
    }

    #[test]
    fn many_small_records_respect_the_fragment_limit() {
        let records: Vec<ObservedRecord> = (1..=700)
            .map(|sequence| text_record(sequence, "c"))
            .collect();
        let pages = assert_round_trip(&records, OutputPageLimits::within(ENCODED_BUDGET));
        assert_eq!(pages, 3);
    }

    #[test]
    fn multi_byte_characters_are_never_split() {
        let records = [text_record(1, &"😀".repeat(64))];
        for raw_bytes in [1, 3, 5, 7, 13] {
            let limits = OutputPageLimits {
                raw_bytes: raw_bytes.max(4),
                fragments: MAX_OUTPUT_PAGE_FRAGMENTS,
                encoded_bytes: ENCODED_BUDGET,
            };
            assert_round_trip(&records, limits);
        }
    }

    #[test]
    fn a_cursor_outside_the_next_record_is_invalid() {
        let records = [text_record(1, "é"), text_record(2, "second")];
        let limits = OutputPageLimits::within(ENCODED_BUDGET);
        let raw = records[0].raw_json();
        let inside_character = raw.find('é').expect("é") + 1;
        for cursor_offset in [inside_character, raw.len(), raw.len() + 5] {
            let cursor = OutputCursor {
                after_sequence: 0,
                cursor_offset: cursor_offset as u64,
            };
            assert_eq!(
                build_output_page(&records, true, cursor, limits),
                Err(DispatchError::InvalidRequest)
            );
        }
        let past_the_end = OutputCursor {
            after_sequence: 2,
            cursor_offset: 1,
        };
        assert_eq!(
            build_output_page(&records, true, past_the_end, limits),
            Err(DispatchError::InvalidRequest)
        );
        let resumed = OutputCursor {
            after_sequence: 0,
            cursor_offset: 3,
        };
        let page = build_output_page(&records, true, resumed, limits).expect("resumed page");
        assert_eq!(page.fragments[0].offset, 3);
        assert_eq!(page.fragments[0].fragment, raw[3..]);
    }

    #[test]
    fn limits_that_hold_no_character_are_an_internal_error() {
        let records = [text_record(1, "chunk")];
        let limits = OutputPageLimits {
            raw_bytes: MAX_OUTPUT_PAGE_RAW_BYTES,
            fragments: MAX_OUTPUT_PAGE_FRAGMENTS,
            encoded_bytes: 64,
        };
        assert_eq!(
            build_output_page(&records, true, OutputCursor::default(), limits),
            Err(DispatchError::Internal)
        );
    }

    #[test]
    fn the_reassembler_refuses_a_page_that_skips_or_repeats_bytes() {
        let records = [text_record(1, &"x".repeat(100))];
        let limits = OutputPageLimits {
            raw_bytes: 40,
            fragments: MAX_OUTPUT_PAGE_FRAGMENTS,
            encoded_bytes: ENCODED_BUDGET,
        };
        let first =
            build_output_page(&records, true, OutputCursor::default(), limits).expect("first page");
        let mut reassembler = OutputReassembler::starting_after(0);
        assert!(reassembler.accept(first.clone()).expect("first").is_empty());
        assert_eq!(reassembler.accept(first), Err(OutputReassemblyError));

        let mut skipping = OutputReassembler::starting_after(0);
        let later = OutputCursor {
            after_sequence: 0,
            cursor_offset: 40,
        };
        let second = build_output_page(&records, true, later, limits).expect("second page");
        assert_eq!(skipping.accept(second), Err(OutputReassemblyError));
    }

    #[test]
    fn fragment_debug_output_hides_the_content() {
        let records = [text_record(1, "SYNTHETIC_SECRET_TOKEN")];
        let page = build_output_page(
            &records,
            true,
            OutputCursor::default(),
            OutputPageLimits::within(ENCODED_BUDGET),
        )
        .expect("page");
        let rendered = format!("{page:?}");
        assert!(!rendered.contains("SYNTHETIC_SECRET_TOKEN"));
        let mut reassembler = OutputReassembler::starting_after(0);
        let rebuilt = reassembler.accept(page).expect("records");
        assert!(!format!("{rebuilt:?}").contains("SYNTHETIC_SECRET_TOKEN"));
    }
}
