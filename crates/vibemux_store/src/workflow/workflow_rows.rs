//! Row persistence for the schema-5 workflow tables. Every load cross-checks
//! the indexed columns against the record JSON; any disagreement fails
//! closed with `store_workflow_projection_mismatch`.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest, contract::ContractArtifact, receipts::WorkflowReceipt, slots::SlotFacts,
    workflow_record::WorkflowRecord,
};

use super::{
    ContentEntry, LeaseRecord, MessageRecord, PolicyVersionRecord, WorkflowAttemptRecord,
    workflow_error,
};
use crate::StoreError;

fn mismatch() -> StoreError {
    workflow_error("store_workflow_projection_mismatch")
}

fn decode<T: DeserializeOwned>(json: &str) -> Result<T, StoreError> {
    serde_json::from_str(json).map_err(|_| mismatch())
}

fn encode(value: &impl Serialize) -> Result<String, StoreError> {
    Ok(serde_json::to_string(value)?)
}

pub(super) fn uuid_key(value: Uuid) -> String {
    value.as_hyphenated().to_string()
}

// --- workflows -----------------------------------------------------------

pub(super) struct StoredWorkflow {
    pub(super) record: WorkflowRecord,
    pub(super) sequence: u64,
}

fn load_workflow_where(
    connection: &Connection,
    column: &str,
    value: &str,
) -> Result<Option<StoredWorkflow>, StoreError> {
    let row = connection
        .query_row(
            &format!("SELECT workflow_id, request_key, phase, version, record_json, updated_sequence FROM workflows WHERE {column} = ?"),
            [value],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u64>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((workflow_id, request_key, phase, version, json, sequence)) = row else {
        return Ok(None);
    };
    let record: WorkflowRecord = decode(&json)?;
    if uuid_key(record.workflow_id) != workflow_id
        || record.request_key != request_key
        || record.phase.as_str() != phase
        || record.version != version
        || record.validate().is_err()
    {
        return Err(mismatch());
    }
    Ok(Some(StoredWorkflow { record, sequence }))
}

pub(super) fn load_workflow(
    connection: &Connection,
    workflow_id: Uuid,
) -> Result<Option<StoredWorkflow>, StoreError> {
    load_workflow_where(connection, "workflow_id", &uuid_key(workflow_id))
}

pub(super) fn load_workflow_by_request(
    connection: &Connection,
    request_key: &str,
) -> Result<Option<StoredWorkflow>, StoreError> {
    load_workflow_where(connection, "request_key", request_key)
}

pub(super) fn load_existing_workflow(
    connection: &Connection,
    workflow_id: Uuid,
) -> Result<StoredWorkflow, StoreError> {
    load_workflow(connection, workflow_id)?
        .ok_or_else(|| workflow_error("store_workflow_not_found"))
}

pub(super) fn workflow_fingerprint(
    connection: &Connection,
    workflow_id: Uuid,
) -> Result<String, StoreError> {
    Ok(connection.query_row(
        "SELECT fingerprint FROM workflows WHERE workflow_id = ?",
        [uuid_key(workflow_id)],
        |row| row.get(0),
    )?)
}

pub(super) fn insert_workflow(
    transaction: &Transaction<'_>,
    record: &WorkflowRecord,
    fingerprint: Sha256Digest,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO workflows(workflow_id, request_key, fingerprint, phase, version, record_json, updated_sequence) VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![
            uuid_key(record.workflow_id),
            record.request_key,
            fingerprint.to_hex(),
            record.phase.as_str(),
            record.version,
            encode(record)?,
            sequence,
        ],
    )?;
    Ok(())
}

/// Compare-and-set on the version the transaction loaded.
pub(super) fn update_workflow(
    transaction: &Transaction<'_>,
    record: &WorkflowRecord,
    previous_version: u64,
    sequence: i64,
) -> Result<(), StoreError> {
    record
        .validate()
        .map_err(|error| workflow_error(error.code()))?;
    let changed = transaction.execute(
        "UPDATE workflows SET phase = ?, version = ?, record_json = ?, updated_sequence = ? WHERE workflow_id = ? AND version = ?",
        params![
            record.phase.as_str(),
            record.version,
            encode(record)?,
            sequence,
            uuid_key(record.workflow_id),
            previous_version,
        ],
    )?;
    if changed != 1 {
        return Err(workflow_error("store_workflow_version_conflict"));
    }
    Ok(())
}

