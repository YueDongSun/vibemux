//! Durable, content-free operator submissions (ADR 074).
//! Admission precedes effects. A lost owner is unknown, never replayed.

use super::*;
use crate::sql_integer::{from_sql_u64, to_sql_u64};

pub(crate) const MIGRATION_V15: &str = r#"
CREATE TABLE workflow_operations (
    event_id TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    state TEXT NOT NULL,
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE INDEX workflow_operations_state ON workflow_operations(state);
INSERT INTO schema_migrations(version, applied_at)
VALUES (15, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
"#;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowOperationState {
    Accepted,
    Preparing,
    Prepared,
    Running,
    Blocked,
    Failed,
    Unknown,
}

impl WorkflowOperationState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Preparing => "preparing",
            Self::Prepared => "prepared",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Accepted, Self::Preparing | Self::Unknown)
                | (
                    Self::Preparing,
                    Self::Prepared | Self::Running | Self::Blocked | Self::Failed | Self::Unknown
                )
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOperationRecord {
    pub schema_version: u32,
    pub event_id: Uuid,
    /// Fixed operation kind, never a caller-selected authority.
    pub kind: String,
    pub state: WorkflowOperationState,
    pub fingerprint: Sha256Digest,
    pub workflow_id: Option<Uuid>,
    pub start_contract: Option<Sha256Digest>,
    pub code: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkflowOperationCommit {
    pub operation: WorkflowOperationRecord,
    pub duplicate: bool,
}

fn load_operation(
    connection: &rusqlite::Connection,
    event_id: Uuid,
) -> Result<Option<WorkflowOperationRecord>, StoreError> {
    let row: Option<(String, String, String, u64)> = connection.query_row(
        "SELECT fingerprint,state,record_json,updated_sequence FROM workflow_operations WHERE event_id=?",
        [uuid_key(event_id)],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,from_sql_u64(row.get::<_, i64>(3)?)?)),
    ).optional()?;
    row.map(|(fingerprint, state, encoded, sequence)| {
        let operation: WorkflowOperationRecord = serde_json::from_str(&encoded)?;
        let event_json: String = connection.query_row(
            "SELECT envelope_json FROM events WHERE sequence=?",
            [to_sql_u64(sequence)?],
            |row| row.get(0),
        )?;
        let event = EventEnvelope::from_json_slice(event_json.as_bytes())?;
        if operation.schema_version != 1
            || operation.event_id != event_id
            || operation.kind != "goal"
            || operation.fingerprint.to_hex() != fingerprint
            || operation.state.as_str() != state
            || event.sequence().get() != sequence
            || event.event_type().as_str() != "workflow_operation_updated"
            || event.payload().value() != &serde_json::to_value(&operation)?
        {
            return Err(workflow_error("workflow_operation_projection_mismatch"));
        }
        Ok(operation)
    })
    .transpose()
}

impl SqliteStore {
    pub fn workflow_operation(
        &self,
        event_id: Uuid,
    ) -> Result<Option<WorkflowOperationRecord>, StoreError> {
        load_operation(&self.connection, event_id)
    }

