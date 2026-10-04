//! Bounded, sequenced capture of byte-exact runtime records.
//!
//! A record is one decoded line from the runtime, kept verbatim so the
//! terminal UI can show what the runtime actually said. Classification reads
//! only verified discriminants and never rewrites the bytes; an unknown frame
//! shape stays [`DshRecordKind::Other`] rather than being dropped, because a
//! merge-extensible protocol must not silently lose traffic.
//!
//! Because a DSH session is long-lived, the buffer is a ring rather than a
//! one-shot capture: reaching a ceiling evicts the oldest records and counts
//! the evictions, so a poll page can report the gap honestly. A single record
//! larger than the byte ceiling is refused outright — a record is never
//! truncated in place (ADR 030 Decision 8).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    DshError, DshSessionLimits, MAX_POLL_BYTES, MAX_POLL_RECORDS,
    wire::{
        EVENT_TURN_END, EVENT_TURN_START, NOTIFICATION_SESSION_EVENT, NOTIFICATION_SESSION_STATUS,
        NOTIFICATION_SUBAGENT_FINISHED, NOTIFICATION_SUBAGENT_STARTED, session_event_type,
    },
};

/// Coarse classification of one runtime record, for status views and
/// dashboards. Never a completion signal by itself.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DshRecordKind {
    /// A response carrying `result.serverInfo`.
    Handshake,
    TurnStart,
    TurnEnd,
    /// Assistant text or raw stream chunks.
    Text,
    Tool,
    Approval,
    /// A `session.status` transition. Never completion authority.
    Status,
    /// A subagent lifecycle notification.
    Subagent,
    /// Any other correlated response.
    Response,
    /// A JSON-RPC error frame.
    Error,
    /// A tolerated frame this adapter does not interpret.
    Other,
}

impl DshRecordKind {
    pub const ALL: [Self; 11] = [
        Self::Handshake,
        Self::TurnStart,
        Self::TurnEnd,
        Self::Text,
        Self::Tool,
        Self::Approval,
        Self::Status,
        Self::Subagent,
        Self::Response,
        Self::Error,
        Self::Other,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Handshake => "handshake",
            Self::TurnStart => "turn_start",
            Self::TurnEnd => "turn_end",
            Self::Text => "text",
            Self::Tool => "tool",
            Self::Approval => "approval",
            Self::Status => "status",
            Self::Subagent => "subagent",
            Self::Response => "response",
            Self::Error => "error",
            Self::Other => "other",
        }
    }

    /// Classifies one decoded frame. Pure and total: every frame gets a kind.
    #[must_use]
    pub fn classify(value: &Value) -> Self {
        if value.get("error").is_some() {
            return Self::Error;
        }
        match value.get("method").and_then(Value::as_str) {
            Some(NOTIFICATION_SESSION_EVENT) => match session_event_type(value) {
                Some(EVENT_TURN_START) => Self::TurnStart,
                Some(EVENT_TURN_END) => Self::TurnEnd,
                Some("assistant/message" | "assistant/chunk") => Self::Text,
                Some("tool/call" | "tool/result") => Self::Tool,
                Some("approval/asked") => Self::Approval,
                _ => Self::Other,
            },
            Some(NOTIFICATION_SESSION_STATUS) => Self::Status,
            Some(NOTIFICATION_SUBAGENT_STARTED | NOTIFICATION_SUBAGENT_FINISHED) => Self::Subagent,
            // Server-to-client requests and unknown notifications.
            Some(_) => Self::Other,
            None => {
                if value.pointer("/result/serverInfo").is_some() {
                    Self::Handshake
                } else {
                    Self::Response
                }
            }
        }
    }
}

/// One decoded runtime record, byte-exact including unknown fields.
///
/// Deliberately not `Serialize`: raw runtime content leaves the daemon only
/// through the explicit session-poll operation, never through events, logs,
/// status views, or `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct DshRecord {
    sequence: u64,
    kind: DshRecordKind,
    raw_json: String,
}

impl DshRecord {
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub const fn kind(&self) -> DshRecordKind {
        self.kind
    }

    #[must_use]
    pub fn raw_json(&self) -> &str {
        &self.raw_json
    }

    #[must_use]
    pub fn byte_count(&self) -> usize {
        self.raw_json.len()
    }
}

impl std::fmt::Debug for DshRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DshRecord")
            .field("sequence", &self.sequence)
            .field("kind", &self.kind)
            .field("bytes", &self.raw_json.len())
            .finish()
    }
}

/// One page of records, in ascending sequence order.
#[derive(Debug)]
pub struct DshRecordBatch<'a> {
    pub records: Vec<&'a DshRecord>,
    /// Cursor to pass to the next poll.
    pub next: u64,
    /// More records are already available.
    pub more: bool,
    /// The requested cursor predated the retained window, so records between
    /// the cursor and [`DshRecordBatch::oldest_retained`] were evicted.
    pub truncated: bool,
    /// Oldest sequence still retained; zero when the buffer is empty.
    pub oldest_retained: u64,
}

