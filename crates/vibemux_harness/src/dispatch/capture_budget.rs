//! Bounded capture accounting for one attempt.
//!
//! Every stdout frame passes through [`CaptureBudget::accept`], which enforces
//! the byte and record limits before parsing, assigns the record sequence,
//! classifies the record, and folds it into the per-kind counts and the
//! transcript digest. Limits fail closed: nothing is silently truncated.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    DispatchError, DispatchLimits, NativeProtocol, ObservationKind, ObservedRecord, Sha256Digest,
};

/// Record counts per observation kind.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KindCounts {
    pub started: u64,
    pub text: u64,
    pub tool: u64,
    pub approval: u64,
    pub usage: u64,
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub opaque: u64,
}

impl KindCounts {
    fn increment(&mut self, kind: ObservationKind) {
        let slot = match kind {
            ObservationKind::Started => &mut self.started,
            ObservationKind::Text => &mut self.text,
            ObservationKind::Tool => &mut self.tool,
            ObservationKind::Approval => &mut self.approval,
            ObservationKind::Usage => &mut self.usage,
            ObservationKind::Completed => &mut self.completed,
            ObservationKind::Failed => &mut self.failed,
            ObservationKind::Cancelled => &mut self.cancelled,
            ObservationKind::Opaque => &mut self.opaque,
        };
        *slot = slot.saturating_add(1);
    }

    #[must_use]
    pub const fn total(&self) -> u64 {
        self.started
            .saturating_add(self.text)
            .saturating_add(self.tool)
            .saturating_add(self.approval)
            .saturating_add(self.usage)
            .saturating_add(self.completed)
            .saturating_add(self.failed)
            .saturating_add(self.cancelled)
            .saturating_add(self.opaque)
    }
}

/// Content-free aggregate of a capture. `transcript_sha256` covers each
/// accepted record's bytes followed by LF, i.e. the canonical JSONL form.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSummary {
    pub record_count: u64,
    pub record_bytes: u64,
    pub kinds: KindCounts,
    pub transcript_sha256: Sha256Digest,
}

#[derive(Clone, Debug)]
pub struct CaptureBudget {
    protocol: NativeProtocol,
    max_bytes: u64,
    max_records: u64,
    record_bytes: u64,
    record_count: u64,
    kinds: KindCounts,
    transcript: Sha256,
}

impl CaptureBudget {
    pub fn new(protocol: NativeProtocol, limits: &DispatchLimits) -> Result<Self, DispatchError> {
        limits.validate()?;
        Ok(Self {
            protocol,
            max_bytes: limits.capture_bytes as u64,
            max_records: limits.record_count as u64,
            record_bytes: 0,
            record_count: 0,
            kinds: KindCounts::default(),
            transcript: Sha256::new(),
        })
    }

    /// Admits one frame: limit check, parse, classify, account.
    pub fn accept(&mut self, frame: String) -> Result<(ObservedRecord, Value), DispatchError> {
        let bytes = self
            .record_bytes
            .checked_add(frame.len() as u64)
            .ok_or(DispatchError::CaptureTooLarge)?;
        if bytes > self.max_bytes || self.record_count >= self.max_records {
            return Err(DispatchError::CaptureTooLarge);
        }
        let (record, value) = ObservedRecord::parse(self.protocol, self.record_count + 1, frame)?;
        self.record_bytes = bytes;
        self.record_count += 1;
        self.kinds.increment(record.kind());
        self.transcript.update(record.raw_json().as_bytes());
        self.transcript.update(b"\n");
        Ok((record, value))
    }