    pub fn admit_workflow_operation(
        &mut self,
        event_id: Uuid,
        fingerprint: Sha256Digest,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowOperationCommit, StoreError> {
        if event_id.is_nil() {
            return Err(workflow_error("workflow_operation_invalid"));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(operation) = load_operation(&transaction, event_id)? {
            if operation.fingerprint != fingerprint {
                return Err(workflow_error("workflow_operation_conflict"));
            }
            return Ok(WorkflowOperationCommit {
                operation,
                duplicate: true,
            });
        }
        let operation = WorkflowOperationRecord {
            schema_version: 1,
            event_id,
            kind: "goal".into(),
            state: WorkflowOperationState::Accepted,
            fingerprint,
            workflow_id: None,
            start_contract: None,
            code: None,
            updated_at_ms: millis(timestamp),
        };
        let (_, sequence) = append(
            &transaction,
            "workflow_operation_updated",
            format!("workflow_operation:{}:accepted", uuid_key(event_id)),
            serde_json::to_value(&operation)?,
            timestamp,
        )?;
        transaction.execute(
            "INSERT INTO workflow_operations(event_id,fingerprint,state,record_json,updated_sequence) VALUES (?,?,?,?,?)",
            params![uuid_key(event_id), fingerprint.to_hex(), operation.state.as_str(), serde_json::to_string(&operation)?, sequence],
        )?;
        transaction.commit()?;
        Ok(WorkflowOperationCommit {
            operation,
            duplicate: false,
        })
    }

    pub fn finish_workflow_operation(
        &mut self,
        event_id: Uuid,
        state: WorkflowOperationState,
        workflow_id: Option<Uuid>,
        start_contract: Option<Sha256Digest>,
        code: Option<String>,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowOperationRecord, StoreError> {
        if code.as_ref().is_some_and(|code| {
            code.is_empty()
                || code.len() > 128
                || !code
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        }) {
            return Err(workflow_error("workflow_operation_invalid"));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut operation = load_operation(&transaction, event_id)?
            .ok_or_else(|| workflow_error("workflow_operation_not_found"))?;
        if operation.state == state
            && operation.workflow_id == workflow_id
            && operation.start_contract == start_contract
            && operation.code == code
        {
            return Ok(operation);
        }
        if !operation.state.allows(state) {
            return Err(workflow_error("workflow_operation_invalid_transition"));
        }
        if let Some(id) = workflow_id {
            let workflow = workflow_rows::load_workflow(&transaction, id)?
                .ok_or_else(|| workflow_error("store_workflow_not_found"))?;
            if workflow.record.request_key != format!("goal_{}", event_id.simple()) {
                return Err(workflow_error("workflow_operation_scope_mismatch"));
            }
        }
        if matches!(
            state,
            WorkflowOperationState::Prepared | WorkflowOperationState::Running
        ) && (workflow_id.is_none() || start_contract.is_none())
        {
            return Err(workflow_error("workflow_operation_invalid"));
        }
        operation.state = state;
        operation.workflow_id = workflow_id;
        operation.start_contract = start_contract;
        operation.code = code;
        operation.updated_at_ms = millis(timestamp).max(operation.updated_at_ms);
        let (_, sequence) = append(
            &transaction,
            "workflow_operation_updated",
            format!(
                "workflow_operation:{}:{}",
                uuid_key(event_id),
                state.as_str()
            ),
            serde_json::to_value(&operation)?,
            timestamp,
        )?;
        transaction.execute(
            "UPDATE workflow_operations SET state=?,record_json=?,updated_sequence=? WHERE event_id=?",
            params![state.as_str(), serde_json::to_string(&operation)?, sequence, uuid_key(event_id)],
        )?;
        transaction.commit()?;
        Ok(operation)
    }

    /// Runs only during daemon startup, before new owners may be admitted.
    /// Interrupted requests are retained for explicit operator reconciliation.
    pub fn recover_workflow_operations(
        &mut self,
        timestamp: OffsetDateTime,
    ) -> Result<usize, StoreError> {
        let mut recovered = 0;
        loop {
            let ids = {
                let mut query = self.connection.prepare(
                    "SELECT event_id FROM workflow_operations WHERE state IN ('accepted','preparing') ORDER BY event_id LIMIT 256")?;
                query
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            if ids.is_empty() {
                break;
            }
            for id in &ids {
                let event_id = Uuid::parse_str(id)
                    .map_err(|_| workflow_error("workflow_operation_projection_mismatch"))?;
                let workflow =
                    self.workflow_by_request_key(&format!("goal_{}", event_id.simple()))?;
                self.finish_workflow_operation(
                    event_id,
                    WorkflowOperationState::Unknown,
                    workflow.map(|record| record.workflow_id),
                    None,
                    Some("workflow_operation_owner_lost".into()),
                    timestamp,
                )?;
                recovered += 1;
            }
        }
        Ok(recovered)
    }
}
