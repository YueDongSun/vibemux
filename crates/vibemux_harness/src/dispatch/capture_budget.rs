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
