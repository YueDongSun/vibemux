//! Advisory decision records (ADR 034 §10): store schema 6.
//!
//! A decision is recorded once with its `decision_recorded` event and is
//! consumed at most once, by the dispatch admission it authorized, in the
//! same immediate transaction that admits that dispatch. The record is
//! content-free; the prompt is known only by its digest.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;
use vibemux_decision::{
    DecisionError, DecisionRecord, DecisionStatus,
    events::{self as decision_events, ConsumedContext, DecisionConsumedPayload},
    record::DispatchBinding,
};
use vibemux_events::EventEnvelope;
use vibemux_harness::dispatch::DispatchError;

use crate::harness_dispatch::admit_dispatch_in;
use crate::{
    HarnessDispatchAdmission, HarnessDispatchCommit, SqliteStore, StoreError,
    resolve_or_mint_project_id,
};

/// Largest page `decisions_after` returns.
pub const MAX_DECISION_PAGE: usize = 100;

/// Adds `decision_records`. `migrate` runs it and the version marker in one
/// immediate transaction, so a failure leaves the database at schema 5. The
/// unique `consumed_request_id` binds a decision to at most one dispatch and
/// a dispatch to at most one decision.
pub(crate) const MIGRATION_V6: &str = r#"
CREATE TABLE decision_records (
    decision_id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK(status IN ('resolved', 'escalated')),
    consumed_request_id TEXT UNIQUE REFERENCES harness_dispatches(request_id),
    record_json TEXT NOT NULL,
    created_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence),
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
    CHECK(status = 'resolved' OR consumed_request_id IS NULL)
);
INSERT INTO schema_migrations(version, applied_at)
VALUES (6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
"#;

/// A validated decision with the sequences of the events that created and
/// last changed it.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredDecision {
    pub record: DecisionRecord,
    pub created_sequence: u64,
    pub updated_sequence: u64,
}

/// Result of recording a decision.
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionCommit {
    pub decision: StoredDecision,
    /// `None` for an idempotent repeat that wrote nothing.
    pub event: Option<EventEnvelope>,
}

/// Result of a decided dispatch admission.
#[derive(Clone, Debug, PartialEq)]
pub struct DecidedDispatchCommit {
    pub dispatch: HarnessDispatchCommit,
    pub decision: StoredDecision,
    /// The `decision_consumed` event; `None` when the same request replayed.
    pub consumed_event: Option<EventEnvelope>,
}