pub(super) fn list_workflows(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<WorkflowRecord>, StoreError> {
    let mut statement =
        connection.prepare("SELECT record_json FROM workflows ORDER BY rowid DESC LIMIT ?")?;
    let rows = statement
        .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.iter().map(|json| decode(json)).collect()
}

pub(super) fn list_workflows_after(
    connection: &Connection,
    after: Option<Uuid>,
    limit: usize,
) -> Result<Vec<WorkflowRecord>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT workflow_id FROM workflows WHERE workflow_id > ? ORDER BY workflow_id ASC LIMIT ?",
    )?;
    let keys = statement
        .query_map(
            params![
                after.map(uuid_key).unwrap_or_default(),
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    keys.into_iter()
        .map(|key| {
            let id = Uuid::parse_str(&key).map_err(|_| mismatch())?;
            load_workflow(connection, id)?
                .map(|stored| stored.record)
                .ok_or_else(mismatch)
        })
        .collect()
}

// --- contracts -----------------------------------------------------------

pub(super) fn load_contract(
    connection: &Connection,
    contract_id: Sha256Digest,
) -> Result<Option<ContractArtifact>, StoreError> {
    let json: Option<String> = connection
        .query_row(
            "SELECT record_json FROM workflow_contracts WHERE contract_id = ?",
            [contract_id.to_hex()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(json) = json else {
        return Ok(None);
    };
    let artifact: ContractArtifact = decode(&json)?;
    if artifact.contract_id != contract_id {
        return Err(mismatch());
    }
    Ok(Some(artifact))
}

/// Content-addressed: an identical artifact is reused; a different record
/// under the same identity is an integrity failure.
pub(super) fn insert_contract_if_absent(
    transaction: &Transaction<'_>,
    artifact: &ContractArtifact,
    sequence: i64,
) -> Result<bool, StoreError> {
    if let Some(existing) = load_contract(transaction, artifact.contract_id)? {
        if existing != *artifact {
            return Err(workflow_error("store_workflow_contract_conflict"));
        }
        return Ok(false);
    }
    transaction.execute(
        "INSERT INTO workflow_contracts(contract_id, record_json, created_sequence) VALUES (?, ?, ?)",
        params![artifact.contract_id.to_hex(), encode(artifact)?, sequence],
    )?;
    Ok(true)
}

// --- slots ---------------------------------------------------------------

pub(super) fn load_slot(
    connection: &Connection,
    slot_id: &str,
) -> Result<Option<(SlotFacts, u64)>, StoreError> {
    let row = connection
        .query_row(
            "SELECT record_json, last_generation FROM workflow_slots WHERE slot_id = ?",
            [slot_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
        )
        .optional()?;
    let Some((json, generation)) = row else {
        return Ok(None);
    };
    let slot: SlotFacts = decode(&json)?;
    if slot.slot_id.as_str() != slot_id {
        return Err(mismatch());
    }
    Ok(Some((slot, generation)))
}

pub(super) fn upsert_slot(
    transaction: &Transaction<'_>,
    slot: &SlotFacts,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        r#"
        INSERT INTO workflow_slots(slot_id, record_json, last_generation, updated_sequence) VALUES (?, ?, 0, ?)
        ON CONFLICT(slot_id) DO UPDATE SET record_json = excluded.record_json, updated_sequence = excluded.updated_sequence
        "#,
        params![slot.slot_id.as_str(), encode(slot)?, sequence],
    )?;
    Ok(())
}

pub(super) fn bump_slot_generation(
    transaction: &Transaction<'_>,
    slot_id: &str,
    previous: u64,
    next: u64,
) -> Result<(), StoreError> {
    let changed = transaction.execute(
        "UPDATE workflow_slots SET last_generation = ? WHERE slot_id = ? AND last_generation = ?",
        params![next, slot_id, previous],
    )?;
    if changed != 1 {
        return Err(workflow_error("store_workflow_version_conflict"));
    }
    Ok(())
}

pub(super) fn all_slots(connection: &Connection) -> Result<Vec<SlotFacts>, StoreError> {
    let mut statement =
        connection.prepare("SELECT slot_id FROM workflow_slots ORDER BY slot_id")?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut slots = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some((slot, _)) = load_slot(connection, &id)? {
            slots.push(slot);
        }
    }
    Ok(slots)
}

// --- leases --------------------------------------------------------------

const LEASE_COLUMNS: &str =
    "lease_id, workflow_id, slot_id, worktree_key, generation, state, record_json";

fn lease_from_row(
    row: (String, String, String, String, u64, String, String),
) -> Result<LeaseRecord, StoreError> {
    let (lease_id, workflow_id, slot_id, worktree_key, generation, state, json) = row;
    let lease: LeaseRecord = decode(&json)?;
    if uuid_key(lease.lease_id) != lease_id
        || uuid_key(lease.workflow_id) != workflow_id
        || lease.slot_id.as_str() != slot_id
        || lease.worktree_key.to_hex() != worktree_key
        || lease.generation != generation
        || lease.state.as_str() != state
    {
        return Err(mismatch());
    }
    Ok(lease)
}

fn query_leases(
    connection: &Connection,
    condition: &str,
    values: impl rusqlite::Params,
) -> Result<Vec<LeaseRecord>, StoreError> {
    let mut statement = connection.prepare(&format!(
        "SELECT {LEASE_COLUMNS} FROM workflow_leases WHERE {condition} ORDER BY rowid"
    ))?;
    let rows = statement
        .query_map(values, |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(lease_from_row).collect()
}

pub(super) fn load_lease(
    connection: &Connection,
    lease_id: Uuid,
) -> Result<Option<LeaseRecord>, StoreError> {
    Ok(
        query_leases(connection, "lease_id = ?", [uuid_key(lease_id)])?
            .into_iter()
            .next(),
    )
}

pub(super) fn leases_for(
    connection: &Connection,
    workflow_id: Uuid,
) -> Result<Vec<LeaseRecord>, StoreError> {
    query_leases(connection, "workflow_id = ?", [uuid_key(workflow_id)])
}

/// Leases that still hold their reservation, in acquisition order.
pub(super) fn holding_leases(connection: &Connection) -> Result<Vec<LeaseRecord>, StoreError> {
    query_leases(connection, "state IN ('active', 'quarantined')", ())
}

/// The lease holding `slot_id` or `worktree_key`, if any.
pub(super) fn holder(
    connection: &Connection,
    column: &str,
    value: &str,
) -> Result<Option<LeaseRecord>, StoreError> {
    Ok(query_leases(
        connection,
        &format!("{column} = ? AND state IN ('active', 'quarantined')"),
        [value],
    )?
    .into_iter()
    .next())
}

pub(super) fn insert_lease(
    transaction: &Transaction<'_>,
    lease: &LeaseRecord,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO workflow_leases(lease_id, workflow_id, task_key, slot_id, worktree_key, generation, state, record_json, updated_sequence) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            uuid_key(lease.lease_id),
            uuid_key(lease.workflow_id),
            lease.task_key.as_str(),
            lease.slot_id.as_str(),
            lease.worktree_key.to_hex(),
            lease.generation,
            lease.state.as_str(),
            encode(lease)?,
            sequence,
        ],
    )?;
    Ok(())
}

pub(super) fn update_lease(
    transaction: &Transaction<'_>,
    lease: &LeaseRecord,
    previous_state: &str,
    sequence: i64,
) -> Result<(), StoreError> {
    let changed = transaction.execute(
        "UPDATE workflow_leases SET state = ?, record_json = ?, updated_sequence = ? WHERE lease_id = ? AND state = ? AND generation = ?",
        params![
            lease.state.as_str(),
            encode(lease)?,
            sequence,
            uuid_key(lease.lease_id),
            previous_state,
            lease.generation,
        ],
    )?;
    if changed != 1 {
        return Err(workflow_error("store_workflow_version_conflict"));
    }
    Ok(())
}

// --- attempts ------------------------------------------------------------

fn query_attempts(
    connection: &Connection,
    condition: &str,
    value: &str,
) -> Result<Vec<WorkflowAttemptRecord>, StoreError> {
    let mut statement = connection.prepare(&format!(
        "SELECT request_id, workflow_id, lease_id, record_json FROM workflow_attempts WHERE {condition} ORDER BY rowid"
    ))?;
    let rows = statement
        .query_map([value], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(request_id, workflow_id, lease_id, json)| {
            let attempt: WorkflowAttemptRecord = decode(&json)?;
            if uuid_key(attempt.request_id) != request_id
                || uuid_key(attempt.workflow_id) != workflow_id
                || uuid_key(attempt.lease_id) != lease_id
            {
                return Err(mismatch());
            }
            Ok(attempt)
        })
        .collect()
}

pub(super) fn load_attempt(
    connection: &Connection,
    request_id: Uuid,
) -> Result<Option<WorkflowAttemptRecord>, StoreError> {
    Ok(
        query_attempts(connection, "request_id = ?", &uuid_key(request_id))?
            .into_iter()
            .next(),
    )
}

pub(super) fn attempts_for(
    connection: &Connection,
    workflow_id: Uuid,
) -> Result<Vec<WorkflowAttemptRecord>, StoreError> {
    query_attempts(connection, "workflow_id = ?", &uuid_key(workflow_id))
}

pub(super) fn attempts_for_lease(
    connection: &Connection,
    lease_id: Uuid,
) -> Result<Vec<WorkflowAttemptRecord>, StoreError> {
    query_attempts(connection, "lease_id = ?", &uuid_key(lease_id))
}

pub(super) fn insert_attempt(
    transaction: &Transaction<'_>,
    attempt: &WorkflowAttemptRecord,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO workflow_attempts(request_id, workflow_id, lease_id, lease_generation, task_key, contract_id, purpose, record_json, created_sequence) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            uuid_key(attempt.request_id),
            uuid_key(attempt.workflow_id),
            uuid_key(attempt.lease_id),
            attempt.lease_generation,
            attempt.task_key.as_str(),
            attempt.contract_id.to_hex(),
            attempt.purpose.as_str(),
            encode(attempt)?,
            sequence,
        ],
    )?;
    Ok(())
}

