//! Typed decision operations on the single writer (ADR 034 §10).
//!
//! Each method runs one `SqliteStore` decision operation on the writer
//! thread. Recording is idempotent per decision id and a decided admission
//! replays for the request that consumed the decision, so the bounded retry
//! in `call_writer` is safe.

use uuid::Uuid;
use vibemux_decision::DecisionRecord;
use vibemux_store::{
    DecidedDispatchCommit, DecisionCommit, HarnessDispatchAdmission, StoredDecision,
};

use crate::{WriterError, WriterHandle};

impl WriterHandle {
    pub fn record_decision(&self, record: DecisionRecord) -> Result<DecisionCommit, WriterError> {
        self.with_store(move |store| store.record_decision(&record))
    }

    pub fn decision(&self, decision_id: Uuid) -> Result<Option<StoredDecision>, WriterError> {
        self.with_store(move |store| store.decision(decision_id))
    }

    pub fn decisions_after(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<StoredDecision>, WriterError> {
        self.with_store(move |store| store.decisions_after(after_sequence, limit))
    }

    pub fn admit_decided_dispatch(
        &self,
        decision_id: Uuid,
        admission: HarnessDispatchAdmission,
    ) -> Result<DecidedDispatchCommit, WriterError> {
        self.with_store(move |store| store.admit_decided_dispatch(decision_id, &admission))
    }
}
