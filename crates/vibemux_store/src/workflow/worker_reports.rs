//! Append-only, content-free worker claims (ADR 057, store schema 10).

use super::*;
use crate::sql_integer::{from_sql_u64, to_sql_u64};
use vibemux_workflow::worker_report::{WorkerReport, WorkerReportRecord};

pub(crate) const MIGRATION_V10: &str = r#"
CREATE TABLE workflow_worker_reports (
    request_id TEXT PRIMARY KEY REFERENCES workflow_attempts(request_id),
    workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id),
    record_json TEXT NOT NULL,
    created_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence)
);
CREATE INDEX workflow_worker_reports_page ON workflow_worker_reports(workflow_id, created_sequence);
INSERT INTO schema_migrations(version, applied_at)
VALUES (10, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
"#;

pub const MAX_WORKER_REPORT_PAGE: usize = 8;

#[derive(Clone, Debug, Serialize)]
pub struct WorkerReportPage {
    pub items: Vec<(u64, WorkerReportRecord)>,
    pub next: u64,
    pub more: bool,
}

fn load_report(
    connection: &rusqlite::Connection,
    request_id: Uuid,
) -> Result<Option<WorkerReportRecord>, StoreError> {
    let row: Option<(String, String, u64)> = connection
        .query_row(
            "SELECT record_json,workflow_id,created_sequence FROM workflow_worker_reports WHERE request_id=?",
            [uuid_key(request_id)],
            |row| Ok((row.get(0)?,row.get(1)?,from_sql_u64(row.get::<_, i64>(2)?)?)),
        )
        .optional()?;
    row.map(|(json, workflow, sequence)| {
        let record: WorkerReportRecord = serde_json::from_str(&json)?;
        if record.request_id != request_id || uuid_key(record.workflow_id) != workflow {
            return Err(workflow_error("workflow_worker_report_projection_mismatch"));
        }
        verify_record(connection, &record, sequence)?;
        Ok(record)
    })
    .transpose()
}

fn verify_record(
    connection: &rusqlite::Connection,
    record: &WorkerReportRecord,
    sequence: u64,
) -> Result<(), StoreError> {
    let encoded: String = connection.query_row(
        "SELECT envelope_json FROM events WHERE sequence=?",
        [to_sql_u64(sequence)?],
        |row| row.get(0),
    )?;
    let event = EventEnvelope::from_json_slice(encoded.as_bytes())?;
    let attempt = workflow_rows::load_attempt(connection, record.request_id)?
        .ok_or_else(|| workflow_error("workflow_worker_report_projection_mismatch"))?;
    if record.schema_version != 1
        || event.sequence().get() != sequence
        || event.event_type().as_str() != "workflow_worker_reported"
        || event.payload().value() != &serde_json::to_value(record)?
        || record.workflow_id != attempt.workflow_id
        || record.task_key != attempt.task_key
        || record.session_id != attempt.session_id
        || record.contract_id != attempt.contract_id
        || record.lease_id != attempt.lease_id
        || record.lease_generation != attempt.lease_generation
        || attempt.purpose == AttemptPurpose::Review
    {
        return Err(workflow_error("workflow_worker_report_projection_mismatch"));
    }
    Ok(())
}

/// An attention claim remains a gate after a crash or operator restart. A
/// later report from this exact session and contract may resolve it.
pub(super) fn check_attention(
    connection: &rusqlite::Connection,
    workflow_id: Uuid,
    session_id: Uuid,
    contract_id: Sha256Digest,
) -> Result<(), StoreError> {
    let request: Option<String> = connection.query_row(
        "SELECT r.request_id FROM workflow_worker_reports r JOIN workflow_attempts a ON a.request_id=r.request_id WHERE r.workflow_id=? AND json_extract(a.record_json,'$.session_id')=? AND a.contract_id=? ORDER BY r.created_sequence DESC LIMIT 1",
        params![uuid_key(workflow_id), session_id.to_string(), contract_id.to_hex()], |row| row.get(0)).optional()?;
    let report = request
        .map(|id| {
            let id = Uuid::parse_str(&id)
                .map_err(|_| workflow_error("workflow_worker_report_projection_mismatch"))?;
            load_report(connection, id)
        })
        .transpose()?
        .flatten();
    let contract = workflow_rows::load_contract(connection, contract_id)?;
    if report.as_ref().is_some_and(|report| report.needs_attention)
        || (contract.is_some_and(|contract| contract.template_version == "worker_v2")
            && report.is_none())
    {
        return Err(workflow_error("workflow_worker_report_attention_required"));
    }
    if report.as_ref().is_some_and(|report| {
        report.state != vibemux_workflow::worker_report::WorkerReportState::Ready
    }) {
        return Err(workflow_error("workflow_worker_report_not_ready"));
    }
    Ok(())
}

impl SqliteStore {
    /// The process must have completed the exact worker attempt under its
    /// still-active lease. A report is a claim, never an acceptance receipt.
    pub fn record_workflow_worker_report(
        &mut self,
        request_id: Uuid,
        fence: LeaseFence,
        report: &WorkerReport,
        timestamp: OffsetDateTime,
    ) -> Result<WorkerReportRecord, StoreError> {
        if !report.valid() {
            return Err(workflow_error("workflow_worker_report_invalid"));
        }
        let digest = report
            .digest()
            .map_err(|_| workflow_error("workflow_worker_report_invalid"))?;
        let transaction = self.immediate()?;
        if let Some(existing) = load_report(&transaction, request_id)? {
            if existing.report_digest != digest
                || existing.lease_id != fence.lease_id
                || existing.lease_generation != fence.generation
            {
                return Err(workflow_error("workflow_worker_report_conflict"));
            }
            return Ok(existing);
        }
        let attempt = workflow_rows::load_attempt(&transaction, request_id)?
            .ok_or_else(|| workflow_error("store_workflow_unknown_attempt"))?;
        let workflow =
            workflow_rows::load_existing_workflow(&transaction, attempt.workflow_id)?.record;
        let lease = workflow_rows::load_lease(&transaction, fence.lease_id)?
            .ok_or_else(|| workflow_error("store_workflow_unknown_lease"))?;
        let task = workflow
            .task(&attempt.task_key)
            .ok_or_else(|| workflow_error("store_workflow_unknown_task"))?;
        vibemux_workflow::leases::check_fence(lease.view(), fence.generation)
            .map_err(|error| workflow_error(error.code()))?;
        if lease.state != LeaseState::Active
            || workflow.phase.is_terminal()
            || workflow.phase == WorkflowPhase::CancelRequested
        {
            return Err(workflow_error("workflow_worker_report_not_active"));
        }
        if attempt.purpose == AttemptPurpose::Review
            || attempt.execution.is_none()
            || lease.workflow_id != workflow.workflow_id
            || lease.task_key != attempt.task_key
            || attempt.lease_id != fence.lease_id
            || attempt.lease_generation != fence.generation
            || task.contract_id != attempt.contract_id
        {
            return Err(workflow_error("workflow_worker_report_attempt_mismatch"));
        }
        if workflow_rows::dispatch_phase(&transaction, request_id)?.as_deref()
            != Some(DispatchPhase::Completed.as_str())
        {
            return Err(workflow_error("workflow_worker_report_attempt_unsettled"));
        }
        let record = WorkerReportRecord {
            schema_version: 1,
            workflow_id: workflow.workflow_id,
            request_id,
            session_id: attempt.session_id,
            task_key: attempt.task_key,
            contract_id: attempt.contract_id,
            lease_id: fence.lease_id,
            lease_generation: fence.generation,
            state: report.state,
            report_digest: digest,
            content_sha256: Sha256Digest::of(&serde_json::to_vec(report)?),
            summary_sha256: Sha256Digest::of(report.summary.as_bytes()),
            summary_bytes: report.summary.len() as u64,
            human_action_sha256: report
                .human_action
                .as_ref()
                .map(|text| Sha256Digest::of(text.as_bytes())),
            human_action_bytes: report
                .human_action
                .as_ref()
                .map_or(0, |text| text.len() as u64),
            needs_attention: report.needs_attention(),
        };
        let (_, sequence) = append(
            &transaction,
            "workflow_worker_reported",
            format!("workflow_worker_report:{}", uuid_key(request_id)),
            serde_json::to_value(&record)?,
            timestamp,
        )?;
        transaction.execute(
            "INSERT INTO workflow_worker_reports(request_id,workflow_id,record_json,created_sequence) VALUES (?,?,?,?)",
            params![uuid_key(request_id), uuid_key(workflow.workflow_id), serde_json::to_string(&record)?, sequence])?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn workflow_worker_reports(
        &self,
        workflow_id: Uuid,
        after: u64,
    ) -> Result<WorkerReportPage, StoreError> {
        let after_sql = i64::try_from(after)
            .map_err(|_| workflow_error("workflow_worker_report_cursor_invalid"))?;
        if workflow_rows::load_workflow(&self.connection, workflow_id)?.is_none() {
            return Err(workflow_error("store_workflow_not_found"));
        }
        let mut query = self.connection.prepare(
            "SELECT created_sequence,record_json,request_id FROM workflow_worker_reports WHERE workflow_id=? AND created_sequence>? ORDER BY created_sequence LIMIT ?")?;
        let rows = query.query_map(
            params![
                uuid_key(workflow_id),
                after_sql,
                (MAX_WORKER_REPORT_PAGE + 1) as i64
            ],
            |row| {
                Ok((
                    from_sql_u64(row.get::<_, i64>(0)?)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        let mut items = Vec::new();
        for row in rows {
            let (sequence, json, request_id) = row?;
            let record: WorkerReportRecord = serde_json::from_str(&json)?;
            if uuid_key(record.request_id) != request_id || record.workflow_id != workflow_id {
                return Err(workflow_error("workflow_worker_report_projection_mismatch"));
            }
            verify_record(&self.connection, &record, sequence)?;
            items.push((sequence, record));
        }
        let more = items.len() > MAX_WORKER_REPORT_PAGE;
        items.truncate(MAX_WORKER_REPORT_PAGE);
        let next = items.last().map_or(after, |(sequence, _)| *sequence);
        Ok(WorkerReportPage { items, next, more })
    }
}