/// The dispatch phase of an attempt, from the dispatch table.
pub(super) fn dispatch_phase(
    connection: &Connection,
    request_id: Uuid,
) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT phase FROM harness_dispatches WHERE request_id = ?",
            [uuid_key(request_id)],
            |row| row.get(0),
        )
        .optional()?)
}

/// The Run ID the dispatch record of `request_id` owns.
pub(super) fn dispatch_run_id(
    connection: &Connection,
    request_id: Uuid,
) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT run_id FROM harness_dispatches WHERE request_id = ?",
            [uuid_key(request_id)],
            |row| row.get(0),
        )
        .optional()?)
}

// --- receipts ------------------------------------------------------------

pub(super) fn load_receipt(
    connection: &Connection,
    receipt_id: Uuid,
) -> Result<Option<(WorkflowReceipt, String)>, StoreError> {
    let row = connection
        .query_row(
            "SELECT record_json, receipt_digest FROM workflow_receipts WHERE receipt_id = ?",
            [uuid_key(receipt_id)],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    row.map(|(json, digest)| Ok((decode(&json)?, digest)))
        .transpose()
}

pub(super) fn receipts_for(
    connection: &Connection,
    workflow_id: Uuid,
    kind: Option<&str>,
) -> Result<Vec<WorkflowReceipt>, StoreError> {
    let workflow = uuid_key(workflow_id);
    let mut statement = connection.prepare(
        "SELECT kind, record_json FROM workflow_receipts WHERE workflow_id = ?1 AND (?2 IS NULL OR kind = ?2) ORDER BY created_sequence, rowid",
    )?;
    let rows = statement
        .query_map(params![workflow, kind], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(kind, json)| {
            let receipt: WorkflowReceipt = decode(&json)?;
            if receipt.kind() != kind {
                return Err(mismatch());
            }
            Ok(receipt)
        })
        .collect()
}

pub(super) fn insert_receipt(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
    receipt: &WorkflowReceipt,
    digest: Sha256Digest,
    sequence: i64,
) -> Result<(), StoreError> {
    let subject = receipt.subject_digest().map_err(|_| mismatch())?;
    transaction.execute(
        "INSERT INTO workflow_receipts(receipt_id, workflow_id, kind, task_key, subject_digest, receipt_digest, record_json, created_sequence) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            uuid_key(receipt.receipt_id()),
            uuid_key(workflow_id),
            receipt.kind(),
            receipt.task_key().map(|key| key.as_str().to_string()),
            subject.to_hex(),
            digest.to_hex(),
            encode(receipt)?,
            sequence,
        ],
    )?;
    Ok(())
}