/// Monotonically sequenced ring of runtime records.
#[derive(Debug)]
pub struct DshRecordBuffer {
    records: VecDeque<DshRecord>,
    bytes: usize,
    bytes_limit: usize,
    count_limit: usize,
    head: u64,
    dropped: u64,
}

impl DshRecordBuffer {
    #[must_use]
    pub fn new(limits: &DshSessionLimits) -> Self {
        Self {
            records: VecDeque::new(),
            bytes: 0,
            bytes_limit: limits.record_bytes,
            count_limit: limits.record_count,
            head: 0,
            dropped: 0,
        }
    }

    /// Appends one decoded frame and returns its sequence. The frame is
    /// classified but never rewritten. A frame larger than the byte ceiling is
    /// refused; otherwise the oldest records are evicted to make room.
    pub fn push(&mut self, raw_json: String, value: &Value) -> Result<u64, DshError> {
        if raw_json.len() > self.bytes_limit {
            return Err(DshError::RecordBufferFull);
        }
        // One slot is reserved for the record being admitted, so the count
        // ceiling bounds the buffer before the push rather than after it.
        while self.records.len() >= self.count_limit
            || self.bytes.saturating_add(raw_json.len()) > self.bytes_limit
        {
            let Some(evicted) = self.records.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(evicted.raw_json.len());
            self.dropped = self.dropped.saturating_add(1);
        }
        self.head = self.head.saturating_add(1);
        self.bytes = self.bytes.saturating_add(raw_json.len());
        self.records.push_back(DshRecord {
            sequence: self.head,
            kind: DshRecordKind::classify(value),
            raw_json,
        });
        Ok(self.head)
    }

    /// Sequence of the most recent record, or zero before the first push.
    #[must_use]
    pub const fn head(&self) -> u64 {
        self.head
    }

    /// Sequence of the oldest retained record, or zero when empty.
    #[must_use]
    pub fn oldest_retained(&self) -> u64 {
        self.records.front().map_or(0, DshRecord::sequence)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }

