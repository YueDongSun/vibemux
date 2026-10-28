//! Durable, content-free reservations for model effects. Only the writer owns these rows.
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    gateway::{ModelCallReceipt, RouteRole},
    model_call::{ModelCallAdmission, ModelCallRecord, ModelCallState},
    workflow_record::WorkflowRecord,
};

use super::{append, millis, workflow_error, workflow_rows};
use crate::{SqliteStore, StoreError};

// The exact one-call compiler namespace and ID derivation used by the daemon's
// model_compiler. Other model-call owner names retain their existing semantics.
const COMPILER_OWNER_PREFIX: &str = "compiler_";
const COMPILER_REQUEST_DOMAIN: &str = "vibemux.workflow.compiler_request.v1";
const MODEL_CALL_RECOVERY_PAGE_SIZE: u32 = 512;

/// A preparation-time accounting association. The original admission remains
/// unchanged, including workflow_id=None; only the owner row gains an aggregate.
pub(super) struct PreparedCompilerAssociation {
    workflow_id: Uuid,
    call: ModelCallRecord,
    remaining_model_requests: u32,
}

fn compiler_workflow_id(admission: &ModelCallAdmission) -> Result<Option<Uuid>, StoreError> {
    let Some(suffix) = admission
        .owner_key
        .as_str()
        .strip_prefix(COMPILER_OWNER_PREFIX)
    else {
        return Ok(None);
    };
    if suffix.len() != 32 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let workflow_id = Uuid::parse_str(suffix)
        .map_err(|_| workflow_error("model_call_compiler_association_invalid"))?;
    let digest = Sha256Digest::of_fields(
        COMPILER_REQUEST_DOMAIN,
        &[
            workflow_id.as_bytes(),
            admission.identity.request_fingerprint.as_bytes(),
        ],
    );
    let expected_call_id = Uuid::from_bytes(
        digest.as_bytes()[..16]
            .try_into()
            .map_err(|_| workflow_error("model_call_compiler_association_invalid"))?,
    );
    if workflow_id.is_nil()
        || suffix != workflow_id.simple().to_string()
        || admission.workflow_id.is_some()
        || admission.identity.role != RouteRole::Compiler
        || admission.max_calls != 1
        || admission.call_id != expected_call_id
    {
        return Err(workflow_error("model_call_compiler_association_invalid"));
    }
    Ok(Some(workflow_id))
}