// --- messages ------------------------------------------------------------

pub(super) fn load_message(
    connection: &Connection,
    message_id: Uuid,
) -> Result<Option<MessageRecord>, StoreError> {
    let row = connection
        .query_row(
            "SELECT state, recipient_sequence, record_json FROM workflow_messages WHERE message_id = ?",
            [uuid_key(message_id)],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?, row.get::<_, String>(2)?)),
        )
        .optional()?;
    let Some((state, sequence, json)) = row else {
        return Ok(None);
    };
    let message: MessageRecord = decode(&json)?;
    if message.state.as_str() != state
        || message.recipient_sequence != sequence
        || message.envelope.message_id != message_id
    {
        return Err(mismatch());
    }
    Ok(Some(message))
}

pub(super) fn messages_for(
    connection: &Connection,
    workflow_id: Uuid,
) -> Result<Vec<MessageRecord>, StoreError> {
    let mut statement = connection
        .prepare("SELECT message_id FROM workflow_messages WHERE workflow_id = ? ORDER BY rowid")?;
    let ids = statement
        .query_map([uuid_key(workflow_id)], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut messages = Vec::with_capacity(ids.len());
    for id in ids {
        let id = Uuid::parse_str(&id).map_err(|_| mismatch())?;
        messages.push(load_message(connection, id)?.ok_or_else(mismatch)?);
    }
    Ok(messages)
}

pub(super) fn next_recipient_sequence(
    connection: &Connection,
    workflow_id: Uuid,
    recipient: &str,
) -> Result<u64, StoreError> {
    let current: u64 = connection.query_row(
        "SELECT COALESCE(MAX(recipient_sequence), 0) FROM workflow_messages WHERE workflow_id = ? AND recipient = ?",
        params![uuid_key(workflow_id), recipient],
        |row| row.get(0),
    )?;
    current.checked_add(1).ok_or_else(mismatch)
}

pub(super) fn insert_message(
    transaction: &Transaction<'_>,
    message: &MessageRecord,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO workflow_messages(message_id, workflow_id, recipient, recipient_sequence, state, fingerprint, record_json, updated_sequence) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            uuid_key(message.envelope.message_id),
            uuid_key(message.envelope.workflow_id),
            message.recipient_label,
            message.recipient_sequence,
            message.state.as_str(),
            message.fingerprint.to_hex(),
            encode(message)?,
            sequence,
        ],
    )?;
    Ok(())
}

