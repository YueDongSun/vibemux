//! Durable supervised workflows (ADR 031): store schema 5 and the writer
//! transactions that apply the pure `vibemux_workflow` rules.
//!
//! A workflow references its admitted contracts by identity and its child
//! attempts by dispatch request ID. It never rebinds a dispatch-owned or
//! A2A-owned Task/Run: each worker turn is admitted through the same
//! dispatch admission (same gates, record, projections, and event) inside
//! the transaction that checks the workflow phase and the lease fence and
//! records the typed association. Every state change appends its canonical
//! event in the same immediate transaction. Payloads are content-free:
//! identifiers, digests, counts, and fixed codes only.

mod workflow_rows;

use rusqlite::{Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_events::{ActorName, EventDraft, EventEnvelope, EventPayload, EventType};
use vibemux_harness::dispatch::DispatchPhase;
use vibemux_types::{EventId, ProjectId};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    contract::ContractArtifact,
    gates::{
        AcceptanceLevel, CandidateAcceptance, GateRequirements, IntegrationReceipt, ReviewReceipt,
        VerifierReceipt, WorkflowPhase, candidate_gate, cooperative_acceptance,
    },
    leases::{LeaseEvent, LeaseState, LeaseView, apply as apply_lease},
    messages::{
        DeliveryEvent, MessageEnvelope, MessageState, Participant, transition as message_transition,
    },
    optimizer::{OptimizablePolicy, PolicyHistory, PolicyVersion},
    receipts::{CandidateReceipt, WorkflowReceipt},
    slots::SlotFacts,
    task_spec::WorkflowMode,
    workflow_record::{TaskProgress, WorkflowRecord},
};

use crate::{
    HarnessDispatchAdmission, HarnessDispatchCommit, SqliteStore, StoreError,
    harness_dispatch::admit_dispatch_in, resolve_or_mint_project_id,
};

use workflow_rows::uuid_key;

pub const WORKFLOW_STORE_RECORD_SCHEMA_VERSION: u32 = 1;
const EVENT_ACTOR: &str = "vibemuxd";
/// Bound on records returned by one snapshot read.
pub const MAX_SNAPSHOT_ITEMS: usize = 512;