/// Called only for a genuinely new prepare, inside its immediate transaction.
/// Every settled reservation is charged, including rejected, blocked, and unknown
/// calls. Pending work must first settle or be reconciled. No charge is refunded.
pub(super) fn charge_compiler_for_prepare(
    transaction: &Transaction<'_>,
    record: &WorkflowRecord,
) -> Result<(WorkflowRecord, Option<PreparedCompilerAssociation>), StoreError> {
    let owner_key = format!("{COMPILER_OWNER_PREFIX}{}", record.workflow_id.simple());
    let owner: Option<(Option<String>, u32, u64, u32)> = transaction.query_row(
        "SELECT workflow_id, max_calls, deadline_unix_ms, calls_used FROM workflow_model_call_owners WHERE owner_key=?",
        [&owner_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    let Some((workflow_id, max_calls, deadline, calls_used)) = owner else {
        return Ok((record.clone(), None));
    };
    if workflow_id.is_some() || max_calls != 1 || calls_used != 1 {
        return Err(workflow_error("model_call_compiler_association_invalid"));
    }
    // Read at most two IDs: one is required; a second proves a broken one-call fence.
    let call_ids: Vec<String> = {
        let mut statement = transaction
            .prepare("SELECT call_id FROM workflow_model_calls WHERE owner_key=? LIMIT 2")?;
        statement
            .query_map([&owner_key], |row| row.get(0))?
            .collect::<Result<_, _>>()?
    };
    if call_ids.len() != 1 {
        return Err(workflow_error("model_call_compiler_association_invalid"));
    }
    let call_id = Uuid::parse_str(&call_ids[0])
        .map_err(|_| workflow_error("model_call_compiler_association_invalid"))?;
    let call = load_call(transaction, call_id)?
        .ok_or_else(|| workflow_error("model_call_compiler_association_invalid"))?;
    if compiler_workflow_id(&call.admission)? != Some(record.workflow_id)
        || call.admission.owner_key.as_str() != owner_key
        || call.calls_used != calls_used
        || call.admission.deadline_unix_ms != deadline
    {
        return Err(workflow_error("model_call_compiler_association_invalid"));
    }
    if matches!(
        call.state,
        ModelCallState::Reserved | ModelCallState::Submitted
    ) {
        return Err(workflow_error("model_call_compiler_pending"));
    }
    let mut charged = record.clone();
    charged.budget.model_requests_remaining = charged
        .budget
        .model_requests_remaining
        .checked_sub(calls_used)
        .ok_or_else(|| workflow_error("model_call_limit_exceeded"))?;
    let association = PreparedCompilerAssociation {
        workflow_id: record.workflow_id,
        call,
        remaining_model_requests: charged.budget.model_requests_remaining,
    };
    Ok((charged, Some(association)))
}

/// Run after the workflow row has been inserted, in the same transaction. The
/// foreign key and CAS prevent orphan, repeated, or cross-workflow associations.
pub(super) fn associate_prepared_compiler(
    transaction: &Transaction<'_>,
    association: &PreparedCompilerAssociation,
    timestamp: OffsetDateTime,
) -> Result<(), StoreError> {
    let call = &association.call;
    let changed = transaction.execute(
        "UPDATE workflow_model_call_owners SET workflow_id=? WHERE owner_key=? AND workflow_id IS NULL AND max_calls=1 AND calls_used=1 AND deadline_unix_ms=?",
        params![association.workflow_id.to_string(), call.admission.owner_key.as_str(), call.admission.deadline_unix_ms])?;
    if changed != 1 {
        return Err(workflow_error("model_call_compiler_association_invalid"));
    }
    append(
        transaction,
        "workflow_compiler_call_associated",
        format!(
            "workflow_compiler_call:{}:{}",
            association.workflow_id, call.call_id
        ),
        serde_json::json!({
            "workflow_id": association.workflow_id, "call_id": call.call_id,
            "owner_key": call.admission.owner_key, "state": call.state,
            "request_fingerprint": call.admission.identity.request_fingerprint,
            "charged_model_requests": call.calls_used,
            "model_requests_remaining": association.remaining_model_requests,
        }),
        timestamp,
    )?;
    Ok(())
}

pub(super) fn load_call(
    connection: &Connection,
    call_id: Uuid,
) -> Result<Option<ModelCallRecord>, StoreError> {
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT owner_key, state, record_json FROM workflow_model_calls WHERE call_id = ?",
            [call_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    row.map(|(owner, state, json)| {
        let record: ModelCallRecord = serde_json::from_str(&json)
            .map_err(|_| workflow_error("store_workflow_projection_mismatch"))?;
        record
            .validate()
            .map_err(|error| workflow_error(error.code()))?;
        if record.call_id != call_id
            || record.admission.owner_key.as_str() != owner
            || state != state_key(record.state)
        {
            return Err(workflow_error("store_workflow_projection_mismatch"));
        }
        Ok(record)
    })
    .transpose()
}

fn state_key(state: ModelCallState) -> &'static str {
    match state {
        ModelCallState::Reserved => "reserved",
        ModelCallState::Submitted => "submitted",
        ModelCallState::Completed => "completed",
        ModelCallState::Rejected => "rejected",
        ModelCallState::Blocked => "blocked",
        ModelCallState::Unknown => "unknown",
    }
}

fn persist(
    transaction: &Transaction<'_>,
    record: &ModelCallRecord,
    kind: &str,
) -> Result<(), StoreError> {
    let (_, sequence) = append(
        transaction,
        kind,
        format!("model_call:{}:{}", record.call_id, state_key(record.state)),
        serde_json::json!({"call_id": record.call_id, "owner_key": record.admission.owner_key,
            "workflow_id": record.admission.workflow_id, "state": record.state,
            "request_fingerprint": record.admission.identity.request_fingerprint,
            "role": record.admission.identity.role, "calls_used": record.calls_used}),
        OffsetDateTime::now_utc(),
    )?;
    transaction.execute("UPDATE workflow_model_calls SET state=?, record_json=?, updated_sequence=? WHERE call_id=?",
        params![state_key(record.state), serde_json::to_string(record)?, sequence, record.call_id.to_string()])?;
    Ok(())
}

pub(super) fn reserve_model_call_in(
    transaction: &Transaction<'_>,
    admission: &ModelCallAdmission,
) -> Result<ModelCallRecord, StoreError> {
    admission
        .validate()
        .map_err(|error| workflow_error(error.code()))?;
    if admission.call_id.is_nil() || admission.workflow_id.is_some_and(|id| id.is_nil()) {
        return Err(workflow_error("model_call_admission_invalid"));
    }
    let compiler_workflow = compiler_workflow_id(admission)?;
    if let Some(record) = load_call(transaction, admission.call_id)? {
        if !record.matches_admission(admission) {
            return Err(workflow_error("model_call_identity_mismatch"));
        }
        return Ok(record);
    }
    if let Some(workflow_id) = compiler_workflow
        && workflow_rows::load_workflow(transaction, workflow_id)?.is_some()
    {
        return Err(workflow_error("model_call_compiler_workflow_prepared"));
    }
    let now = OffsetDateTime::now_utc();
    if millis(now) >= admission.deadline_unix_ms {
        return Err(workflow_error("model_call_deadline_exceeded"));
    }
    let owner: Option<(Option<String>, u32, u64, u32)> = transaction.query_row(
            "SELECT workflow_id, max_calls, deadline_unix_ms, calls_used FROM workflow_model_call_owners WHERE owner_key=?",
            [admission.owner_key.as_str()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    let used = match owner {
        Some((workflow, max_calls, deadline, used)) => {
            if workflow != admission.workflow_id.map(|id| id.to_string())
                || max_calls != admission.max_calls
                || deadline != admission.deadline_unix_ms
            {
                return Err(workflow_error("model_call_budget_identity_mismatch"));
            }
            used
        }
        None => {
            transaction.execute("INSERT INTO workflow_model_call_owners(owner_key, workflow_id, max_calls, deadline_unix_ms, calls_used) VALUES (?, ?, ?, ?, 0)",
                    params![admission.owner_key.as_str(), admission.workflow_id.map(|id| id.to_string()), admission.max_calls, admission.deadline_unix_ms])?;
            0
        }
    };
    let next_used = used
        .checked_add(1)
        .filter(|count| *count <= admission.max_calls)
        .ok_or_else(|| workflow_error("model_call_limit_exceeded"))?;
    let record = ModelCallRecord::reserved(admission.clone(), next_used)
        .map_err(|error| workflow_error(error.code()))?;
    let (_, sequence) = append(
        transaction,
        "workflow_model_call_reserved",
        format!("model_call:{}:reserved", record.call_id),
        serde_json::json!({"call_id": record.call_id, "owner_key": admission.owner_key, "workflow_id": admission.workflow_id,
                "role": admission.identity.role, "request_fingerprint": admission.identity.request_fingerprint, "calls_used": next_used}),
        now,
    )?;
    if let Some(workflow_id) = admission.workflow_id {
        let stored = workflow_rows::load_existing_workflow(transaction, workflow_id)?;
        let mut workflow = stored.record.clone();
        if !workflow.phase.admits_work()
            || admission.deadline_unix_ms > workflow.budget.deadline_unix_ms
        {
            return Err(workflow_error("store_workflow_not_running"));
        }
        workflow.budget.model_requests_remaining = workflow
            .budget
            .model_requests_remaining
            .checked_sub(1)
            .ok_or_else(|| workflow_error("model_call_limit_exceeded"))?;
        workflow.version = workflow
            .version
            .checked_add(1)
            .ok_or_else(|| workflow_error("workflow_record_invalid"))?;
        workflow.updated_at_ms = millis(now).max(workflow.updated_at_ms);
        workflow_rows::update_workflow(transaction, &workflow, stored.record.version, sequence)?;
    }
    transaction.execute(
        "UPDATE workflow_model_call_owners SET calls_used=? WHERE owner_key=?",
        params![next_used, admission.owner_key.as_str()],
    )?;
    transaction.execute("INSERT INTO workflow_model_calls(call_id, owner_key, state, record_json, updated_sequence) VALUES (?, ?, 'reserved', ?, ?)",
            params![record.call_id.to_string(), admission.owner_key.as_str(), serde_json::to_string(&record)?, sequence])?;
    Ok(record)
}

impl SqliteStore {
    /// Bounded read of the existing one-call compiler namespace, without admission.
    pub fn compiler_model_call(
        &self,
        workflow_id: Uuid,
    ) -> Result<Option<ModelCallRecord>, StoreError> {
        if workflow_id.is_nil() {
            return Err(workflow_error("model_call_compiler_association_invalid"));
        }
        let owner_key = format!("{COMPILER_OWNER_PREFIX}{}", workflow_id.simple());
        let mut statement = self
            .connection
            .prepare("SELECT call_id FROM workflow_model_calls WHERE owner_key=? LIMIT 2")?;
        let call_ids: Vec<String> = statement
            .query_map([&owner_key], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        if call_ids.len() > 1 {
            return Err(workflow_error("model_call_compiler_association_invalid"));
        }
        let Some(call_id) = call_ids.first() else {
            return Ok(None);
        };
        let call_id = Uuid::parse_str(call_id)
            .map_err(|_| workflow_error("model_call_compiler_association_invalid"))?;
        let call = load_call(&self.connection, call_id)?
            .ok_or_else(|| workflow_error("model_call_compiler_association_invalid"))?;
        if compiler_workflow_id(&call.admission)? != Some(workflow_id)
            || call.admission.owner_key.as_str() != owner_key
        {
            return Err(workflow_error("model_call_compiler_association_invalid"));
        }
        Ok(Some(call))
    }

    pub fn model_call(&self, call_id: Uuid) -> Result<Option<ModelCallRecord>, StoreError> {
        load_call(&self.connection, call_id)
    }

    pub fn reserve_model_call(
        &mut self,
        admission: &ModelCallAdmission,
    ) -> Result<ModelCallRecord, StoreError> {
        let transaction = self.immediate()?;
        let record = reserve_model_call_in(&transaction, admission)?;
        transaction.commit()?;
        Ok(record)
    }
    pub fn submit_model_call(&mut self, call_id: Uuid) -> Result<ModelCallRecord, StoreError> {
        let transaction = self.immediate()?;
        let mut record = load_call(&transaction, call_id)?
            .ok_or_else(|| workflow_error("model_call_unknown"))?;
        if millis(OffsetDateTime::now_utc()) >= record.admission.deadline_unix_ms {
            return Err(workflow_error("model_call_deadline_exceeded"));
        }
        record
            .mark_submitted()
            .map_err(|error| workflow_error(error.code()))?;
        persist(&transaction, &record, "workflow_model_call_submitted")?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn finish_model_call(
        &mut self,
        call_id: Uuid,
        receipt: Option<ModelCallReceipt>,
        error_code: Option<String>,
    ) -> Result<ModelCallRecord, StoreError> {
        let transaction = self.immediate()?;
        let mut record = load_call(&transaction, call_id)?
            .ok_or_else(|| workflow_error("model_call_unknown"))?;
        let error = error_code
            .map(SpecIdentifier::new)
            .transpose()
            .map_err(|_| workflow_error("model_call_finish_invalid"))?;
        if matches!(
            record.state,
            ModelCallState::Completed
                | ModelCallState::Rejected
                | ModelCallState::Blocked
                | ModelCallState::Unknown
        ) {
            if record.receipt == receipt && record.error_code == error {
                return Ok(record);
            }
            return Err(workflow_error("model_call_finish_conflict"));
        }
        record
            .finish(receipt, error)
            .map_err(|error| workflow_error(error.code()))?;
        record
            .validate()
            .map_err(|error| workflow_error(error.code()))?;
        persist(&transaction, &record, "workflow_model_call_finished")?;
        transaction.commit()?;
        Ok(record)
    }

    /// Recovery retains every charge and blocks every possibly submitted call.
    pub fn recover_model_calls(&mut self) -> Result<u32, StoreError> {
        let transaction = self.immediate()?;
        let mut recovered = 0_u32;
        let mut after_call_id = String::new();
        loop {
            let ids: Vec<String> = {
                let mut query = transaction.prepare(
                    "SELECT call_id FROM workflow_model_calls WHERE state='submitted' AND call_id > ? ORDER BY call_id LIMIT ?",
                )?;
                query
                    .query_map(
                        params![after_call_id, MODEL_CALL_RECOVERY_PAGE_SIZE],
                        |row| row.get(0),
                    )?
                    .collect::<Result<_, _>>()?
            };
            if ids.is_empty() {
                break;
            }
            for id in &ids {
                let id = Uuid::parse_str(id)
                    .map_err(|_| workflow_error("store_workflow_projection_mismatch"))?;
                let mut record = load_call(&transaction, id)?
                    .ok_or_else(|| workflow_error("model_call_unknown"))?;
                if record
                    .recover_after_restart()
                    .map_err(|error| workflow_error(error.code()))?
                {
                    persist(&transaction, &record, "workflow_model_call_recovered")?;
                    recovered = recovered
                        .checked_add(1)
                        .ok_or_else(|| workflow_error("model_call_limit_exceeded"))?;
                }
            }
            after_call_id = ids
                .last()
                .cloned()
                .ok_or_else(|| workflow_error("model_call_unknown"))?;
        }
        transaction.commit()?;
        Ok(recovered)
    }
}
