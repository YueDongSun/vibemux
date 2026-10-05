//! Indexed supervisor rows, projection cross-checks and bounded recovery reads.
use super::{MAX_STATUS_ITEMS, workflow_error};
use crate::StoreError;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;
use vibemux_workflow::supervisor::{
    SupervisorAction, SupervisorActionState, SupervisorExecution, SupervisorExecutionState,
    SupervisorMode, SupervisorRound, SupervisorRoundState,
};

pub(super) fn execution_state(state: SupervisorExecutionState) -> &'static str {
    match state {
        SupervisorExecutionState::Running => "running",
        SupervisorExecutionState::Stopped => "stopped",
        SupervisorExecutionState::RecoveryRequired => "recovery_required",
    }
}

pub(super) fn action_state(state: SupervisorActionState) -> &'static str {
    match state {
        SupervisorActionState::Pending => "pending",
        SupervisorActionState::Claimed => "claimed",
        SupervisorActionState::Applied => "applied",
        SupervisorActionState::Blocked => "blocked",
        SupervisorActionState::Unknown => "unknown",
    }
}

pub(super) fn load_execution(
    connection: &Connection,
    workflow_id: Uuid,
) -> Result<Option<SupervisorExecution>, StoreError> {
    let row: Option<(u64, String, String)> = connection
        .query_row(
            "SELECT generation,state,record_json FROM workflow_supervisors WHERE workflow_id=?",
            [workflow_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    row.map(|(generation, state, json)| {
        let record: SupervisorExecution = serde_json::from_str(&json)?;
        if record.workflow_id != workflow_id
            || record.generation != generation
            || execution_state(record.state) != state
        {
            return Err(workflow_error("store_supervisor_projection_mismatch"));
        }
        Ok(record)
    })
    .transpose()
}

pub(super) fn save_execution(
    transaction: &Transaction<'_>,
    record: &SupervisorExecution,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute("INSERT INTO workflow_supervisors(workflow_id,generation,state,record_json,updated_sequence) VALUES (?,?,?,?,?) ON CONFLICT(workflow_id) DO UPDATE SET generation=excluded.generation,state=excluded.state,record_json=excluded.record_json,updated_sequence=excluded.updated_sequence",
        params![record.workflow_id.to_string(), record.generation, execution_state(record.state), serde_json::to_string(record)?, sequence])?;
    Ok(())
}

pub(super) fn load_round(
    connection: &Connection,
    round_id: Uuid,
) -> Result<SupervisorRound, StoreError> {
    let row: Option<(String, u64, String, String)> = connection.query_row(
        "SELECT workflow_id,generation,call_id,record_json FROM workflow_supervisor_rounds WHERE round_id=?",
        [round_id.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    let (workflow_id, generation, call_id, json) =
        row.ok_or_else(|| workflow_error("store_supervisor_round_unknown"))?;
    let record: SupervisorRound = serde_json::from_str(&json)?;
    if record.round_id != round_id
        || record.workflow_id.to_string() != workflow_id
        || record.generation != generation
        || record.call_id.to_string() != call_id
    {
        return Err(workflow_error("store_supervisor_projection_mismatch"));
    }
    Ok(record)
}

pub(super) fn save_round(
    transaction: &Transaction<'_>,
    record: &SupervisorRound,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute("INSERT INTO workflow_supervisor_rounds(round_id,workflow_id,generation,call_id,record_json,updated_sequence) VALUES (?,?,?,?,?,?) ON CONFLICT(round_id) DO UPDATE SET record_json=excluded.record_json,updated_sequence=excluded.updated_sequence",
        params![record.round_id.to_string(),record.workflow_id.to_string(),record.generation,record.call_id.to_string(),serde_json::to_string(record)?,sequence])?;
    Ok(())
}

struct ActionRow {
    workflow_id: String,
    generation: u64,
    round_id: String,
    idempotency_key: String,
    state: String,
    fingerprint: String,
    dispatch_request_id: Option<String>,
    record_json: String,
}

pub(super) fn load_action(
    connection: &Connection,
    action_id: Uuid,
) -> Result<SupervisorAction, StoreError> {
    let row = connection.query_row(
        "SELECT workflow_id,generation,round_id,idempotency_key,state,fingerprint,dispatch_request_id,record_json FROM workflow_supervisor_actions WHERE action_id=?",
        [action_id.to_string()], |row| Ok(ActionRow {
            workflow_id:row.get(0)?,generation:row.get(1)?,round_id:row.get(2)?,idempotency_key:row.get(3)?,
            state:row.get(4)?,fingerprint:row.get(5)?,dispatch_request_id:row.get(6)?,record_json:row.get(7)?,
        })).optional()?.ok_or_else(|| workflow_error("store_supervisor_action_unknown"))?;
    let record: SupervisorAction = serde_json::from_str(&row.record_json)?;
    if record.action_id != action_id
        || record.workflow_id.to_string() != row.workflow_id
        || record.generation != row.generation
        || record.round_id.to_string() != row.round_id
        || record.idempotency_key != row.idempotency_key
        || action_state(record.state) != row.state
        || record.fingerprint.to_hex() != row.fingerprint
        || record.dispatch_request_id.map(|id| id.to_string()) != row.dispatch_request_id
    {
        return Err(workflow_error("store_supervisor_projection_mismatch"));
    }
    Ok(record)
}

pub(super) fn save_action(
    transaction: &Transaction<'_>,
    record: &SupervisorAction,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute("UPDATE workflow_supervisor_actions SET state=?,dispatch_request_id=?,record_json=?,updated_sequence=? WHERE action_id=?",
        params![action_state(record.state),record.dispatch_request_id.map(|id| id.to_string()),serde_json::to_string(record)?,sequence,record.action_id.to_string()])?;
    Ok(())
}

pub(super) fn actions_for(
    connection: &Connection,
    workflow_id: Uuid,
) -> Result<Vec<SupervisorAction>, StoreError> {
    let mut query = connection.prepare("SELECT action_id FROM workflow_supervisor_actions WHERE workflow_id=? ORDER BY rowid LIMIT ?")?;
    let ids = query
        .query_map(
            params![workflow_id.to_string(), MAX_STATUS_ITEMS as u32 + 1],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if ids.len() > MAX_STATUS_ITEMS {
        return Err(workflow_error("store_supervisor_status_too_large"));
    }
    ids.into_iter()
        .map(|id| {
            load_action(
                connection,
                Uuid::parse_str(&id)
                    .map_err(|_| workflow_error("store_supervisor_projection_mismatch"))?,
            )
        })
        .collect()
}

pub(super) fn active_execution(
    connection: &Connection,
    workflow_id: Uuid,
    generation: u64,
) -> Result<SupervisorExecution, StoreError> {
    let execution = load_execution(connection, workflow_id)?
        .ok_or_else(|| workflow_error("store_supervisor_unknown"))?;
    if execution.generation != generation
        || execution.state != SupervisorExecutionState::Running
        || execution.mode != SupervisorMode::Model
    {
        return Err(workflow_error("store_supervisor_stale_generation"));
    }
    Ok(execution)
}

pub(super) fn unresolved_round(
    connection: &Connection,
    workflow_id: Uuid,
) -> Result<bool, StoreError> {
    let mut query = connection.prepare(
        "SELECT round_id FROM workflow_supervisor_rounds WHERE workflow_id=? ORDER BY rowid",
    )?;
    let ids = query.query_map([workflow_id.to_string()], |row| row.get::<_, String>(0))?;
    for id in ids {
        let round = load_round(
            connection,
            Uuid::parse_str(&id?)
                .map_err(|_| workflow_error("store_supervisor_projection_mismatch"))?,
        )?;
        if matches!(
            round.state,
            SupervisorRoundState::Unknown | SupervisorRoundState::Reserved
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}