/// Adds the workflow tables. `migrate` runs it and the version marker in one
/// immediate transaction, so a failure leaves the database at schema 4.
/// The partial unique indexes are the slot and worktree reservations: they
/// cover exactly the lease states that hold a reservation.
pub(crate) const MIGRATION_V5: &str = r#"
CREATE TABLE workflows (
    workflow_id TEXT PRIMARY KEY,
    request_key TEXT NOT NULL UNIQUE,
    fingerprint TEXT NOT NULL,
    phase TEXT NOT NULL CHECK(phase IN ('prepared', 'running', 'paused', 'integrating',
        'accepted', 'failed', 'cancel_requested', 'cancelled', 'blocked')),
    version INTEGER NOT NULL CHECK(version > 0),
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE TABLE workflow_contracts (
    contract_id TEXT PRIMARY KEY,
    record_json TEXT NOT NULL,
    created_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE TABLE workflow_slots (
    slot_id TEXT PRIMARY KEY,
    record_json TEXT NOT NULL,
    last_generation INTEGER NOT NULL CHECK(last_generation >= 0),
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE TABLE workflow_leases (
    lease_id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id),
    task_key TEXT NOT NULL,
    slot_id TEXT NOT NULL REFERENCES workflow_slots(slot_id),
    worktree_key TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(generation > 0),
    state TEXT NOT NULL CHECK(state IN ('active', 'quarantined', 'released', 'revoked')),
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
    UNIQUE(slot_id, generation)
);
CREATE UNIQUE INDEX workflow_leases_holding_slot ON workflow_leases(slot_id)
    WHERE state IN ('active', 'quarantined');
CREATE UNIQUE INDEX workflow_leases_holding_worktree ON workflow_leases(worktree_key)
    WHERE state IN ('active', 'quarantined');
CREATE TABLE workflow_attempts (
    request_id TEXT PRIMARY KEY REFERENCES harness_dispatches(request_id),
    workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id),
    lease_id TEXT NOT NULL REFERENCES workflow_leases(lease_id),
    lease_generation INTEGER NOT NULL CHECK(lease_generation > 0),
    task_key TEXT NOT NULL,
    contract_id TEXT NOT NULL REFERENCES workflow_contracts(contract_id),
    purpose TEXT NOT NULL CHECK(purpose IN ('implement', 'repair', 'answer', 'review')),
    record_json TEXT NOT NULL,
    created_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE TABLE workflow_receipts (
    receipt_id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id),
    kind TEXT NOT NULL CHECK(kind IN ('candidate', 'review', 'verification', 'integration',
        'selection', 'decision', 'model_call', 'bundle')),
    task_key TEXT,
    subject_digest TEXT NOT NULL,
    receipt_digest TEXT NOT NULL,
    record_json TEXT NOT NULL,
    created_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE INDEX workflow_receipts_by_workflow ON workflow_receipts(workflow_id, kind, created_sequence);
CREATE TABLE workflow_messages (
    message_id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id),
    recipient TEXT NOT NULL,
    recipient_sequence INTEGER NOT NULL CHECK(recipient_sequence > 0),
    state TEXT NOT NULL CHECK(state IN ('admitted', 'delivered', 'acknowledged', 'rejected', 'expired')),
    fingerprint TEXT NOT NULL,
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
    UNIQUE(workflow_id, recipient, recipient_sequence)
);
CREATE TABLE content_store_entries (
    content_sha256 TEXT PRIMARY KEY,
    workflow_id TEXT,
    kind TEXT NOT NULL,
    byte_count INTEGER NOT NULL CHECK(byte_count >= 0),
    state TEXT NOT NULL CHECK(state IN ('present', 'deleted')),
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE TABLE prompt_policy_versions (
    version INTEGER PRIMARY KEY CHECK(version > 0),
    parent INTEGER,
    policy_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('active', 'superseded', 'rolled_back')),
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE UNIQUE INDEX prompt_policy_versions_one_active ON prompt_policy_versions(state)
    WHERE state = 'active';
INSERT INTO schema_migrations(version, applied_at)
VALUES (5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
"#;

pub(crate) fn workflow_error(code: &'static str) -> StoreError {
    StoreError::Workflow(code)
}

/// A persisted lease: one slot and one worktree for one task under one
/// generation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRecord {
    pub schema_version: u32,
    pub lease_id: Uuid,
    pub workflow_id: Uuid,
    pub task_key: SpecIdentifier,
    pub slot_id: SpecIdentifier,
    pub worktree_key: Sha256Digest,
    pub generation: u64,
    pub state: LeaseState,
    pub heartbeat_deadline_ms: u64,
    pub end_reason: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl LeaseRecord {
    #[must_use]
    pub const fn view(&self) -> LeaseView {
        LeaseView {
            generation: self.generation,
            state: self.state,
            heartbeat_deadline_ms: self.heartbeat_deadline_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPurpose {
    Implement,
    Repair,
    /// A source task answering a brokered question from its own evidence.
    Answer,
    /// A fresh independent reviewer session.
    Review,
}

impl AttemptPurpose {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Implement => "implement",
            Self::Repair => "repair",
            Self::Answer => "answer",
            Self::Review => "review",
        }
    }
}

/// The typed association of one dispatch attempt with a workflow.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAttemptRecord {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub workflow_id: Uuid,
    pub lease_id: Uuid,
    pub lease_generation: u64,
    pub task_key: SpecIdentifier,
    pub contract_id: Sha256Digest,
    pub purpose: AttemptPurpose,
    pub session_id: Uuid,
    pub rendered_prompt_sha256: Sha256Digest,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug)]
pub struct WorkflowAttemptAdmission {
    pub workflow_id: Uuid,
    pub expected_workflow_version: u64,
    pub lease_id: Uuid,
    pub lease_generation: u64,
    pub task_key: SpecIdentifier,
    pub contract_id: Sha256Digest,
    pub purpose: AttemptPurpose,
    pub session_id: Uuid,
    pub dispatch: HarnessDispatchAdmission,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowAttemptCommit {
    pub attempt: WorkflowAttemptRecord,
    pub dispatch: HarnessDispatchCommit,
    pub workflow: WorkflowRecord,
    pub duplicate: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowCommit {
    pub record: WorkflowRecord,
    pub event: Option<EventEnvelope>,
    pub sequence: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LeaseCommit {
    pub lease: LeaseRecord,
    pub event: Option<EventEnvelope>,
}

#[derive(Clone, Debug)]
pub struct LeaseRequest {
    pub lease_id: Uuid,
    pub workflow_id: Uuid,
    pub task_key: SpecIdentifier,
    pub slot_id: SpecIdentifier,
    pub worktree_key: Sha256Digest,
    pub heartbeat_deadline_ms: u64,
    pub timestamp: OffsetDateTime,
}

/// The lease generation a completion is reported under.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseFence {
    pub lease_id: Uuid,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReceiptCommit {
    pub receipt_digest: Sha256Digest,
    pub event: Option<EventEnvelope>,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MessageRecord {
    pub schema_version: u32,
    pub envelope: MessageEnvelope,
    pub recipient_label: String,
    pub recipient_sequence: u64,
    pub state: MessageState,
    pub fingerprint: Sha256Digest,
    pub delivered_in_attempt: Option<Uuid>,
    pub acknowledged_in_attempt: Option<Uuid>,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MessageCommit {
    pub message: MessageRecord,
    pub event: Option<EventEnvelope>,
}

/// A delivery-state change of an admitted message.
#[derive(Clone, Debug)]
pub enum MessageChange {
    Deliver { attempt: Uuid },
    Acknowledge { by: Participant, attempt: Uuid },
    Expire,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Request,
    RenderedPrompt,
    Transcript,
    Bundle,
    MessageBody,
    CandidateFile,
    Report,
}

impl ContentKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::RenderedPrompt => "rendered_prompt",
            Self::Transcript => "transcript",
            Self::Bundle => "bundle",
            Self::MessageBody => "message_body",
            Self::CandidateFile => "candidate_file",
            Self::Report => "report",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentState {
    Present,
    Deleted,
}

impl ContentState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Deleted => "deleted",
        }
    }
}

/// Index entry for one blob in the opt-in private content store. The blob
/// lives in the daemon's content directory; the store holds only facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContentEntry {
    pub content_sha256: Sha256Digest,
    pub workflow_id: Option<Uuid>,
    pub kind: ContentKind,
    pub byte_count: u64,
    pub state: ContentState,
    pub retain_until_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyVersionRecord {
    pub version: PolicyVersion,
    pub policy: OptimizablePolicy,
}

/// Everything a reader needs about one workflow, bounded.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowSnapshot {
    pub record: WorkflowRecord,
    pub sequence: u64,
    pub contracts: Vec<ContractArtifact>,
    pub leases: Vec<LeaseRecord>,
    pub attempts: Vec<WorkflowAttemptRecord>,
    pub receipts: Vec<WorkflowReceipt>,
    pub messages: Vec<MessageRecord>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowRecovery {
    pub quarantined_leases: Vec<Uuid>,
    pub released_leases: Vec<Uuid>,
}

fn millis(timestamp: OffsetDateTime) -> u64 {
    u64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
}

fn workflow_draft(
    project_id: ProjectId,
    event_type: &str,
    idempotency_key: String,
    payload: Value,
    timestamp: OffsetDateTime,
) -> Result<EventDraft, StoreError> {
    Ok(EventDraft {
        event_id: EventId::new(),
        event_type: EventType::new(event_type)?,
        project_id,
        task_id: None,
        run_id: None,
        causation_id: None,
        actor: ActorName::new(EVENT_ACTOR)?,
        timestamp,
        idempotency_key: Some(idempotency_key),
        payload: EventPayload::new(payload)?,
    })
}

/// Appends an event and returns it with its raw sequence.
fn append(
    transaction: &Transaction<'_>,
    event_type: &str,
    idempotency_key: String,
    payload: Value,
    timestamp: OffsetDateTime,
) -> Result<(EventEnvelope, i64), StoreError> {
    let project_id = resolve_or_mint_project_id(transaction)?;
    let draft = workflow_draft(project_id, event_type, idempotency_key, payload, timestamp)?;
    SqliteStore::insert_event(transaction, draft)
}

fn dispatch_terminal(phase: &str) -> bool {
    matches!(phase, "completed" | "failed" | "unverified" | "cancelled")
}

impl SqliteStore {
    fn immediate(&mut self) -> Result<Transaction<'_>, StoreError> {
        Ok(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?)
    }

    /// Admits a prepared workflow and its contract artifacts. A repeated
    /// request key with the same admission fingerprint returns the stored
    /// workflow and writes nothing; different content is a conflict.
    pub fn prepare_workflow(
        &mut self,
        record: &WorkflowRecord,
        contracts: &[ContractArtifact],
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowCommit, StoreError> {
        record
            .validate()
            .map_err(|error| workflow_error(error.code()))?;
        if record.phase != WorkflowPhase::Prepared || record.version != 1 {
            return Err(workflow_error("store_workflow_invalid_admission"));
        }
        for task in &record.tasks {
            let matching: Vec<&ContractArtifact> = contracts
                .iter()
                .filter(|contract| contract.contract_id == task.contract_id)
                .collect();
            let valid = matching.len() == 1
                && matching[0].task_key == task.task_key.as_str()
                && matching[0].workflow_key == record.workflow_key.as_str()
                && matching[0].contract_version == task.contract_version
                && matching[0].base_commit == record.base_commit;
            if !valid {
                return Err(workflow_error("store_workflow_contract_mismatch"));
            }
        }
        if contracts.len() != record.tasks.len() {
            return Err(workflow_error("store_workflow_contract_mismatch"));
        }
        let fingerprint = record
            .admission_fingerprint()
            .map_err(|error| workflow_error(error.code()))?;
        let transaction = self.immediate()?;
        if let Some(existing) =
            workflow_rows::load_workflow_by_request(&transaction, &record.request_key)?
        {
            let stored =
                workflow_rows::workflow_fingerprint(&transaction, existing.record.workflow_id)?;
            if stored != fingerprint.to_hex() {
                return Err(workflow_error("store_workflow_request_conflict"));
            }
            return Ok(WorkflowCommit {
                record: existing.record,
                event: None,
                sequence: existing.sequence,
            });
        }
        let contract_ids: Vec<String> = record
            .tasks
            .iter()
            .map(|task| task.contract_id.to_hex())
            .collect();
        let (event, sequence) = append(
            &transaction,
            "workflow_prepared",
            format!("workflow_prepared:{}", uuid_key(record.workflow_id)),
            json!({
                "workflow_id": record.workflow_id,
                "mode": record.mode,
                "evidence_class": record.evidence_class,
                "task_count": record.tasks.len(),
                "contract_ids": contract_ids,
                "policy_digest": record.policy_digest,
                "verifier_digest": record.verifier_digest,
                "content_store": record.content_store,
            }),
            timestamp,
        )?;
        for contract in contracts {
            workflow_rows::insert_contract_if_absent(&transaction, contract, sequence)?;
        }
        workflow_rows::insert_workflow(&transaction, record, fingerprint, sequence)?;
        transaction.commit()?;
        Ok(WorkflowCommit {
            record: record.clone(),
            sequence: event.sequence().get(),
            event: Some(event),
        })
    }

    pub fn workflow_record(&self, workflow_id: Uuid) -> Result<Option<WorkflowRecord>, StoreError> {
        Ok(
            workflow_rows::load_workflow(&self.connection, workflow_id)?
                .map(|stored| stored.record),
        )
    }

    pub fn workflow_by_request_key(
        &self,
        request_key: &str,
    ) -> Result<Option<WorkflowRecord>, StoreError> {
        Ok(
            workflow_rows::load_workflow_by_request(&self.connection, request_key)?
                .map(|stored| stored.record),
        )
    }

    pub fn workflows(&self, limit: usize) -> Result<Vec<WorkflowRecord>, StoreError> {
        workflow_rows::list_workflows(&self.connection, limit.min(MAX_SNAPSHOT_ITEMS))
    }

    pub fn workflow_contract(
        &self,
        contract_id: Sha256Digest,
    ) -> Result<Option<ContractArtifact>, StoreError> {
        workflow_rows::load_contract(&self.connection, contract_id)
    }

    pub fn workflow_snapshot(
        &self,
        workflow_id: Uuid,
    ) -> Result<Option<WorkflowSnapshot>, StoreError> {
        let Some(stored) = workflow_rows::load_workflow(&self.connection, workflow_id)? else {
            return Ok(None);
        };
        let mut contracts = Vec::new();
        for task in &stored.record.tasks {
            let mut next = Some(task.contract_id);
            // Follow the generation chain back so old contracts stay visible.
            while let Some(contract_id) = next {
                let contract = workflow_rows::load_contract(&self.connection, contract_id)?
                    .ok_or_else(|| workflow_error("store_workflow_projection_mismatch"))?;
                next = contract.previous_contract_id;
                contracts.push(contract);
                if contracts.len() > MAX_SNAPSHOT_ITEMS {
                    return Err(workflow_error("store_workflow_snapshot_too_large"));
                }
            }
        }
        let bounded = |count: usize| {
            if count > MAX_SNAPSHOT_ITEMS {
                Err(workflow_error("store_workflow_snapshot_too_large"))
            } else {
                Ok(())
            }
        };
        let leases = workflow_rows::leases_for(&self.connection, workflow_id)?;
        let attempts = workflow_rows::attempts_for(&self.connection, workflow_id)?;
        let receipts = workflow_rows::receipts_for(&self.connection, workflow_id, None)?;
        let messages = workflow_rows::messages_for(&self.connection, workflow_id)?;
        bounded(leases.len())?;
        bounded(attempts.len())?;
        bounded(receipts.len())?;
        bounded(messages.len())?;
        Ok(Some(WorkflowSnapshot {
            record: stored.record,
            sequence: stored.sequence,
            contracts,
            leases,
            attempts,
            receipts,
            messages,
        }))
    }

    /// Moves a workflow between phases. Acceptance is not reachable here;
    /// it needs [`Self::accept_workflow`] and its in-transaction gate.
    /// Cancellation completes only when no lease still holds a reservation.
    pub fn transition_workflow(
        &mut self,
        workflow_id: Uuid,
        expected_version: u64,
        phase: WorkflowPhase,
        reason_code: Option<&str>,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowCommit, StoreError> {
        if phase == WorkflowPhase::Accepted {
            return Err(workflow_error("store_workflow_acceptance_requires_gate"));
        }
        let transaction = self.immediate()?;
        let stored = workflow_rows::load_existing_workflow(&transaction, workflow_id)?;
        if stored.record.version != expected_version {
            return Err(workflow_error("store_workflow_version_conflict"));
        }
        if phase == WorkflowPhase::Cancelled
            && workflow_rows::leases_for(&transaction, workflow_id)?
                .iter()
                .any(|lease| lease.state.holds_reservation())
        {
            return Err(workflow_error("store_workflow_attempts_unsettled"));
        }
        let mut next = stored
            .record
            .with_phase(phase, millis(timestamp))
            .map_err(|error| workflow_error(error.code()))?;
        next.blocked_reason = match phase {
            WorkflowPhase::Blocked => Some(reason_code.unwrap_or("blocked").to_string()),
            WorkflowPhase::Failed => Some(reason_code.unwrap_or("failed").to_string()),
            _ => None,
        };
        let (event, sequence) = append(
            &transaction,
            "workflow_phase_changed",
            format!("workflow_phase:{}:{}", uuid_key(workflow_id), next.version),
            json!({
                "workflow_id": workflow_id,
                "from_phase": stored.record.phase,
                "phase": phase,
                "version": next.version,
                "reason_code": reason_code,
            }),
            timestamp,
        )?;
        workflow_rows::update_workflow(&transaction, &next, stored.record.version, sequence)?;
        transaction.commit()?;
        Ok(WorkflowCommit {
            record: next,
            sequence: event.sequence().get(),
            event: Some(event),
        })
    }

    /// Records a new contract generation for one task. The admitted older
    /// contract and every attempt and receipt bound to it stay unchanged; the
    /// task returns to `pending` under the new contract.
    pub fn add_contract_generation(
        &mut self,
        workflow_id: Uuid,
        expected_version: u64,
        artifact: &ContractArtifact,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowCommit, StoreError> {
        let transaction = self.immediate()?;
        let stored = workflow_rows::load_existing_workflow(&transaction, workflow_id)?;
        if stored.record.version != expected_version {
            return Err(workflow_error("store_workflow_version_conflict"));
        }
        if stored.record.phase.is_terminal() {
            return Err(workflow_error("workflow_invalid_transition"));
        }
        let task_key = SpecIdentifier::new(artifact.task_key.clone())
            .map_err(|error| workflow_error(error.code()))?;
        let mut next = stored.record.clone();
        let task = next
            .task_mut(&task_key)
            .map_err(|error| workflow_error(error.code()))?;
        let successor = artifact.previous_contract_id == Some(task.contract_id)
            && task.contract_version.checked_add(1) == Some(artifact.contract_version)
            && artifact
                .semantic_diff
                .as_ref()
                .is_some_and(|diff| !diff.is_empty())
            && artifact.workflow_key == stored.record.workflow_key.as_str();
        if !successor {
            return Err(workflow_error("store_workflow_invalid_generation"));
        }
        let previous_contract = task.contract_id;
        task.contract_id = artifact.contract_id;
        task.contract_version = artifact.contract_version;
        task.progress = TaskProgress::Pending;
        task.accepted_candidate = None;
        task.failure_code = None;
        next.version = next
            .version
            .checked_add(1)
            .ok_or_else(|| workflow_error("workflow_record_invalid"))?;
        next.updated_at_ms = millis(timestamp).max(next.updated_at_ms);
        let (event, sequence) = append(
            &transaction,
            "workflow_contract_generation",
            format!(
                "workflow_generation:{}:{}",
                uuid_key(workflow_id),
                artifact.contract_id
            ),
            json!({
                "workflow_id": workflow_id,
                "task_key": task_key,
                "previous_contract_id": previous_contract,
                "contract_id": artifact.contract_id,
                "contract_version": artifact.contract_version,
                "version": next.version,
            }),
            timestamp,
        )?;
        workflow_rows::insert_contract_if_absent(&transaction, artifact, sequence)?;
        workflow_rows::update_workflow(&transaction, &next, stored.record.version, sequence)?;
        transaction.commit()?;
        Ok(WorkflowCommit {
            record: next,
            sequence: event.sequence().get(),
            event: Some(event),
        })
    }

    /// Records the latest observation of a slot. An identical observation
    /// writes nothing. Lease state is never taken from the caller.
    pub fn upsert_workflow_slot(
        &mut self,
        slot: &SlotFacts,
        timestamp: OffsetDateTime,
    ) -> Result<Option<EventEnvelope>, StoreError> {
        if slot.leased_generation.is_some() {
            return Err(workflow_error("store_workflow_slot_lease_not_settable"));
        }
        let transaction = self.immediate()?;
        if let Some((existing, _)) = workflow_rows::load_slot(&transaction, slot.slot_id.as_str())?
        {
            if existing == *slot {
                return Ok(None);
            }
        }
        let digest = Sha256Digest::of(serde_json::to_string(slot)?.as_bytes());
        let (event, sequence) = append(
            &transaction,
            "workflow_slot_observed",
            format!("workflow_slot:{}:{}", slot.slot_id, digest),
            json!({
                "slot_id": slot.slot_id,
                "harness": slot.harness,
                "interface_mode": slot.interface_mode,
                "health": slot.health,
                "capabilities": slot.capabilities.keys().collect::<Vec<_>>(),
                "record_sha256": digest,
            }),
            timestamp,
        )?;
        workflow_rows::upsert_slot(&transaction, slot, sequence)?;
        transaction.commit()?;
        Ok(Some(event))
    }

    /// Slots with their current lease generation filled in from leases.
    pub fn workflow_slots(&self) -> Result<Vec<SlotFacts>, StoreError> {
        let holding = workflow_rows::holding_leases(&self.connection)?;
        let mut slots = workflow_rows::all_slots(&self.connection)?;
        for slot in &mut slots {
            slot.leased_generation = holding
                .iter()
                .find(|lease| lease.slot_id == slot.slot_id)
                .map(|lease| lease.generation);
        }
        Ok(slots)
    }

    /// Atomically reserves a slot and a worktree under the next generation.
    pub fn acquire_workflow_lease(
        &mut self,
        request: &LeaseRequest,
    ) -> Result<LeaseCommit, StoreError> {
        let transaction = self.immediate()?;
        if let Some(existing) = workflow_rows::load_lease(&transaction, request.lease_id)? {
            let same = existing.workflow_id == request.workflow_id
                && existing.task_key == request.task_key
                && existing.slot_id == request.slot_id
                && existing.worktree_key == request.worktree_key;
            if !same {
                return Err(workflow_error("store_workflow_lease_conflict"));
            }
            return Ok(LeaseCommit {
                lease: existing,
                event: None,
            });
        }
        let workflow = workflow_rows::load_existing_workflow(&transaction, request.workflow_id)?;
        if !workflow.record.phase.admits_work() {
            return Err(workflow_error("store_workflow_not_running"));
        }
        let task = workflow
            .record
            .task(&request.task_key)
            .ok_or_else(|| workflow_error("workflow_unknown_task"))?;
        if matches!(task.progress, TaskProgress::Accepted) {
            return Err(workflow_error("store_workflow_task_settled"));
        }
        let (_, last_generation) =
            workflow_rows::load_slot(&transaction, request.slot_id.as_str())?
                .ok_or_else(|| workflow_error("store_workflow_unknown_slot"))?;
        if workflow_rows::holder(&transaction, "slot_id", request.slot_id.as_str())?.is_some() {
            return Err(workflow_error("store_workflow_slot_busy"));
        }
        if workflow_rows::holder(&transaction, "worktree_key", &request.worktree_key.to_hex())?
            .is_some()
        {
            return Err(workflow_error("store_workflow_worktree_reserved"));
        }
        let generation = last_generation
            .checked_add(1)
            .ok_or_else(|| workflow_error("store_workflow_version_conflict"))?;
        let now = millis(request.timestamp);
        let lease = LeaseRecord {
            schema_version: WORKFLOW_STORE_RECORD_SCHEMA_VERSION,
            lease_id: request.lease_id,
            workflow_id: request.workflow_id,
            task_key: request.task_key.clone(),
            slot_id: request.slot_id.clone(),
            worktree_key: request.worktree_key,
            generation,
            state: LeaseState::Active,
            heartbeat_deadline_ms: request.heartbeat_deadline_ms,
            end_reason: None,
            created_at_ms: now,
            updated_at_ms: now,
        };
        let (event, sequence) = append(
            &transaction,
            "workflow_lease_acquired",
            format!("workflow_lease:{}:acquired", uuid_key(request.lease_id)),
            json!({
                "lease_id": request.lease_id,
                "workflow_id": request.workflow_id,
                "task_key": request.task_key,
                "slot_id": request.slot_id,
                "generation": generation,
                "worktree_key": request.worktree_key,
            }),
            request.timestamp,
        )?;
        workflow_rows::bump_slot_generation(
            &transaction,
            request.slot_id.as_str(),
            last_generation,
            generation,
        )?;
        workflow_rows::insert_lease(&transaction, &lease, sequence)?;
        transaction.commit()?;
        Ok(LeaseCommit {
            lease,
            event: Some(event),
        })
    }

    pub fn workflow_lease(&self, lease_id: Uuid) -> Result<Option<LeaseRecord>, StoreError> {
        workflow_rows::load_lease(&self.connection, lease_id)
    }

    /// Applies a lease event through the pure reducer. Whether the lease's
    /// attempt has settled is read inside the transaction, never trusted
    /// from the caller.
    pub fn change_workflow_lease(
        &mut self,
        lease_id: Uuid,
        event: LeaseEvent,
        heartbeat_ms: u64,
        reason_code: &str,
        timestamp: OffsetDateTime,
    ) -> Result<LeaseCommit, StoreError> {
        let transaction = self.immediate()?;
        let lease = workflow_rows::load_lease(&transaction, lease_id)?
            .ok_or_else(|| workflow_error("store_workflow_unknown_lease"))?;
        let attempts = workflow_rows::attempts_for_lease(&transaction, lease_id)?;
        let mut settled = true;
        for attempt in attempts
            .iter()
            .filter(|attempt| attempt.lease_generation == lease.generation)
        {
            let phase = workflow_rows::dispatch_phase(&transaction, attempt.request_id)?
                .ok_or_else(|| workflow_error("store_workflow_projection_mismatch"))?;
            settled &= dispatch_terminal(&phase);
        }
        let event = match event {
            LeaseEvent::Revoke { generation, .. } => LeaseEvent::Revoke {
                generation,
                attempt_settled: settled,
            },
            LeaseEvent::AttemptSettled { .. }
            | LeaseEvent::Reconciled {
                process_gone: true, ..
            } if !settled => {
                return Err(workflow_error("workflow_lease_attempt_unsettled"));
            }
            other => other,
        };
        let next_view = apply_lease(lease.view(), event, heartbeat_ms)
            .map_err(|error| workflow_error(error.code()))?;
        if next_view == lease.view() {
            return Ok(LeaseCommit { lease, event: None });
        }
        let mut next = lease.clone();
        next.state = next_view.state;
        next.heartbeat_deadline_ms = next_view.heartbeat_deadline_ms;
        next.updated_at_ms = millis(timestamp).max(lease.updated_at_ms);
        if !next.state.holds_reservation() {
            next.end_reason = Some(reason_code.to_string());
        }
        let (envelope, sequence) = append(
            &transaction,
            "workflow_lease_changed",
            format!(
                "workflow_lease:{}:{}:{}:{}",
                uuid_key(lease_id),
                next.state.as_str(),
                next.heartbeat_deadline_ms,
                reason_code
            ),
            json!({
                "lease_id": lease_id,
                "workflow_id": lease.workflow_id,
                "generation": lease.generation,
                "from_state": lease.state,
                "state": next.state,
                "reason_code": reason_code,
            }),
            timestamp,
        )?;
        workflow_rows::update_lease(&transaction, &next, lease.state.as_str(), sequence)?;
        transaction.commit()?;
        Ok(LeaseCommit {
            lease: next,
            event: Some(envelope),
        })
    }

    /// Admits one worker, answer, repair, or review turn: the workflow must
    /// admit work, the lease must be active under the claimed generation for
    /// this task, and the dispatch's working-directory reservation must be
    /// the lease's worktree. The dispatch record, its Task/Run, the
    /// association, the task budget charge, and both events commit together.
    pub fn admit_workflow_attempt(
        &mut self,
        admission: &WorkflowAttemptAdmission,
    ) -> Result<WorkflowAttemptCommit, StoreError> {
        let transaction = self.immediate()?;
        if let Some(existing) =
            workflow_rows::load_attempt(&transaction, admission.dispatch.request_id)?
        {
            let same = existing.workflow_id == admission.workflow_id
                && existing.lease_id == admission.lease_id
                && existing.lease_generation == admission.lease_generation
                && existing.task_key == admission.task_key
                && existing.contract_id == admission.contract_id
                && existing.purpose == admission.purpose
                && existing.session_id == admission.session_id;
            if !same {
                return Err(workflow_error("store_workflow_attempt_conflict"));
            }
            let dispatch = admit_dispatch_in(&transaction, &admission.dispatch)?;
            let workflow =
                workflow_rows::load_existing_workflow(&transaction, admission.workflow_id)?.record;
            transaction.commit()?;
            return Ok(WorkflowAttemptCommit {
                attempt: existing,
                dispatch,
                workflow,
                duplicate: true,
            });
        }
        let stored = workflow_rows::load_existing_workflow(&transaction, admission.workflow_id)?;
        if stored.record.version != admission.expected_workflow_version {
            return Err(workflow_error("store_workflow_version_conflict"));
        }
        if !stored.record.phase.admits_work() {
            return Err(workflow_error("store_workflow_not_running"));
        }
        let lease = workflow_rows::load_lease(&transaction, admission.lease_id)?
            .ok_or_else(|| workflow_error("store_workflow_unknown_lease"))?;
        if lease.state != LeaseState::Active {
            return Err(workflow_error("workflow_lease_not_active"));
        }
        if lease.generation != admission.lease_generation {
            return Err(workflow_error("workflow_lease_stale_generation"));
        }
        if lease.workflow_id != admission.workflow_id
            || lease.task_key != admission.task_key
            || lease.worktree_key != admission.dispatch.resource_key
        {
            return Err(workflow_error("store_workflow_lease_mismatch"));
        }
        let mut next = stored.record.clone();
        let task = next
            .task(&admission.task_key)
            .ok_or_else(|| workflow_error("workflow_unknown_task"))?;
        if task.contract_id != admission.contract_id {
            return Err(workflow_error("store_workflow_stale_contract"));
        }
        match admission.purpose {
            AttemptPurpose::Implement | AttemptPurpose::Repair | AttemptPurpose::Answer => {
                next.charge_turn(
                    &admission.task_key,
                    admission.purpose == AttemptPurpose::Repair,
                )
                .map_err(|error| workflow_error(error.code()))?;
                if admission.purpose != AttemptPurpose::Answer {
                    let compare = next.mode == WorkflowMode::Compare;
                    let task = next
                        .task_mut(&admission.task_key)
                        .map_err(|error| workflow_error(error.code()))?;
                    // Compare competitors share one task: a later competitor
                    // turn must not hide a candidate already collected.
                    if !(compare && task.progress == TaskProgress::CandidateCollected) {
                        task.progress = TaskProgress::Running;
                        task.failure_code = None;
                    }
                }
            }
            AttemptPurpose::Review => {}
        }
        next.version = next
            .version
            .checked_add(1)
            .ok_or_else(|| workflow_error("workflow_record_invalid"))?;
        next.updated_at_ms = millis(admission.dispatch.timestamp).max(next.updated_at_ms);
        let dispatch = admit_dispatch_in(&transaction, &admission.dispatch)?;
        if dispatch.event.is_none() {
            // The request ID belongs to a dispatch that is not this attempt.
            return Err(workflow_error("store_workflow_attempt_conflict"));
        }
        let attempt = WorkflowAttemptRecord {
            schema_version: WORKFLOW_STORE_RECORD_SCHEMA_VERSION,
            request_id: admission.dispatch.request_id,
            workflow_id: admission.workflow_id,
            lease_id: admission.lease_id,
            lease_generation: admission.lease_generation,
            task_key: admission.task_key.clone(),
            contract_id: admission.contract_id,
            purpose: admission.purpose,
            session_id: admission.session_id,
            rendered_prompt_sha256: admission.dispatch.prompt.sha256,
            created_at_ms: millis(admission.dispatch.timestamp),
        };
        let (_, sequence) = append(
            &transaction,
            "workflow_attempt_admitted",
            format!("workflow_attempt:{}", uuid_key(attempt.request_id)),
            json!({
                "workflow_id": attempt.workflow_id,
                "request_id": attempt.request_id,
                "lease_id": attempt.lease_id,
                "lease_generation": attempt.lease_generation,
                "task_key": attempt.task_key,
                "contract_id": attempt.contract_id,
                "purpose": attempt.purpose,
                "session_id": attempt.session_id,
                "rendered_prompt_sha256": attempt.rendered_prompt_sha256,
                "version": next.version,
            }),
            admission.dispatch.timestamp,
        )?;
        workflow_rows::insert_attempt(&transaction, &attempt, sequence)?;
        workflow_rows::update_workflow(&transaction, &next, stored.record.version, sequence)?;
        transaction.commit()?;
        Ok(WorkflowAttemptCommit {
            attempt,
            dispatch,
            workflow: next,
            duplicate: false,
        })
    }

    pub fn workflow_attempt(
        &self,
        request_id: Uuid,
    ) -> Result<Option<WorkflowAttemptRecord>, StoreError> {
        workflow_rows::load_attempt(&self.connection, request_id)
    }

    /// Records one immutable receipt after checking that it is bound to
    /// entities of this workflow. A candidate must carry the fence of an
    /// active lease whose settled attempt produced it.
    pub fn record_workflow_receipt(
        &mut self,
        workflow_id: Uuid,
        receipt: &WorkflowReceipt,
        fence: Option<LeaseFence>,
        timestamp: OffsetDateTime,
    ) -> Result<ReceiptCommit, StoreError> {
        if receipt
            .workflow_id()
            .is_some_and(|claimed| claimed != workflow_id)
        {
            return Err(workflow_error("store_workflow_receipt_wrong_workflow"));
        }
        let digest = receipt
            .digest()
            .map_err(|error| workflow_error(error.code()))?;
        let transaction = self.immediate()?;
        if let Some((_, stored_digest)) =
            workflow_rows::load_receipt(&transaction, receipt.receipt_id())?
        {
            if stored_digest != digest.to_hex() {
                return Err(workflow_error("store_workflow_receipt_conflict"));
            }
            return Ok(ReceiptCommit {
                receipt_digest: digest,
                event: None,
                duplicate: true,
            });
        }
        let stored = workflow_rows::load_existing_workflow(&transaction, workflow_id)?;
        let mut next = stored.record.clone();
        let summary = match receipt {
            WorkflowReceipt::Candidate(candidate) => {
                check_candidate(&transaction, &stored.record, candidate, fence)?;
                // In compare mode a refused competitor does not fail a task
                // that already holds an admitted competitor candidate.
                let keeps_collected = stored.record.mode == WorkflowMode::Compare
                    && admitted_candidates(&transaction, workflow_id)?
                        .iter()
                        .any(|other| other.candidate.task_key == candidate.candidate.task_key);
                let task = next
                    .task_mut(&candidate.candidate.task_key)
                    .map_err(|error| workflow_error(error.code()))?;
                if candidate.admitted {
                    task.progress = TaskProgress::CandidateCollected;
                } else if !keeps_collected {
                    task.progress = TaskProgress::Failed;
                    task.failure_code = candidate.violation_codes.first().cloned();
                }
                json!({"admitted": candidate.admitted, "violation_count": candidate.violation_codes.len()})
            }
            WorkflowReceipt::Review(review) => {
                check_review(&transaction, workflow_id, review)?;
                json!({"verdict": review.verdict, "findings_count": review.findings_count})
            }
            WorkflowReceipt::Verification(verification) => {
                check_verification_subject(&transaction, &stored.record, verification)?;
                let failed: Vec<&str> = verification
                    .suites
                    .iter()
                    .filter(|suite| suite.status != vibemux_workflow::gates::SuiteStatus::Passed)
                    .map(|suite| suite.suite_id.as_str())
                    .collect();
                json!({"suite_count": verification.suites.len(), "unpassed_suites": failed, "subject_unchanged": verification.subject_unchanged})
            }
            WorkflowReceipt::Integration(integration) => {
                let accepted: Vec<Sha256Digest> = stored
                    .record
                    .tasks
                    .iter()
                    .filter_map(|task| task.accepted_candidate)
                    .collect();
                if integration
                    .applied_candidates
                    .iter()
                    .any(|digest| !accepted.contains(digest))
                {
                    return Err(workflow_error("store_workflow_integration_not_approved"));
                }
                json!({"applied_count": integration.applied_candidates.len(), "conflict_count": integration.conflicts.len()})
            }
            WorkflowReceipt::Selection(selection) => {
                let candidates = admitted_candidates(&transaction, workflow_id)?;
                if let Some(winner) = selection.outcome.winner {
                    if !candidates
                        .iter()
                        .any(|candidate| candidate.candidate.candidate_digest == winner)
                    {
                        return Err(workflow_error("store_workflow_selection_unknown_candidate"));
                    }
                }
                json!({"has_winner": selection.outcome.winner.is_some(), "excluded_count": selection.outcome.excluded.len()})
            }
            WorkflowReceipt::Decision(decision) => {
                json!({"tool": decision.tool, "accepted": decision.accepted})
            }
            WorkflowReceipt::ModelCall(call) => {
                json!({"role": call.role, "call_outcome": call.outcome, "usage_known": !call.usage.is_unknown()})
            }
            WorkflowReceipt::Bundle(bundle) => {
                if stored.record.task(&bundle.source_task).is_none() {
                    return Err(workflow_error("workflow_unknown_task"));
                }
                json!({"byte_count": bundle.byte_count, "redacted_values": bundle.redaction.redacted_values})
            }
        };
        let task_changed = next != stored.record;
        if task_changed {
            next.version = next
                .version
                .checked_add(1)
                .ok_or_else(|| workflow_error("workflow_record_invalid"))?;
            next.updated_at_ms = millis(timestamp).max(next.updated_at_ms);
        }
        let subject = receipt
            .subject_digest()
            .map_err(|error| workflow_error(error.code()))?;
        let (event, sequence) = append(
            &transaction,
            "workflow_receipt_recorded",
            format!("workflow_receipt:{}", uuid_key(receipt.receipt_id())),
            json!({
                "workflow_id": workflow_id,
                "kind": receipt.kind(),
                "receipt_id": receipt.receipt_id(),
                "receipt_sha256": digest,
                "subject_sha256": subject,
                "task_key": receipt.task_key(),
                "summary": summary,
            }),
            timestamp,
        )?;
        workflow_rows::insert_receipt(&transaction, workflow_id, receipt, digest, sequence)?;
        if task_changed {
            workflow_rows::update_workflow(&transaction, &next, stored.record.version, sequence)?;
        }
        transaction.commit()?;
        Ok(ReceiptCommit {
            receipt_digest: digest,
            event: Some(event),
            duplicate: false,
        })
    }

    /// Runs the candidate gate inside the transaction on the stored receipts
    /// and, if it passes, marks the task accepted with that candidate.
    pub fn accept_workflow_candidate(
        &mut self,
        workflow_id: Uuid,
        expected_version: u64,
        task_key: &SpecIdentifier,
        candidate_digest: Sha256Digest,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowCommit, StoreError> {
        let transaction = self.immediate()?;
        let stored = workflow_rows::load_existing_workflow(&transaction, workflow_id)?;
        if stored.record.version != expected_version {
            return Err(workflow_error("store_workflow_version_conflict"));
        }
        if !stored.record.phase.admits_work() {
            return Err(workflow_error("store_workflow_not_running"));
        }
        let acceptance = gate_candidate(&transaction, &stored.record, task_key, candidate_digest)?;
        let mut next = stored.record.clone();
        let task = next
            .task_mut(task_key)
            .map_err(|error| workflow_error(error.code()))?;
        if task.progress != TaskProgress::CandidateCollected {
            return Err(workflow_error("store_workflow_task_not_collected"));
        }
        task.progress = TaskProgress::Accepted;
        task.accepted_candidate = Some(candidate_digest);
        next.version = next
            .version
            .checked_add(1)
            .ok_or_else(|| workflow_error("workflow_record_invalid"))?;
        next.updated_at_ms = millis(timestamp).max(next.updated_at_ms);
        let (event, sequence) = append(
            &transaction,
            "workflow_candidate_accepted",
            format!(
                "workflow_candidate_accepted:{}:{}",
                uuid_key(workflow_id),
                candidate_digest
            ),
            json!({
                "workflow_id": workflow_id,
                "task_key": task_key,
                "candidate_sha256": candidate_digest,
                "contract_id": acceptance.contract_id,
                "review_receipt_sha256": acceptance.review_receipt,
                "verifier_receipt_sha256": acceptance.verifier_receipt,
                "level": acceptance.level,
                "version": next.version,
            }),
            timestamp,
        )?;
        workflow_rows::update_workflow(&transaction, &next, stored.record.version, sequence)?;
        transaction.commit()?;
        Ok(WorkflowCommit {
            record: next,
            sequence: event.sequence().get(),
            event: Some(event),
        })
    }

    /// The only path to `accepted`: re-runs every candidate gate and the
    /// integration gate on stored receipts inside the transaction. A
    /// cancelled, failed, or not-yet-integrating workflow cannot pass, so a
    /// delayed child result cannot promote it.
    pub fn accept_workflow(
        &mut self,
        workflow_id: Uuid,
        expected_version: u64,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowCommit, StoreError> {
        let transaction = self.immediate()?;
        let stored = workflow_rows::load_existing_workflow(&transaction, workflow_id)?;
        if stored.record.version != expected_version {
            return Err(workflow_error("store_workflow_version_conflict"));
        }
        let record = &stored.record;
        let selected: Option<Sha256Digest> = if record.mode == WorkflowMode::Compare {
            let selections =
                workflow_rows::receipts_for(&transaction, workflow_id, Some("selection"))?;
            match selections.last() {
                Some(WorkflowReceipt::Selection(selection)) => selection.outcome.winner,
                _ => None,
            }
        } else {
            None
        };
        let mut expected = Vec::new();
        let mut accepted = Vec::new();
        for task in &record.tasks {
            let counts = match (record.mode, selected) {
                (WorkflowMode::Compare, Some(winner)) => task.accepted_candidate == Some(winner),
                (WorkflowMode::Compare, None) => false,
                _ => true,
            };
            if !counts {
                continue;
            }
            expected.push(task.task_key.clone());
            if let Some(candidate) = task.accepted_candidate {
                accepted.push((
                    task.task_key.clone(),
                    gate_candidate(&transaction, record, &task.task_key, candidate)?,
                ));
            }
        }
        if expected.is_empty() {
            return Err(workflow_error("store_workflow_gate_failed"));
        }
        let integrations =
            workflow_rows::receipts_for(&transaction, workflow_id, Some("integration"))?;
        let integration: Option<&IntegrationReceipt> =
            integrations.iter().rev().find_map(|receipt| match receipt {
                WorkflowReceipt::Integration(integration) => Some(integration),
                _ => None,
            });
        let verifications =
            workflow_rows::receipts_for(&transaction, workflow_id, Some("verification"))?;
        let integration_verification: Option<&VerifierReceipt> =
            integration.and_then(|integration| {
                verifications
                    .iter()
                    .rev()
                    .find_map(|receipt| match receipt {
                        WorkflowReceipt::Verification(verification)
                            if verification.subject_digest
                                == integration.integration_tree_digest
                                && verification.task_key.is_none() =>
                        {
                            Some(verification)
                        }
                        _ => None,
                    })
            });
        let level = cooperative_acceptance(
            record.phase,
            &expected,
            &accepted,
            integration,
            integration_verification,
            GateRequirements {
                required_suites: &record.integration_suites,
                admitted_verifier_digest: record.verifier_digest,
            },
        )
        .map_err(|_| workflow_error("store_workflow_gate_failed"))?;
        if level != AcceptanceLevel::WorkflowAccepted {
            return Err(workflow_error("store_workflow_gate_failed"));
        }
        let next = record
            .with_phase(WorkflowPhase::Accepted, millis(timestamp))
            .map_err(|error| workflow_error(error.code()))?;
        let (event, sequence) = append(
            &transaction,
            "workflow_accepted",
            format!("workflow_accepted:{}", uuid_key(workflow_id)),
            json!({
                "workflow_id": workflow_id,
                "mode": record.mode,
                "accepted_candidates": accepted.iter().map(|(_, acceptance)| acceptance.candidate_digest).collect::<Vec<_>>(),
                "integration_tree_sha256": integration.map(|integration| integration.integration_tree_digest),
                "version": next.version,
            }),
            timestamp,
        )?;
        workflow_rows::update_workflow(&transaction, &next, record.version, sequence)?;
        transaction.commit()?;
        Ok(WorkflowCommit {
            record: next,
            sequence: event.sequence().get(),
            event: Some(event),
        })
    }

    /// Admits a broker-validated envelope with the next per-recipient
    /// sequence. A repeated message ID with the same content is a no-op.
    pub fn admit_workflow_message(
        &mut self,
        envelope: &MessageEnvelope,
        timestamp: OffsetDateTime,
    ) -> Result<MessageCommit, StoreError> {
        let fingerprint = message_fingerprint(envelope)?;
        let transaction = self.immediate()?;
        if let Some(existing) = workflow_rows::load_message(&transaction, envelope.message_id)? {
            if existing.fingerprint != fingerprint {
                return Err(workflow_error("store_workflow_message_conflict"));
            }
            return Ok(MessageCommit {
                message: existing,
                event: None,
            });
        }
        let stored = workflow_rows::load_existing_workflow(&transaction, envelope.workflow_id)?;
        if stored.record.phase.is_terminal() {
            return Err(workflow_error("store_workflow_not_running"));
        }
        let recipient_label = envelope.recipient.label();
        let recipient_sequence = workflow_rows::next_recipient_sequence(
            &transaction,
            envelope.workflow_id,
            &recipient_label,
        )?;
        let message = MessageRecord {
            schema_version: WORKFLOW_STORE_RECORD_SCHEMA_VERSION,
            envelope: envelope.clone(),
            recipient_label,
            recipient_sequence,
            state: MessageState::Admitted,
            fingerprint,
            delivered_in_attempt: None,
            acknowledged_in_attempt: None,
            updated_at_ms: millis(timestamp),
        };
        let (event, sequence) = append(
            &transaction,
            "workflow_message_admitted",
            format!(
                "workflow_message:{}:admitted",
                uuid_key(envelope.message_id)
            ),
            json!({
                "workflow_id": envelope.workflow_id,
                "message_id": envelope.message_id,
                "kind": envelope.kind,
                "sender": envelope.sender.label(),
                "recipient": message.recipient_label,
                "recipient_sequence": recipient_sequence,
                "correlation_id": envelope.correlation_id,
                "body_sha256": envelope.body_sha256,
                "body_bytes": envelope.body_bytes,
                "source_ref_count": envelope.source_refs.len(),
                "escalated": envelope.escalated,
            }),
            timestamp,
        )?;
        workflow_rows::insert_message(&transaction, &message, sequence)?;
        transaction.commit()?;
        Ok(MessageCommit {
            message,
            event: Some(event),
        })
    }

    /// Records a refused draft as content-free evidence.
    pub fn record_workflow_message_rejection(
        &mut self,
        workflow_id: Uuid,
        message_id: Uuid,
        sender: &Participant,
        code: &str,
        timestamp: OffsetDateTime,
    ) -> Result<Option<EventEnvelope>, StoreError> {
        let transaction = self.immediate()?;
        workflow_rows::load_existing_workflow(&transaction, workflow_id)?;
        let key = format!("workflow_message:{}:rejected", uuid_key(message_id));
        if let Some(existing) = SqliteStore::find_duplicate(&transaction, &key)? {
            transaction.commit()?;
            return Ok(Some(existing));
        }
        let (event, _) = append(
            &transaction,
            "workflow_message_rejected",
            key,
            json!({"workflow_id": workflow_id, "message_id": message_id, "sender": sender.label(), "rejection_code": code}),
            timestamp,
        )?;
        transaction.commit()?;
        Ok(Some(event))
    }

    /// Delivery, acknowledgement, and expiry through the pure machine.
    /// Repeating a change that already happened writes nothing.
    pub fn change_workflow_message(
        &mut self,
        message_id: Uuid,
        change: &MessageChange,
        timestamp: OffsetDateTime,
    ) -> Result<MessageCommit, StoreError> {
        let transaction = self.immediate()?;
        let message = workflow_rows::load_message(&transaction, message_id)?
            .ok_or_else(|| workflow_error("store_workflow_unknown_message"))?;
        let now = millis(timestamp);
        let event = match change {
            MessageChange::Deliver { .. } => DeliveryEvent::Deliver { now_ms: now },
            MessageChange::Acknowledge { by, .. } => DeliveryEvent::Acknowledge { by },
            MessageChange::Expire => DeliveryEvent::Expire { now_ms: now },
        };
        let state = message_transition(&message.envelope, message.state, &event)
            .map_err(|error| workflow_error(error.code()))?;
        let mut next = message.clone();
        next.state = state;
        match change {
            MessageChange::Deliver { attempt } => {
                if let Some(previous) = message.delivered_in_attempt {
                    if previous != *attempt {
                        return Err(workflow_error("store_workflow_message_redelivery"));
                    }
                }
                next.delivered_in_attempt = Some(*attempt);
            }
            MessageChange::Acknowledge { attempt, .. } => {
                next.acknowledged_in_attempt.get_or_insert(*attempt);
            }
            MessageChange::Expire => {}
        }
        if next == message {
            return Ok(MessageCommit {
                message,
                event: None,
            });
        }
        next.updated_at_ms = now.max(message.updated_at_ms);
        let (envelope, sequence) = append(
            &transaction,
            "workflow_message_changed",
            format!(
                "workflow_message:{}:{}",
                uuid_key(message_id),
                state.as_str()
            ),
            json!({
                "workflow_id": message.envelope.workflow_id,
                "message_id": message_id,
                "from_state": message.state,
                "state": state,
                "delivered_in_attempt": next.delivered_in_attempt,
                "acknowledged_in_attempt": next.acknowledged_in_attempt,
            }),
            timestamp,
        )?;
        workflow_rows::update_message(&transaction, &next, message.state.as_str(), sequence)?;
        transaction.commit()?;
        Ok(MessageCommit {
            message: next,
            event: Some(envelope),
        })
    }

    pub fn workflow_message(&self, message_id: Uuid) -> Result<Option<MessageRecord>, StoreError> {
        workflow_rows::load_message(&self.connection, message_id)
    }

    /// Indexes a blob the daemon wrote to the opt-in content store.
    pub fn record_workflow_content(
        &mut self,
        entry: &ContentEntry,
        timestamp: OffsetDateTime,
    ) -> Result<Option<EventEnvelope>, StoreError> {
        if entry.state != ContentState::Present {
            return Err(workflow_error("store_workflow_content_invalid"));
        }
        let transaction = self.immediate()?;
        if let Some(existing) = workflow_rows::load_content(&transaction, entry.content_sha256)? {
            if existing.state == ContentState::Present {
                return Ok(None);
            }
        }
        let (event, sequence) = append(
            &transaction,
            "workflow_content_recorded",
            format!(
                "workflow_content:{}:present:{}",
                entry.content_sha256, entry.updated_at_ms
            ),
            json!({
                "content_sha256": entry.content_sha256,
                "workflow_id": entry.workflow_id,
                "kind": entry.kind,
                "byte_count": entry.byte_count,
                "retain_until_ms": entry.retain_until_ms,
            }),
            timestamp,
        )?;
        workflow_rows::upsert_content(&transaction, entry, sequence)?;
        transaction.commit()?;
        Ok(Some(event))
    }

    /// Tombstones a blob; the daemon removes the bytes afterwards.
    pub fn delete_workflow_content(
        &mut self,
        sha256: Sha256Digest,
        reason_code: &str,
        timestamp: OffsetDateTime,
    ) -> Result<Option<ContentEntry>, StoreError> {
        let transaction = self.immediate()?;
        let Some(existing) = workflow_rows::load_content(&transaction, sha256)? else {
            return Ok(None);
        };
        if existing.state == ContentState::Deleted {
            return Ok(Some(existing));
        }
        let mut next = existing.clone();
        next.state = ContentState::Deleted;
        next.updated_at_ms = millis(timestamp).max(existing.updated_at_ms);
        let (_, sequence) = append(
            &transaction,
            "workflow_content_deleted",
            format!("workflow_content:{}:deleted", sha256),
            json!({"content_sha256": sha256, "workflow_id": existing.workflow_id, "kind": existing.kind, "reason_code": reason_code}),
            timestamp,
        )?;
        workflow_rows::upsert_content(&transaction, &next, sequence)?;
        transaction.commit()?;
        Ok(Some(next))
    }

    pub fn workflow_content(
        &self,
        sha256: Sha256Digest,
    ) -> Result<Option<ContentEntry>, StoreError> {
        workflow_rows::load_content(&self.connection, sha256)
    }

    pub fn prompt_policy_versions(&self) -> Result<Vec<PolicyVersionRecord>, StoreError> {
        workflow_rows::policy_versions(&self.connection)
    }

    /// Promotes a validated optimizable policy for future admissions only.
    pub fn promote_prompt_policy(
        &mut self,
        policy: &OptimizablePolicy,
        timestamp: OffsetDateTime,
    ) -> Result<PolicyVersionRecord, StoreError> {
        policy
            .validate()
            .map_err(|error| workflow_error(error.code()))?;
        let digest = policy
            .digest()
            .map_err(|error| workflow_error(error.code()))?;
        let transaction = self.immediate()?;
        let existing = workflow_rows::policy_versions(&transaction)?;
        let mut history = PolicyHistory {
            versions: existing
                .iter()
                .map(|record| record.version.clone())
                .collect(),
        };
        let version = history
            .promote(digest)
            .map_err(|error| workflow_error(error.code()))?;
        let (_, sequence) = append(
            &transaction,
            "workflow_policy_promoted",
            format!("workflow_policy:{version}:promoted"),
            json!({"version": version, "policy_sha256": digest}),
            timestamp,
        )?;
        let mut promoted = None;
        for changed in &history.versions {
            let policy_record = existing
                .iter()
                .find(|record| record.version.version == changed.version)
                .map_or_else(|| policy.clone(), |record| record.policy.clone());
            let record = PolicyVersionRecord {
                version: changed.clone(),
                policy: policy_record,
            };
            if changed.version == version {
                promoted = Some(record.clone());
            }
            // The one-active index forbids two active rows at once, so the
            // superseded row is written before the new active one.
            if changed.version != version {
                workflow_rows::upsert_policy_version(&transaction, &record, sequence)?;
            }
        }
        let promoted = promoted.ok_or_else(|| workflow_error("optimizer_invalid_history"))?;
        workflow_rows::upsert_policy_version(&transaction, &promoted, sequence)?;
        transaction.commit()?;
        Ok(promoted)
    }

    /// Rolls the active policy back to its parent.
    pub fn rollback_prompt_policy(
        &mut self,
        timestamp: OffsetDateTime,
    ) -> Result<PolicyVersionRecord, StoreError> {
        let transaction = self.immediate()?;
        let existing = workflow_rows::policy_versions(&transaction)?;
        let mut history = PolicyHistory {
            versions: existing
                .iter()
                .map(|record| record.version.clone())
                .collect(),
        };
        let restored = history
            .rollback()
            .map_err(|error| workflow_error(error.code()))?;
        let (_, sequence) = append(
            &transaction,
            "workflow_policy_rolled_back",
            format!("workflow_policy:{restored}:restored:{}", existing.len()),
            json!({"version": restored}),
            timestamp,
        )?;
        let mut active = None;
        for (changed, record) in history.versions.iter().zip(&existing) {
            let next = PolicyVersionRecord {
                version: changed.clone(),
                policy: record.policy.clone(),
            };
            if changed.version == restored {
                active = Some(next);
            } else {
                workflow_rows::upsert_policy_version(&transaction, &next, sequence)?;
            }
        }
        let active = active.ok_or_else(|| workflow_error("optimizer_invalid_history"))?;
        workflow_rows::upsert_policy_version(&transaction, &active, sequence)?;
        transaction.commit()?;
        Ok(active)
    }

    /// Startup reconciliation, run after dispatch recovery: a lease whose
    /// attempt did not settle is quarantined (ownership is uncertain, so no
    /// second writer may enter its worktree); a lease that never admitted
    /// an attempt is released.
    pub fn recover_workflows(
        &mut self,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowRecovery, StoreError> {
        let holding = workflow_rows::holding_leases(&self.connection)?;
        let mut recovery = WorkflowRecovery::default();
        for lease in holding
            .into_iter()
            .filter(|lease| lease.state == LeaseState::Active)
        {
            let attempts = workflow_rows::attempts_for_lease(&self.connection, lease.lease_id)?;
            let current: Vec<&WorkflowAttemptRecord> = attempts
                .iter()
                .filter(|attempt| attempt.lease_generation == lease.generation)
                .collect();
            let mut unsettled = false;
            for attempt in &current {
                let phase = workflow_rows::dispatch_phase(&self.connection, attempt.request_id)?
                    .unwrap_or_default();
                unsettled |= !dispatch_terminal(&phase);
            }
            if unsettled {
                self.change_workflow_lease(
                    lease.lease_id,
                    LeaseEvent::HeartbeatMissed {
                        now_ms: lease.heartbeat_deadline_ms.saturating_add(1),
                    },
                    0,
                    "daemon_restart",
                    timestamp,
                )?;
                recovery.quarantined_leases.push(lease.lease_id);
            } else if current.is_empty() {
                self.change_workflow_lease(
                    lease.lease_id,
                    LeaseEvent::AttemptSettled {
                        generation: lease.generation,
                    },
                    0,
                    "recovered_without_attempt",
                    timestamp,
                )?;
                recovery.released_leases.push(lease.lease_id);
            }
        }
        Ok(recovery)
    }
}

fn message_fingerprint(envelope: &MessageEnvelope) -> Result<Sha256Digest, StoreError> {
    let mut stable = envelope.clone();
    stable.created_at_ms = 0;
    stable.expires_at_ms = envelope
        .expires_at_ms
        .saturating_sub(envelope.created_at_ms);
    Ok(Sha256Digest::of(serde_json::to_string(&stable)?.as_bytes()))
}

fn admitted_candidates(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
) -> Result<Vec<CandidateReceipt>, StoreError> {
    Ok(
        workflow_rows::receipts_for(transaction, workflow_id, Some("candidate"))?
            .into_iter()
            .filter_map(|receipt| match receipt {
                WorkflowReceipt::Candidate(candidate) if candidate.admitted => Some(candidate),
                _ => None,
            })
            .collect(),
    )
}

fn check_candidate(
    transaction: &Transaction<'_>,
    record: &WorkflowRecord,
    candidate: &CandidateReceipt,
    fence: Option<LeaseFence>,
) -> Result<(), StoreError> {
    let fence = fence.ok_or_else(|| workflow_error("workflow_lease_stale_generation"))?;
    if fence.lease_id != candidate.lease_id
        || fence.generation != candidate.candidate.lease_generation
    {
        return Err(workflow_error("workflow_lease_stale_generation"));
    }
    let lease = workflow_rows::load_lease(transaction, fence.lease_id)?
        .ok_or_else(|| workflow_error("store_workflow_unknown_lease"))?;
    vibemux_workflow::leases::check_fence(lease.view(), fence.generation)
        .map_err(|error| workflow_error(error.code()))?;
    if lease.state != LeaseState::Active {
        return Err(workflow_error("workflow_lease_not_active"));
    }
    if lease.workflow_id != record.workflow_id || lease.task_key != candidate.candidate.task_key {
        return Err(workflow_error("store_workflow_lease_mismatch"));
    }
    let attempt = workflow_rows::load_attempt(transaction, candidate.attempt_request_id)?
        .ok_or_else(|| workflow_error("store_workflow_unknown_attempt"))?;
    if attempt.lease_id != lease.lease_id
        || attempt.lease_generation != lease.generation
        || attempt.task_key != candidate.candidate.task_key
        || attempt.contract_id != candidate.candidate.contract_id
        || attempt.session_id != candidate.candidate.worker_session_id
        || attempt.purpose == AttemptPurpose::Review
    {
        return Err(workflow_error("store_workflow_attempt_mismatch"));
    }
    let phase = workflow_rows::dispatch_phase(transaction, attempt.request_id)?
        .ok_or_else(|| workflow_error("store_workflow_projection_mismatch"))?;
    if !dispatch_terminal(&phase) {
        return Err(workflow_error("workflow_lease_attempt_unsettled"));
    }
    let run_id = workflow_rows::dispatch_run_id(transaction, attempt.request_id)?;
    if run_id.as_deref()
        != Some(
            candidate
                .candidate
                .worker_run_id
                .as_hyphenated()
                .to_string()
                .as_str(),
        )
    {
        return Err(workflow_error("store_workflow_attempt_mismatch"));
    }
    if candidate.admitted {
        let clean = phase == DispatchPhase::Completed.as_str()
            && candidate.manifest.is_some()
            && candidate.violation_codes.is_empty();
        let manifest_digest = candidate
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.digest().ok());
        if !clean || manifest_digest != Some(candidate.candidate.candidate_digest) {
            return Err(workflow_error("store_workflow_candidate_invalid"));
        }
    }
    Ok(())
}

fn check_review(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
    review: &ReviewReceipt,
) -> Result<(), StoreError> {
    let candidates = admitted_candidates(transaction, workflow_id)?;
    if !candidates.iter().any(|candidate| {
        candidate.candidate.candidate_digest == review.candidate_digest
            && candidate.candidate.task_key == review.task_key
    }) {
        return Err(workflow_error("store_workflow_review_unknown_candidate"));
    }
    // The reviewer Run must belong to a review attempt of this workflow.
    let attempts = workflow_rows::attempts_for(transaction, workflow_id)?;
    let reviewer_run = review.reviewer_run_id.as_hyphenated().to_string();
    let mut bound = false;
    for attempt in attempts
        .iter()
        .filter(|attempt| attempt.purpose == AttemptPurpose::Review)
    {
        if workflow_rows::dispatch_run_id(transaction, attempt.request_id)?.as_deref()
            == Some(reviewer_run.as_str())
            && attempt.session_id == review.reviewer_session_id
        {
            bound = true;
        }
    }
    if !bound {
        return Err(workflow_error("store_workflow_review_unbound"));
    }
    Ok(())
}

fn check_verification_subject(
    transaction: &Transaction<'_>,
    record: &WorkflowRecord,
    verification: &VerifierReceipt,
) -> Result<(), StoreError> {
    let subject = verification.subject_digest;
    let known = match &verification.task_key {
        Some(task_key) => admitted_candidates(transaction, record.workflow_id)?
            .iter()
            .any(|candidate| candidate.candidate.candidate_digest == subject && &candidate.candidate.task_key == task_key),
        None => workflow_rows::receipts_for(transaction, record.workflow_id, Some("integration"))?
            .iter()
            .any(|receipt| matches!(receipt, WorkflowReceipt::Integration(integration) if integration.integration_tree_digest == subject)),
    };
    if !known {
        return Err(workflow_error(
            "store_workflow_verification_unknown_subject",
        ));
    }
    Ok(())
}

/// The candidate gate on the latest stored review and verification of
/// `candidate_digest`.
fn gate_candidate(
    transaction: &Transaction<'_>,
    record: &WorkflowRecord,
    task_key: &SpecIdentifier,
    candidate_digest: Sha256Digest,
) -> Result<CandidateAcceptance, StoreError> {
    let task = record
        .task(task_key)
        .ok_or_else(|| workflow_error("workflow_unknown_task"))?;
    let candidate = admitted_candidates(transaction, record.workflow_id)?
        .into_iter()
        .rev()
        .find(|candidate| {
            candidate.candidate.candidate_digest == candidate_digest
                && &candidate.candidate.task_key == task_key
        })
        .ok_or_else(|| workflow_error("store_workflow_gate_failed"))?;
    if candidate.candidate.contract_id != task.contract_id {
        return Err(workflow_error("store_workflow_stale_contract"));
    }
    let reviews = workflow_rows::receipts_for(transaction, record.workflow_id, Some("review"))?;
    let review = reviews.iter().rev().find_map(|receipt| match receipt {
        WorkflowReceipt::Review(review) if review.candidate_digest == candidate_digest => {
            Some(review)
        }
        _ => None,
    });
    let verifications =
        workflow_rows::receipts_for(transaction, record.workflow_id, Some("verification"))?;
    let verification = verifications
        .iter()
        .rev()
        .find_map(|receipt| match receipt {
            WorkflowReceipt::Verification(verification)
                if verification.subject_digest == candidate_digest
                    && verification.task_key.as_ref() == Some(task_key) =>
            {
                Some(verification)
            }
            _ => None,
        });
    candidate_gate(
        &candidate.candidate,
        review,
        verification,
        GateRequirements {
            required_suites: &task.required_suites,
            admitted_verifier_digest: record.verifier_digest,
        },
    )
    .map_err(|_| workflow_error("store_workflow_gate_failed"))
}