    /// Records evicted by the bounds since the session started.
    #[must_use]
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Records strictly after `cursor`, capped by the poll bounds. A cursor
    /// ahead of the head is refused; a cursor older than the retained window
    /// yields the retained records with `truncated` set.
    pub fn after(&self, cursor: u64) -> Result<DshRecordBatch<'_>, DshError> {
        if cursor > self.head {
            return Err(DshError::InvalidCursor);
        }
        let oldest = self.oldest_retained();
        // A cursor of zero means "from the beginning", so a gap is reported
        // whenever the oldest retained record is not the very first one.
        let truncated = oldest != 0 && cursor + 1 < oldest;
        let mut records = Vec::new();
        let mut bytes = 0_usize;
        for record in self.records.iter().filter(|r| r.sequence > cursor) {
            if records.len() >= MAX_POLL_RECORDS {
                break;
            }
            if bytes + record.raw_json.len() > MAX_POLL_BYTES && !records.is_empty() {
                break;
            }
            bytes += record.raw_json.len();
            records.push(record);
        }
        let more = records
            .last()
            .is_some_and(|record| record.sequence < self.head);
        let next = records.last().map_or(cursor, |record| record.sequence());
        Ok(DshRecordBatch {
            records,
            next,
            more,
            truncated,
            oldest_retained: oldest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsh::{DEFAULT_RECORD_BYTES, DEFAULT_RECORD_COUNT};
    use serde_json::json;

    fn limits(record_bytes: usize, record_count: usize) -> DshSessionLimits {
        DshSessionLimits {
            record_bytes,
            record_count,
            ..DshSessionLimits::default()
        }
    }

    fn frame(index: u64) -> (String, Value) {
        let text = json!({"jsonrpc": "2.0", "method": "session.event",
            "params": {"sessionId": "s", "event": {"type": "assistant/chunk",
                "seq": index, "time": 1, "data": {"text": "x"}}}})
        .to_string();
        let value: Value = serde_json::from_str(&text).expect("frame");
        (text, value)
    }

    #[test]
    fn classification_reads_only_verified_discriminants() {
        let cases: [(Value, DshRecordKind); 11] = [
            (
                json!({"result": {"serverInfo": {"name": "n", "version": "1"}}}),
                DshRecordKind::Handshake,
            ),
            (
                json!({"result": {"messageId": "m"}}),
                DshRecordKind::Response,
            ),
            (
                json!({"error": {"code": -32601, "message": "no"}}),
                DshRecordKind::Error,
            ),
            (
                json!({"method": "session.event",
                    "params": {"event": {"type": "turn/start", "data": {"turn": 1}}}}),
                DshRecordKind::TurnStart,
            ),
            (
                json!({"method": "session.event",
                    "params": {"event": {"type": "turn/end", "data": {"turn": 1,
                        "reason": {"kind": "completed"}}}}}),
                DshRecordKind::TurnEnd,
            ),
            (
                json!({"method": "session.event",
                    "params": {"event": {"type": "assistant/message", "data": {}}}}),
                DshRecordKind::Text,
            ),
            (
                json!({"method": "session.event",
                    "params": {"event": {"type": "tool/result", "data": {}}}}),
                DshRecordKind::Tool,
            ),
            (
                json!({"method": "session.event",
                    "params": {"event": {"type": "approval/asked", "data": {}}}}),
                DshRecordKind::Approval,
            ),
            (
                json!({"method": "session.status", "params": {"status": "idle"}}),
                DshRecordKind::Status,
            ),
            (
                json!({"method": "subagent.started", "params": {}}),
                DshRecordKind::Subagent,
            ),
            (
                json!({"method": "future/extension", "params": {}}),
                DshRecordKind::Other,
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(DshRecordKind::classify(&value), expected, "{value}");
        }
    }

    #[test]
    fn sequences_are_monotonic_and_pages_advance_the_cursor() {
        let mut buffer = DshRecordBuffer::new(&limits(DEFAULT_RECORD_BYTES, DEFAULT_RECORD_COUNT));
        for index in 1..=5 {
            let (text, value) = frame(index);
            assert_eq!(buffer.push(text, &value), Ok(index));
        }
        assert_eq!(buffer.head(), 5);
        assert_eq!(buffer.oldest_retained(), 1);
        let page = buffer.after(0).expect("first page");
        assert_eq!(page.records.len(), 5);
        assert_eq!(page.next, 5);
        assert!(!page.more);
        assert!(!page.truncated);
        let page = buffer.after(3).expect("resumed page");
        assert_eq!(page.records.len(), 2);
        assert_eq!(page.next, 5);
        assert_eq!(page.records[0].sequence(), 4);
        assert_eq!(buffer.after(5).expect("caught up").records.len(), 0);
        assert_eq!(buffer.after(6).err(), Some(DshError::InvalidCursor));
    }

    #[test]
    fn the_count_bound_evicts_the_oldest_and_reports_the_gap() {
        let mut buffer = DshRecordBuffer::new(&limits(DEFAULT_RECORD_BYTES, 4));
        for index in 1..=7 {
            let (text, value) = frame(index);
            buffer.push(text, &value).expect("push");
        }
        assert_eq!(buffer.len(), 4);
        assert_eq!(buffer.dropped(), 3);
        assert_eq!(buffer.oldest_retained(), 4);
        let page = buffer.after(0).expect("page");
        assert!(page.truncated, "a stale cursor must be reported as a gap");
        assert_eq!(page.oldest_retained, 4);
        assert_eq!(page.records.len(), 4);
    }

    #[test]
    fn a_single_oversized_record_is_refused_rather_than_truncated() {
        let mut buffer = DshRecordBuffer::new(&limits(1024, DEFAULT_RECORD_COUNT));
        let (text, value) = frame(1);
        assert_eq!(
            buffer.push("x".repeat(2048), &value),
            Err(DshError::RecordBufferFull)
        );
        // The refusal left the buffer untouched.
        assert!(buffer.is_empty());
        assert_eq!(buffer.head(), 0);
        buffer
            .push(text, &value)
            .expect("a bounded record still fits");
        assert_eq!(buffer.head(), 1);
    }

    #[test]
    fn the_byte_bound_evicts_instead_of_growing() {
        let (sample, _) = frame(1);
        let per_record = sample.len();
        let mut buffer = DshRecordBuffer::new(&limits(per_record * 3, DEFAULT_RECORD_COUNT));
        // Nine frames of identical encoded length, so the arithmetic below is
        // exact rather than approximate.
        for index in 1..=9 {
            let (text, value) = frame(index);
            assert_eq!(text.len(), per_record);
            buffer.push(text, &value).expect("push");
        }
        assert_eq!(buffer.bytes(), per_record * 3);
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.dropped(), 6);
        assert_eq!(buffer.oldest_retained(), 7);
    }

    #[test]
    fn a_page_never_exceeds_the_poll_bounds() {
        let mut buffer = DshRecordBuffer::new(&limits(DEFAULT_RECORD_BYTES, DEFAULT_RECORD_COUNT));
        for index in 1..=(MAX_POLL_RECORDS as u64 + 40) {
            let (text, value) = frame(index);
            buffer.push(text, &value).expect("push");
        }
        let page = buffer.after(0).expect("page");
        assert_eq!(page.records.len(), MAX_POLL_RECORDS);
        assert!(page.more);
        assert!(page.next < buffer.head());
        let rest = buffer.after(page.next).expect("second page");
        assert_eq!(rest.records.len(), 40);
        assert!(!rest.more);
    }
}