pub(super) fn update_message(
    transaction: &Transaction<'_>,
    message: &MessageRecord,
    previous_state: &str,
    sequence: i64,
) -> Result<(), StoreError> {
    let changed = transaction.execute(
        "UPDATE workflow_messages SET state = ?, record_json = ?, updated_sequence = ? WHERE message_id = ? AND state = ?",
        params![
            message.state.as_str(),
            encode(message)?,
            sequence,
            uuid_key(message.envelope.message_id),
            previous_state,
        ],
    )?;
    if changed != 1 {
        return Err(workflow_error("store_workflow_version_conflict"));
    }
    Ok(())
}

// --- content store index -------------------------------------------------

pub(super) fn load_content(
    connection: &Connection,
    sha256: Sha256Digest,
) -> Result<Option<ContentEntry>, StoreError> {
    let row = connection
        .query_row(
            "SELECT state, record_json FROM content_store_entries WHERE content_sha256 = ?",
            [sha256.to_hex()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((state, json)) = row else {
        return Ok(None);
    };
    let entry: ContentEntry = decode(&json)?;
    if entry.content_sha256 != sha256 || entry.state.as_str() != state {
        return Err(mismatch());
    }
    Ok(Some(entry))
}

pub(super) fn upsert_content(
    transaction: &Transaction<'_>,
    entry: &ContentEntry,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        r#"
        INSERT INTO content_store_entries(content_sha256, workflow_id, kind, byte_count, state, record_json, updated_sequence) VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(content_sha256) DO UPDATE SET state = excluded.state, record_json = excluded.record_json, updated_sequence = excluded.updated_sequence
        "#,
        params![
            entry.content_sha256.to_hex(),
            entry.workflow_id.map(uuid_key),
            entry.kind.as_str(),
            entry.byte_count,
            entry.state.as_str(),
            encode(entry)?,
            sequence,
        ],
    )?;
    Ok(())
}

// --- prompt policy versions ----------------------------------------------

pub(super) fn policy_versions(
    connection: &Connection,
) -> Result<Vec<PolicyVersionRecord>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT version, state, record_json FROM prompt_policy_versions ORDER BY version",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(version, state, json)| {
            let record: PolicyVersionRecord = decode(&json)?;
            if record.version.version != version || policy_state(record.version.state) != state {
                return Err(mismatch());
            }
            Ok(record)
        })
        .collect()
}

pub(super) fn policy_state(state: vibemux_workflow::optimizer::PolicyState) -> &'static str {
    use vibemux_workflow::optimizer::PolicyState;
    match state {
        PolicyState::Active => "active",
        PolicyState::Superseded => "superseded",
        PolicyState::RolledBack => "rolled_back",
    }
}

pub(super) fn upsert_policy_version(
    transaction: &Transaction<'_>,
    record: &PolicyVersionRecord,
    sequence: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        r#"
        INSERT INTO prompt_policy_versions(version, parent, policy_digest, state, record_json, updated_sequence) VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(version) DO UPDATE SET state = excluded.state, record_json = excluded.record_json, updated_sequence = excluded.updated_sequence
        "#,
        params![
            record.version.version,
            record.version.parent,
            record.version.policy_digest.to_hex(),
            policy_state(record.version.state),
            encode(record)?,
            sequence,
        ],
    )?;
    Ok(())
}