impl SqliteStore {
    /// Records a new, unconsumed decision with its `decision_recorded` event.
    /// Recording the identical record again writes nothing; a different
    /// record under the same id is a conflict.
    pub fn record_decision(
        &mut self,
        record: &DecisionRecord,
    ) -> Result<DecisionCommit, StoreError> {
        record.validate().map_err(StoreError::Decision)?;
        if record.consumption.is_some() {
            return Err(StoreError::Decision(DecisionError::InvalidRequest));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(stored) = load(&transaction, record.decision_id)? {
            if stored.record != *record {
                return Err(StoreError::DecisionConflict);
            }
            return Ok(DecisionCommit {
                decision: stored,
                event: None,
            });
        }
        let project_id = resolve_or_mint_project_id(&transaction)?;
        let draft = decision_events::recorded(project_id, record).map_err(StoreError::Decision)?;
        let (event, raw_sequence) = Self::insert_event(&transaction, draft)?;
        transaction.execute(
            "INSERT INTO decision_records(decision_id, status, consumed_request_id, record_json, created_sequence, updated_sequence) VALUES (?, ?, NULL, ?, ?, ?)",
            params![
                decision_key(record.decision_id),
                status_key(record.status),
                serde_json::to_string(record)?,
                raw_sequence,
                raw_sequence
            ],
        )?;
        transaction.commit()?;
        let sequence = event.sequence().get();
        Ok(DecisionCommit {
            decision: StoredDecision {
                record: record.clone(),
                created_sequence: sequence,
                updated_sequence: sequence,
            },
            event: Some(event),
        })
    }

    /// The validated decision for `decision_id`, if one was recorded.
    pub fn decision(&self, decision_id: Uuid) -> Result<Option<StoredDecision>, StoreError> {
        load(&self.connection, decision_id)
    }

    /// Up to `limit` decisions created after `after_sequence`, oldest first.
    pub fn decisions_after(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<StoredDecision>, StoreError> {
        if !(1..=MAX_DECISION_PAGE).contains(&limit) {
            return Err(StoreError::Decision(DecisionError::InvalidRequest));
        }
        let after = i64::try_from(after_sequence).unwrap_or(i64::MAX);
        let page = i64::try_from(limit).unwrap_or(1);
        let mut statement = self.connection.prepare(
            "SELECT decision_id, status, consumed_request_id, record_json, created_sequence, updated_sequence FROM decision_records WHERE created_sequence > ? ORDER BY created_sequence LIMIT ?",
        )?;
        let rows = statement.query_map(params![after, page], persisted_row)?;
        let mut decisions = Vec::new();
        for row in rows {
            decisions.push(validated(row?)?);
        }
        Ok(decisions)
    }

    /// Admits the dispatch a decision authorized and consumes the decision in
    /// one immediate transaction. Every ADR 029 admission gate still runs.
    /// The decision must be resolved, unexpired, unconsumed, and recorded
    /// for the same harness, prompt, and dispatch configuration. The request
    /// that consumed a decision may repeat and writes nothing; any other
    /// request is refused. Nothing is written on any rejection.
    pub fn admit_decided_dispatch(
        &mut self,
        decision_id: Uuid,
        admission: &HarnessDispatchAdmission,
    ) -> Result<DecidedDispatchCommit, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = load(&transaction, decision_id)?
            .ok_or(StoreError::Decision(DecisionError::NotFound))?;
        let binding = stored
            .record
            .check_dispatchable(
                admission.request_id,
                admission.prompt,
                admission.config_sha256,
                admission.timestamp,
            )
            .map_err(StoreError::Decision)?;
        let (DispatchBinding::Fresh(harness) | DispatchBinding::Replay(harness)) = binding;
        if harness != admission.harness {
            return Err(StoreError::Decision(DecisionError::HarnessMismatch));
        }
        let dispatch = admit_dispatch_in(&transaction, admission)?;
        if matches!(binding, DispatchBinding::Replay(_)) {
            // The consuming admission committed with the consumption, so a
            // replay must find it unchanged.
            if dispatch.event.is_some() {
                return Err(StoreError::DecisionProjectionMismatch);
            }
            return Ok(DecidedDispatchCommit {
                dispatch,
                decision: stored,
                consumed_event: None,
            });
        }
        if dispatch.event.is_none() {
            // The request was admitted earlier without this decision.
            return Err(StoreError::HarnessDispatch(DispatchError::Conflict));
        }
        let mut record = stored.record;
        record
            .mark_consumed(admission.request_id, admission.timestamp)
            .map_err(StoreError::Decision)?;
        let context = ConsumedContext {
            project_id: dispatch.record.project_id,
            task_id: dispatch.record.task_id,
            run_id: dispatch.record.run_id,
            timestamp: admission.timestamp,
        };
        let payload = DecisionConsumedPayload {
            decision_id,
            request_id: admission.request_id,
            harness,
            consumed_at: admission.timestamp,
        };
        let draft = decision_events::consumed(&context, &payload).map_err(StoreError::Decision)?;
        let (event, raw_sequence) = Self::insert_event(&transaction, draft)?;
        let previous_sequence =
            i64::try_from(stored.updated_sequence).map_err(|_| StoreError::InvalidSequence)?;
        let changed = transaction.execute(
            "UPDATE decision_records SET consumed_request_id = ?, record_json = ?, updated_sequence = ? WHERE decision_id = ? AND consumed_request_id IS NULL AND updated_sequence = ?",
            params![
                request_key(admission.request_id),
                serde_json::to_string(&record)?,
                raw_sequence,
                decision_key(decision_id),
                previous_sequence
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::DecisionConflict);
        }
        transaction.commit()?;
        Ok(DecidedDispatchCommit {
            dispatch,
            decision: StoredDecision {
                record,
                created_sequence: stored.created_sequence,
                updated_sequence: event.sequence().get(),
            },
            consumed_event: Some(event),
        })
    }
}

struct PersistedRow {
    decision_id: String,
    status: String,
    consumed_request_id: Option<String>,
    record_json: String,
    created_sequence: u64,
    updated_sequence: u64,
}

fn persisted_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersistedRow> {
    Ok(PersistedRow {
        decision_id: row.get(0)?,
        status: row.get(1)?,
        consumed_request_id: row.get(2)?,
        record_json: row.get(3)?,
        created_sequence: row.get(4)?,
        updated_sequence: row.get(5)?,
    })
}

fn load(connection: &Connection, decision_id: Uuid) -> Result<Option<StoredDecision>, StoreError> {
    connection
        .query_row(
            "SELECT decision_id, status, consumed_request_id, record_json, created_sequence, updated_sequence FROM decision_records WHERE decision_id = ?",
            [decision_key(decision_id)],
            persisted_row,
        )
        .optional()?
        .map(validated)
        .transpose()
}

/// Cross-checks the indexed columns against the record JSON and the record
/// against its own invariants; any disagreement fails closed.
fn validated(row: PersistedRow) -> Result<StoredDecision, StoreError> {
    let record: DecisionRecord = serde_json::from_str(&row.record_json)
        .map_err(|_| StoreError::DecisionProjectionMismatch)?;
    let consistent = record.validate().is_ok()
        && row.decision_id == decision_key(record.decision_id)
        && row.status == status_key(record.status)
        && row.consumed_request_id
            == record
                .consumption
                .map(|consumption| request_key(consumption.request_id))
        && row.created_sequence <= row.updated_sequence;
    if !consistent {
        return Err(StoreError::DecisionProjectionMismatch);
    }
    Ok(StoredDecision {
        record,
        created_sequence: row.created_sequence,
        updated_sequence: row.updated_sequence,
    })
}

fn decision_key(decision_id: Uuid) -> String {
    decision_id.as_hyphenated().to_string()
}

/// The same key `harness_dispatches.request_id` uses.
fn request_key(request_id: Uuid) -> String {
    request_id.as_hyphenated().to_string()
}

const fn status_key(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Resolved => "resolved",
        DecisionStatus::Escalated => "escalated",
    }
}