    #[must_use]
    pub fn summary(&self) -> CaptureSummary {
        CaptureSummary {
            record_count: self.record_count,
            record_bytes: self.record_bytes,
            kinds: self.kinds,
            transcript_sha256: Sha256Digest::from_hasher(self.transcript.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// One Codex app-server notification per observation kind.
    const ONE_OF_EACH_KIND: [(&str, ObservationKind); 9] = [
        (r#"{"method":"turn/started"}"#, ObservationKind::Started),
        (
            r#"{"method":"item/agentMessage/delta"}"#,
            ObservationKind::Text,
        ),
        (r#"{"method":"item/tool/call"}"#, ObservationKind::Tool),
        (
            r#"{"method":"item/commandExecution/requestApproval"}"#,
            ObservationKind::Approval,
        ),
        (
            r#"{"method":"thread/tokenUsage/updated"}"#,
            ObservationKind::Usage,
        ),
        (
            r#"{"method":"turn/completed","params":{"turn":{"status":"completed"}}}"#,
            ObservationKind::Completed,
        ),
        (
            r#"{"method":"turn/completed","params":{"turn":{"status":"failed"}}}"#,
            ObservationKind::Failed,
        ),
        (
            r#"{"method":"turn/completed","params":{"turn":{"status":"interrupted"}}}"#,
            ObservationKind::Cancelled,
        ),
        (r#"{"method":"error"}"#, ObservationKind::Opaque),
    ];

    fn budget(protocol: NativeProtocol, limits: DispatchLimits) -> CaptureBudget {
        CaptureBudget::new(protocol, &limits).expect("valid limits")
    }

    /// A JSON object frame of exactly `length` bytes (`{"p":""}` is 8).
    fn frame_of_length(length: usize) -> String {
        format!("{{\"p\":\"{}\"}}", "x".repeat(length - 8))
    }

    #[test]
    fn each_kind_lands_in_its_own_count_and_the_total() {
        let mut budget = budget(NativeProtocol::CodexAppServer, DispatchLimits::default());
        for (frame, kind) in ONE_OF_EACH_KIND {
            let (record, _) = budget.accept(frame.to_owned()).expect("record");
            assert_eq!(record.kind(), kind, "{frame}");
        }
        let summary = budget.summary();
        assert_eq!(
            summary.kinds,
            KindCounts {
                started: 1,
                text: 1,
                tool: 1,
                approval: 1,
                usage: 1,
                completed: 1,
                failed: 1,
                cancelled: 1,
                opaque: 1,
            }
        );
        assert_eq!(summary.kinds.total(), 9);
        assert_eq!(summary.record_count, 9);
    }

    #[test]
    fn counts_and_their_total_saturate() {
        let mut counts = KindCounts {
            started: u64::MAX,
            ..KindCounts::default()
        };
        counts.increment(ObservationKind::Started);
        assert_eq!(counts.started, u64::MAX);
        counts.increment(ObservationKind::Text);
        assert_eq!(counts.total(), u64::MAX);
    }

    #[test]
    fn the_capture_bound_is_the_byte_limit_and_admits_an_exact_fill() {
        let limits = DispatchLimits {
            frame_bytes: 64,
            capture_bytes: 128,
            ..DispatchLimits::default()
        };
        let mut budget = budget(NativeProtocol::Acp, limits);
        budget.accept(frame_of_length(64)).expect("first frame");
        budget.accept(frame_of_length(64)).expect("an exact fill");
        assert_eq!(
            budget.accept("{}".to_owned()).err(),
            Some(DispatchError::CaptureTooLarge)
        );
        assert_eq!(budget.summary().record_bytes, 128);
    }

    #[test]
    fn a_rejected_frame_leaves_no_trace() {
        let mut budget = budget(NativeProtocol::Acp, DispatchLimits::default());
        assert_eq!(
            budget.accept("not json".to_owned()).err(),
            Some(DispatchError::InvalidJson)
        );
        assert_eq!(
            budget.accept("[1]".to_owned()).err(),
            Some(DispatchError::InvalidRecord)
        );
        let empty = budget.summary();
        assert_eq!(empty.record_count, 0);
        assert_eq!(empty.record_bytes, 0);
        assert_eq!(empty.transcript_sha256, Sha256Digest::of(b""));
        let (record, _) = budget.accept("{}".to_owned()).expect("record");
        assert_eq!(record.sequence(), 1);
        let summary = budget.summary();
        assert_eq!(summary.record_bytes, 2);
        assert_eq!(summary.transcript_sha256, Sha256Digest::of(b"{}\n"));
    }

    #[test]
    fn summaries_round_trip_and_refuse_unknown_fields() {
        let mut budget = budget(NativeProtocol::Acp, DispatchLimits::default());
        budget.accept("{}".to_owned()).expect("record");
        let summary = budget.summary();
        let value = serde_json::to_value(summary).expect("summary value");
        assert_eq!(
            serde_json::from_value::<CaptureSummary>(value.clone()).expect("summary"),
            summary
        );
        let mut outer = value.clone();
        outer["prompt"] = json!("synthetic");
        assert!(serde_json::from_value::<CaptureSummary>(outer).is_err());
        let mut inner = value;
        inner["kinds"]["prompt"] = json!(1);
        assert!(serde_json::from_value::<CaptureSummary>(inner).is_err());
    }
}
