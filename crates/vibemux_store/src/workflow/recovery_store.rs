//! Durable controller transitions and conservative recovery checkpoints.
//!
//! Every controller transition, its audit event, and any effect intent commit
//! in one immediate transaction before the daemon may act. Outbox rows start
//! as unknown, not replayable work: a crash after commit cannot prove whether
//! a submission or tool effect crossed the external boundary.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_events::EventEnvelope;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    canonical_json::canonical_digest,
    checkpoint::{
        ContextSettingsIdentity, MAX_RECOVERY_REFERENCES, RecoveryAction, RecoveryCheckpoint,
        RecoveryContractRef, RecoveryControllerRef, RecoveryEffectRef, RecoveryLeaseRef,
        RecoveryState, RecoveryTaskBudget,
    },
    native_controller::{
        ControllerActor, ControllerCommand, ControllerFence, ControllerOwner, NativeController,
        apply_controller_command,
    },
    policy::OperatorPolicy,
    receipts::WorkflowReceipt,
    slots::EvidenceClass,
    task_spec::TaskSpec,
};

use super::{ContentEntry, ContentKind, append, millis, uuid_key, workflow_error, workflow_rows};
use crate::{SqliteStore, StoreError};

const CONTROLLER_ADMISSION_EVENT: &str = "workflow_controller_admitted";
const CONTROLLER_CHANGE_EVENT: &str = "workflow_controller_changed";
const OUTBOX_CLAIM_EVENT: &str = "workflow_controller_outbox_claimed";
const OUTBOX_FINISH_EVENT: &str = "workflow_controller_outbox_finished";
const RECOVERY_BIND_EVENT: &str = "workflow_recovery_plan_bound";
const RECOVERY_CHECKPOINT_EVENT: &str = "workflow_recovery_checkpoint_saved";
const OUTBOX_EFFECT_UNKNOWN: &str = "effect_unknown";
const PLAN_DIGEST_DOMAIN: &str = "vibemux.workflow.plan.v1";
const MAX_TRANSIENT_PLAN_BYTES: usize = 512 * 1024;

/// Mirrors the complete daemon plan's wire shape only while binding it.
/// The record persisted below retains no request text, specs, or overrides.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundPlan {
    schema_version: u32,
    /// Source-request provenance is checked by the parent admission writer.
    #[serde(rename = "request_sha256")]
    _request_sha256: Sha256Digest,
    policy: OperatorPolicy,
    specs: BTreeMap<SpecIdentifier, TaskSpec>,
    assignments: BTreeMap<SpecIdentifier, Vec<SpecIdentifier>>,
    reviewer_slot: SpecIdentifier,
    #[serde(rename = "template_overrides")]
    _template_overrides: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(rename = "negative_control", default)]
    _negative_control: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerOutboxState {
    Pending,
    Submitted,
    Acknowledged,
    Unknown,
}

impl ControllerOutboxState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Submitted => "submitted",
            Self::Acknowledged => "acknowledged",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerOutboxKind {
    BeginSubmission,
}

/// A prompt-free, exact reservation; state and version are repeated in
/// indexed columns and cross-checked on every load.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerOutboxRecord {
    pub effect_id: Uuid,
    pub controller_key: Uuid,
    /// Physical identity changes only after a terminal handoff.
    pub controller_id: Uuid,
    pub controller_generation: u64,
    pub controller_version: u64,
    pub kind: ControllerOutboxKind,
    pub submission_id: Uuid,
    /// Native/Human input consumes a model request budget at reservation.
    pub charged_model_request: bool,
    pub prompt_digest: Sha256Digest,
    pub prompt_bytes: u64,
    pub state: ControllerOutboxState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerOutboxOutcome {
    Acknowledged,
    DeliveryUnknown,
}

#[derive(Clone, Debug)]
pub struct ControllerOutboxCommit {
    pub controller_key: Uuid,
    pub controller: NativeController,
    pub effect: ControllerOutboxRecord,
    pub duplicate: bool,
    pub event: Option<EventEnvelope>,
}

#[derive(Clone, Debug)]
pub struct ControllerCommit {
    pub controller_key: Uuid,
    pub controller: NativeController,
    pub duplicate: bool,
    pub event: Option<EventEnvelope>,
    pub effect: Option<ControllerOutboxRecord>,
}

/// Only metadata needed to re-derive the plan-bound part of recovery state.
/// TaskSpec text and its raw request never enter this record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPlanBinding {
    pub workflow_id: Uuid,
    pub plan_digest: Sha256Digest,
    pub plan_generation: u64,
    pub context_settings: ContextSettingsIdentity,
    pub preapproved_resume_policy: Option<Sha256Digest>,
    pub task_dependencies: BTreeMap<SpecIdentifier, Vec<SpecIdentifier>>,
    pub task_model_caps: BTreeMap<SpecIdentifier, u32>,
}

#[derive(Clone, Debug)]
pub struct RecoveryBindingCommit {
    pub binding: RecoveryPlanBinding,
    pub duplicate: bool,
    pub event: Option<EventEnvelope>,
}

#[derive(Clone, Debug)]
pub struct RecoveryCheckpointCommit {
    pub checkpoint: RecoveryCheckpoint,
    pub duplicate: bool,
    pub event: Option<EventEnvelope>,
}

struct StoredController {
    controller_key: Uuid,
    record: NativeController,
    task_key: SpecIdentifier,
    sequence: u64,
}

fn load_binding(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
) -> Result<Option<(RecoveryPlanBinding, u64)>, StoreError> {
    let row: Option<(String, String, u64)> = transaction
        .query_row(
            "SELECT workflow_id,record_json,created_sequence FROM workflow_recovery_bindings WHERE workflow_id=?",
            [uuid_key(workflow_id)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((row_id, json, sequence)) = row else {
        return Ok(None);
    };
    let binding: RecoveryPlanBinding = serde_json::from_str(&json)
        .map_err(|_| workflow_error("store_recovery_binding_mismatch"))?;
    if row_id != uuid_key(binding.workflow_id)
        || binding.workflow_id != workflow_id
        || binding.plan_generation == 0
        || binding.task_dependencies.is_empty()
        || binding.task_dependencies.len() != binding.task_model_caps.len()
        || binding.preapproved_resume_policy.is_some()
    {
        return Err(workflow_error("store_recovery_binding_mismatch"));
    }
    Ok(Some((binding, sequence)))
}

fn load_controller(
    transaction: &Transaction<'_>,
    controller_key: Uuid,
) -> Result<Option<StoredController>, StoreError> {
    let row: Option<(String, String, String, String, String, u64)> = transaction
        .query_row(
            "SELECT controller_key, workflow_id, task_key, session_id, record_json, updated_sequence FROM workflow_controllers WHERE controller_key = ?",
            [uuid_key(controller_key)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .optional()?;
    let Some((id, workflow_id, task_key, session_id, json, sequence)) = row else {
        return Ok(None);
    };
    let record: NativeController = serde_json::from_str(&json)
        .map_err(|_| workflow_error("store_controller_projection_mismatch"))?;
    record
        .validate()
        .map_err(|_| workflow_error("store_controller_projection_mismatch"))?;
    let task_key = SpecIdentifier::new(task_key)
        .map_err(|_| workflow_error("store_controller_projection_mismatch"))?;
    if id != uuid_key(controller_key)
        || workflow_id != uuid_key(record.identity.workflow_id)
        || session_id != uuid_key(record.identity.session_id)
    {
        return Err(workflow_error("store_controller_projection_mismatch"));
    }
    let workflow = workflow_rows::load_existing_workflow(transaction, record.identity.workflow_id)?;
    validate_controller_binding(transaction, &record, &workflow.record, &task_key)?;
    Ok(Some(StoredController {
        controller_key,
        record,
        task_key,
        sequence,
    }))
}

fn load_submission_outbox(
    transaction: &Transaction<'_>,
    effect_id: Uuid,
) -> Result<Option<ControllerOutboxRecord>, StoreError> {
    let row: Option<(String, String, u64, String, String)> = transaction
        .query_row(
            "SELECT effect_id,controller_key,controller_version,command_json,state FROM workflow_controller_outbox WHERE effect_id=?",
            [uuid_key(effect_id)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()?;
    let Some((row_id, controller_key, version, json, state)) = row else {
        return Ok(None);
    };
    let outbox: ControllerOutboxRecord = serde_json::from_str(&json)
        .map_err(|_| workflow_error("store_controller_outbox_mismatch"))?;
    if row_id != uuid_key(outbox.effect_id)
        || outbox.effect_id != effect_id
        || controller_key != uuid_key(outbox.controller_key)
        || version != outbox.controller_version
        || state != outbox.state.as_str()
        || outbox.controller_generation == 0
        || outbox.submission_id.is_nil()
        || outbox.prompt_bytes == 0
        || outbox.kind != ControllerOutboxKind::BeginSubmission
    {
        return Err(workflow_error("store_controller_outbox_mismatch"));
    }
    Ok(Some(outbox))
}

fn validate_controller_binding(
    transaction: &Transaction<'_>,
    controller: &NativeController,
    workflow: &vibemux_workflow::workflow_record::WorkflowRecord,
    task_key: &SpecIdentifier,
) -> Result<(), StoreError> {
    let invalid = || workflow_error("store_controller_binding_mismatch");
    if controller.identity.workflow_id != workflow.workflow_id {
        return Err(invalid());
    }
    let task = workflow.task(task_key).ok_or_else(invalid)?;
    if task.contract_id != controller.identity.contract_id
        || task.contract_version != controller.identity.contract_version
    {
        return Err(invalid());
    }
    let contract =
        workflow_rows::load_contract(transaction, task.contract_id)?.ok_or_else(invalid)?;
    if contract.task_key != task_key.as_str()
        || contract.workflow_key != workflow.workflow_key.as_str()
        || contract.contract_version != task.contract_version
        || contract.base_commit != workflow.base_commit
        || contract.policy_digest != workflow.policy_digest
    {
        return Err(invalid());
    }
    Ok(())
}

fn controller_task(
    controller: &NativeController,
    workflow: &vibemux_workflow::workflow_record::WorkflowRecord,
) -> Result<SpecIdentifier, StoreError> {
    let mut matches = workflow.tasks.iter().filter(|task| {
        task.contract_id == controller.identity.contract_id
            && task.contract_version == controller.identity.contract_version
    });
    let task = matches
        .next()
        .ok_or_else(|| workflow_error("store_controller_binding_mismatch"))?;
    if matches.next().is_some() {
        return Err(workflow_error("store_controller_binding_mismatch"));
    }
    Ok(task.task_key.clone())
}

fn actor_kind(actor: ControllerActor) -> &'static str {
    match actor {
        ControllerActor::Automation => "automation",
        ControllerActor::Adapter => "adapter",
        ControllerActor::Human { .. } => "human",
    }
}

/// A content-free intent, not a command for an automatic replay queue.
fn effect_intent(command: ControllerCommand) -> Option<(&'static str, Value, &'static str)> {
    match command {
        ControllerCommand::RequestInterruption { submission_id } => Some((
            "request_interruption",
            json!({"kind":"request_interruption","submission_id":submission_id}),
            OUTBOX_EFFECT_UNKNOWN,
        )),
        ControllerCommand::ResolveApproval {
            challenge,
            decision,
        } => Some((
            "resolve_approval",
            json!({"kind":"resolve_approval","challenge_id":challenge.challenge_id,"decision":decision}),
            OUTBOX_EFFECT_UNKNOWN,
        )),
        ControllerCommand::BeginHandoff(target) => Some((
            "begin_handoff",
            json!({"kind":"begin_handoff","controller_id":target.controller_id,"session_id":target.session_id,"transfer":target.transfer}),
            OUTBOX_EFFECT_UNKNOWN,
        )),
        ControllerCommand::Close => Some((
            "close_controller",
            json!({"kind":"close_controller"}),
            OUTBOX_EFFECT_UNKNOWN,
        )),
        _ => None,
    }
}

fn update_controller_row(
    transaction: &Transaction<'_>,
    stored: &StoredController,
    next: &NativeController,
    sequence: i64,
) -> Result<(), StoreError> {
    let updated = transaction.execute(
        "UPDATE workflow_controllers SET session_id=?,record_json=?,updated_sequence=? WHERE controller_key=? AND updated_sequence=?",
        params![
            uuid_key(next.identity.session_id),
            serde_json::to_string(next)?,
            sequence,
            uuid_key(stored.controller_key),
            stored.sequence,
        ],
    )?;
    if updated != 1 {
        return Err(workflow_error("store_controller_version_conflict"));
    }
    Ok(())
}

fn update_submission_outbox(
    transaction: &Transaction<'_>,
    previous: &ControllerOutboxRecord,
    next: &ControllerOutboxRecord,
) -> Result<(), StoreError> {
    let changed = transaction.execute(
        "UPDATE workflow_controller_outbox SET controller_version=?,command_json=?,state=? WHERE effect_id=? AND controller_version=? AND state=?",
        params![
            next.controller_version,
            serde_json::to_string(next)?,
            next.state.as_str(),
            uuid_key(next.effect_id),
            previous.controller_version,
            previous.state.as_str(),
        ],
    )?;
    if changed != 1 {
        return Err(workflow_error("store_controller_outbox_conflict"));
    }
    Ok(())
}

fn charged_model_requests_for(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
    task_key: &SpecIdentifier,
) -> Result<u32, StoreError> {
    let mut statement = transaction.prepare(
        "SELECT o.controller_key,o.command_json,o.state FROM workflow_controller_outbox o JOIN workflow_controllers c ON c.controller_key=o.controller_key WHERE c.workflow_id=? AND c.task_key=?",
    )?;
    let rows = statement.query_map(params![uuid_key(workflow_id), task_key.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut used = 0_u32;
    let mut scanned = 0_usize;
    for row in rows {
        scanned += 1;
        if scanned > MAX_RECOVERY_REFERENCES {
            return Err(workflow_error("store_recovery_projection_too_large"));
        }
        let (controller_key, json, state) = row?;
        let value: Value = serde_json::from_str(&json)
            .map_err(|_| workflow_error("store_controller_outbox_mismatch"))?;
        if value.get("kind").and_then(Value::as_str) == Some("begin_submission") {
            let record: ControllerOutboxRecord = serde_json::from_value(value)
                .map_err(|_| workflow_error("store_controller_outbox_mismatch"))?;
            if controller_key != uuid_key(record.controller_key) || state != record.state.as_str() {
                return Err(workflow_error("store_controller_outbox_mismatch"));
            }
            if record.charged_model_request {
                used = used
                    .checked_add(1)
                    .ok_or_else(|| workflow_error("store_controller_budget_exhausted"))?;
            }
        } else if state != OUTBOX_EFFECT_UNKNOWN {
            return Err(workflow_error("store_controller_outbox_mismatch"));
        }
    }
    Ok(used)
}

struct ReceiptReferences {
    artifacts: Vec<Sha256Digest>,
    verifiers: Vec<Uuid>,
    bundle_content: BTreeSet<Sha256Digest>,
    sequence: u64,
}

fn receipt_references(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
    verifier_digest: Sha256Digest,
    base_commit: &str,
) -> Result<ReceiptReferences, StoreError> {
    let mut statement = transaction.prepare(
        "SELECT receipt_id,kind,task_key,subject_digest,receipt_digest,record_json,created_sequence FROM workflow_receipts WHERE workflow_id=? ORDER BY created_sequence,rowid LIMIT ?",
    )?;
    let rows = statement.query_map(
        params![
            uuid_key(workflow_id),
            i64::try_from(MAX_RECOVERY_REFERENCES + 1).unwrap_or(i64::MAX)
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, u64>(6)?,
            ))
        },
    )?;
    let mut receipts = Vec::new();
    let mut sequence = 0_u64;
    for row in rows {
        let (id, kind, task_key, subject, digest, json, row_sequence) = row?;
        let receipt: WorkflowReceipt = serde_json::from_str(&json)
            .map_err(|_| workflow_error("store_recovery_receipt_mismatch"))?;
        if id != uuid_key(receipt.receipt_id())
            || kind != receipt.kind()
            || task_key.as_deref() != receipt.task_key().map(SpecIdentifier::as_str)
            || subject
                != receipt
                    .subject_digest()
                    .map_err(|_| workflow_error("store_recovery_receipt_mismatch"))?
                    .to_hex()
            || digest
                != receipt
                    .digest()
                    .map_err(|_| workflow_error("store_recovery_receipt_mismatch"))?
                    .to_hex()
        {
            return Err(workflow_error("store_recovery_receipt_mismatch"));
        }
        sequence = sequence.max(row_sequence);
        receipts.push(receipt);
    }
    if receipts.len() > MAX_RECOVERY_REFERENCES {
        return Err(workflow_error("store_recovery_projection_too_large"));
    }
    let mut admitted_candidates = BTreeSet::new();
    let mut all_candidates = BTreeSet::new();
    let mut integrations = Vec::new();
    let mut verifier_subjects = Vec::new();
    let mut verifier_ids = Vec::new();
    let mut review_subjects = Vec::new();
    let mut selections = Vec::new();
    let mut bundle_sources = Vec::new();
    let mut bundle_content = BTreeSet::new();
    for receipt in &receipts {
        if receipt.workflow_id().is_some_and(|id| id != workflow_id) {
            return Err(workflow_error("store_recovery_receipt_mismatch"));
        }
        match receipt {
            WorkflowReceipt::Candidate(candidate) => {
                let attempt =
                    workflow_rows::load_attempt(transaction, candidate.attempt_request_id)?
                        .ok_or_else(|| workflow_error("store_recovery_receipt_mismatch"))?;
                if attempt.workflow_id != workflow_id
                    || attempt.task_key != candidate.candidate.task_key
                    || attempt.contract_id != candidate.candidate.contract_id
                    || attempt.lease_id != candidate.lease_id
                {
                    return Err(workflow_error("store_recovery_receipt_mismatch"));
                }
                all_candidates.insert(candidate.candidate.candidate_digest);
                if candidate.admitted {
                    let Some(manifest) = &candidate.manifest else {
                        // A claim without a manifest is not an authorized artifact.
                        continue;
                    };
                    if manifest.base_commit != base_commit
                        || manifest
                            .digest()
                            .map_err(|_| workflow_error("store_recovery_receipt_mismatch"))?
                            != candidate.candidate.candidate_digest
                    {
                        return Err(workflow_error("store_recovery_receipt_mismatch"));
                    }
                    admitted_candidates.insert(candidate.candidate.candidate_digest);
                }
            }
            WorkflowReceipt::Review(review) => review_subjects.push(review.candidate_digest),
            WorkflowReceipt::Verification(verifier) => {
                if verifier.verifier_files_digest != verifier_digest {
                    return Err(workflow_error("store_recovery_receipt_mismatch"));
                }
                if verifier.derived_subject.is_some() {
                    let workflow = workflow_rows::load_existing_workflow(transaction, workflow_id)?;
                    super::check_verification_subject(transaction, &workflow.record, verifier)?;
                    admitted_candidates.insert(verifier.subject_digest);
                    all_candidates.insert(verifier.subject_digest);
                }
                verifier_subjects.push(verifier.subject_digest);
                verifier_ids.push(verifier.receipt_id);
            }
            WorkflowReceipt::Integration(integration) => integrations.push(integration),
            WorkflowReceipt::Bundle(bundle) => {
                bundle_sources.push(bundle.snapshot_digest);
                bundle_content.insert(bundle.content_digest);
            }
            WorkflowReceipt::ModelCall(_) => {
                return Err(workflow_error("store_recovery_projection_unverified"));
            }
            WorkflowReceipt::Selection(selection) => selections.push(selection),
            WorkflowReceipt::Decision(_) => {}
        }
    }
    if review_subjects
        .iter()
        .any(|digest| !all_candidates.contains(digest))
    {
        return Err(workflow_error("store_recovery_receipt_mismatch"));
    }
    let mut artifacts = admitted_candidates.clone();
    for selection in selections {
        if selection
            .outcome
            .winner
            .is_some_and(|digest| !admitted_candidates.contains(&digest))
            || selection
                .outcome
                .ranking
                .iter()
                .any(|digest| !admitted_candidates.contains(digest))
            || selection
                .outcome
                .excluded
                .iter()
                .any(|excluded| !all_candidates.contains(&excluded.candidate_digest))
        {
            return Err(workflow_error("store_recovery_receipt_mismatch"));
        }
    }
    for integration in integrations {
        if integration.base_commit != base_commit
            || integration
                .applied_candidates
                .iter()
                .any(|digest| !admitted_candidates.contains(digest))
        {
            return Err(workflow_error("store_recovery_receipt_mismatch"));
        }
        artifacts.insert(integration.integration_tree_digest);
    }
    // A verifier subject not explained by an admitted candidate or integrated
    // tree may be a derived negative control. It needs an explicit derived
    // subject record before this writer can attest it.
    if verifier_subjects
        .iter()
        .any(|digest| !artifacts.contains(digest))
        || bundle_sources
            .iter()
            .any(|digest| !artifacts.contains(digest))
    {
        return Err(workflow_error("store_recovery_derived_subject_unverified"));
    }
    artifacts.extend(verifier_subjects);
    verifier_ids.sort();
    verifier_ids.dedup();
    Ok(ReceiptReferences {
        artifacts: artifacts.into_iter().collect(),
        verifiers: verifier_ids,
        bundle_content,
        sequence,
    })
}

/// Content-index rows do not become artifact references. Candidate file blobs
/// still require a manifest/file join; rendered prompts and transcripts are
/// validated metadata, not an implicit artifact source.
fn validate_content_metadata(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
    bundle_content: &BTreeSet<Sha256Digest>,
    content_enabled: bool,
) -> Result<u64, StoreError> {
    let mut statement = transaction.prepare(
        "SELECT content_sha256,kind,state,record_json,updated_sequence FROM content_store_entries WHERE workflow_id=?",
    )?;
    let rows = statement.query_map([uuid_key(workflow_id)], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, u64>(4)?,
        ))
    })?;
    let mut count = 0_usize;
    let mut newest_sequence = 0_u64;
    for row in rows {
        count += 1;
        if count > MAX_RECOVERY_REFERENCES {
            return Err(workflow_error("store_recovery_projection_too_large"));
        }
        if !content_enabled {
            return Err(workflow_error("store_recovery_content_unexpected"));
        }
        let (digest, kind, state, json, sequence) = row?;
        newest_sequence = newest_sequence.max(sequence);
        let entry: ContentEntry = serde_json::from_str(&json)
            .map_err(|_| workflow_error("store_recovery_content_mismatch"))?;
        if digest != entry.content_sha256.to_hex()
            || kind != entry.kind.as_str()
            || state != entry.state.as_str()
            || entry.workflow_id != Some(workflow_id)
        {
            return Err(workflow_error("store_recovery_content_mismatch"));
        }
        if entry.kind == ContentKind::CandidateFile
            || (entry.kind == ContentKind::Bundle
                && !bundle_content.contains(&entry.content_sha256))
        {
            return Err(workflow_error("store_recovery_projection_unverified"));
        }
    }
    Ok(newest_sequence)
}

/// Rebuild only fields for which this store has a complete authoritative
/// projection. Attempts without controller/submission ownership and messages
/// without durable participant cursors require a richer writer join and fail
/// closed. Known receipts and metadata-only content remain source checked.
fn project_recovery_state(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
) -> Result<RecoveryState, StoreError> {
    let workflow = workflow_rows::load_existing_workflow(transaction, workflow_id)?;
    let (binding, binding_sequence) = load_binding(transaction, workflow_id)?
        .ok_or_else(|| workflow_error("store_recovery_binding_unavailable"))?;
    if workflow.record.evidence_class != EvidenceClass::Fixture
        || binding.task_dependencies.len() != workflow.record.tasks.len()
        || binding.task_model_caps.len() != workflow.record.tasks.len()
    {
        return Err(workflow_error("store_recovery_projection_unverified"));
    }
    let key = uuid_key(workflow_id);
    let attempt_count: u64 = transaction.query_row(
        "SELECT COUNT(*) FROM workflow_attempts WHERE workflow_id=?",
        [&key],
        |row| row.get(0),
    )?;
    let message_count: u64 = transaction.query_row(
        "SELECT COUNT(*) FROM workflow_messages WHERE workflow_id=?",
        [&key],
        |row| row.get(0),
    )?;
    let receipt_count: u64 = transaction.query_row(
        "SELECT COUNT(*) FROM workflow_receipts WHERE workflow_id=?",
        [&key],
        |row| row.get(0),
    )?;
    let model_calls: u64 = transaction.query_row(
        "SELECT COUNT(*) FROM workflow_model_calls c JOIN workflow_model_call_owners o ON o.owner_key=c.owner_key WHERE o.workflow_id=?",
        [&key], |row| row.get(0),
    )?;
    let used_model_calls: u64 = transaction.query_row(
        "SELECT COALESCE(SUM(calls_used),0) FROM workflow_model_call_owners WHERE workflow_id=?",
        [&key],
        |row| row.get(0),
    )?;
    let orphan_effects: u64 = transaction.query_row(
        "SELECT COUNT(*) FROM workflow_controller_outbox o LEFT JOIN workflow_controllers c ON c.controller_key=o.controller_key WHERE c.controller_key IS NULL",
        [], |row| row.get(0),
    )?;
    if attempt_count != 0
        || message_count != 0
        || model_calls != 0
        || used_model_calls != 0
        || orphan_effects != 0
    {
        return Err(workflow_error("store_recovery_projection_unverified"));
    }
    if receipt_count > MAX_RECOVERY_REFERENCES as u64 {
        return Err(workflow_error("store_recovery_projection_too_large"));
    }
    let receipt_refs = receipt_references(
        transaction,
        workflow_id,
        workflow.record.verifier_digest,
        &workflow.record.base_commit,
    )?;
    let content_sequence = validate_content_metadata(
        transaction,
        workflow_id,
        &receipt_refs.bundle_content,
        matches!(
            workflow.record.content_store,
            vibemux_workflow::workflow_record::ContentStoreMode::Enabled { .. }
        ),
    )?;
    let mut contracts = Vec::with_capacity(workflow.record.tasks.len());
    let mut task_budgets = Vec::with_capacity(workflow.record.tasks.len());
    for task in &workflow.record.tasks {
        let contract = workflow_rows::load_contract(transaction, task.contract_id)?
            .ok_or_else(|| workflow_error("store_recovery_reference_mismatch"))?;
        if contract.task_key != task.task_key.as_str()
            || contract.workflow_key != workflow.record.workflow_key.as_str()
            || contract.contract_version != task.contract_version
            || contract.policy_digest != workflow.record.policy_digest
            || contract.base_commit != workflow.record.base_commit
        {
            return Err(workflow_error("store_recovery_reference_mismatch"));
        }
        let dependencies = binding
            .task_dependencies
            .get(&task.task_key)
            .ok_or_else(|| workflow_error("store_recovery_binding_mismatch"))?;
        let max_model_requests = *binding
            .task_model_caps
            .get(&task.task_key)
            .ok_or_else(|| workflow_error("store_recovery_binding_mismatch"))?;
        contracts.push(RecoveryContractRef {
            task_key: task.task_key.clone(),
            contract_id: task.contract_id,
            contract_version: task.contract_version,
            template_digest: contract.template_digest,
            depends_on: dependencies.clone(),
        });
        let model_requests_used =
            charged_model_requests_for(transaction, workflow_id, &task.task_key)?;
        task_budgets.push(RecoveryTaskBudget {
            task_key: task.task_key.clone(),
            turns_used: task.turns_used,
            max_turns: task.max_turns,
            repairs_used: task.repairs_used,
            max_repairs: task.max_repairs,
            model_requests_used,
            max_model_requests,
        });
    }
    contracts.sort_by(|left, right| left.task_key.cmp(&right.task_key));
    task_budgets.sort_by(|left, right| left.task_key.cmp(&right.task_key));
    let mut cursor = workflow
        .sequence
        .max(binding_sequence)
        .max(receipt_refs.sequence)
        .max(content_sequence);
    let lease_sequence: u64 = transaction.query_row(
        "SELECT COALESCE(MAX(updated_sequence),0) FROM workflow_leases WHERE workflow_id=?",
        [&key],
        |row| row.get(0),
    )?;
    cursor = cursor.max(lease_sequence);
    let mut leases = workflow_rows::leases_for(transaction, workflow_id)?
        .into_iter()
        .map(|lease| RecoveryLeaseRef {
            lease_id: lease.lease_id,
            task_key: lease.task_key,
            generation: lease.generation,
            state: lease.state,
        })
        .collect::<Vec<_>>();
    leases.sort_by_key(|lease| lease.lease_id);
    let mut statement = transaction.prepare(
        "SELECT controller_key FROM workflow_controllers WHERE workflow_id=? ORDER BY controller_key",
    )?;
    let keys = statement
        .query_map([&key], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut controllers = Vec::with_capacity(keys.len());
    for text in keys {
        let logical_key = Uuid::parse_str(&text)
            .map_err(|_| workflow_error("store_controller_projection_mismatch"))?;
        let stored = load_controller(transaction, logical_key)?
            .ok_or_else(|| workflow_error("store_controller_projection_mismatch"))?;
        cursor = cursor.max(stored.sequence);
        if stored.record.pending_approval.is_some() || stored.record.has_unsettled_input() {
            return Err(workflow_error("store_recovery_projection_unverified"));
        }
        controllers.push(RecoveryControllerRef {
            controller_id: stored.record.identity.controller_id,
            session_id: stored.record.identity.session_id,
            task_key: stored.task_key,
            generation: stored.record.generation,
            version: stored.record.version,
            owner: stored.record.owner,
            phase: stored.record.phase,
        });
    }
    controllers.sort_by_key(|controller| controller.controller_id);
    let mut statement = transaction.prepare(
        "SELECT o.effect_id,o.state,c.task_key FROM workflow_controller_outbox o JOIN workflow_controllers c ON c.controller_key=o.controller_key WHERE c.workflow_id=?",
    )?;
    let effects = statement.query_map([&key], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut unresolved = Vec::new();
    for effect in effects {
        let (id, state, task_key) = effect?;
        if state != ControllerOutboxState::Acknowledged.as_str() {
            unresolved.push(RecoveryEffectRef {
                effect_id: Uuid::parse_str(&id)
                    .map_err(|_| workflow_error("store_controller_outbox_mismatch"))?,
                task_key: Some(
                    SpecIdentifier::new(task_key)
                        .map_err(|_| workflow_error("store_controller_outbox_mismatch"))?,
                ),
            });
        }
    }
    unresolved.sort_by_key(|effect| effect.effect_id);
    let state = RecoveryState {
        workflow_id,
        workflow_version: workflow.record.version,
        workflow_phase: workflow.record.phase,
        plan_generation: binding.plan_generation,
        plan_digest: binding.plan_digest,
        policy_digest: workflow.record.policy_digest,
        committed_event_cursor: cursor,
        contracts,
        attempts: Vec::new(),
        leases,
        controllers,
        pending_approvals: Vec::new(),
        message_cursors: Vec::new(),
        artifact_digests: receipt_refs.artifacts,
        verifier_receipt_ids: receipt_refs.verifiers,
        unresolved_effects: unresolved,
        loop_budget: workflow.record.budget,
        task_budgets,
        context_settings: binding.context_settings,
        preapproved_resume_policy: binding.preapproved_resume_policy,
        next_action: RecoveryAction::Inspect,
    };
    state
        .validate()
        .map_err(|error| workflow_error(error.code()))?;
    Ok(state)
}

fn load_checkpoint_in(
    transaction: &Transaction<'_>,
    checkpoint_id: Uuid,
) -> Result<Option<RecoveryCheckpoint>, StoreError> {
    let row: Option<(String, String, String, String)> = transaction
        .query_row(
            "SELECT checkpoint_id,workflow_id,state_digest,record_json FROM workflow_checkpoints WHERE checkpoint_id=?",
            [uuid_key(checkpoint_id)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((id, workflow_id, digest, json)) = row else {
        return Ok(None);
    };
    let checkpoint =
        RecoveryCheckpoint::parse(json.as_bytes()).map_err(|error| workflow_error(error.code()))?;
    if id != uuid_key(checkpoint_id)
        || checkpoint.checkpoint_id != checkpoint_id
        || workflow_id != uuid_key(checkpoint.state.workflow_id)
        || digest != checkpoint.state_digest.to_hex()
    {
        return Err(workflow_error("store_recovery_checkpoint_mismatch"));
    }
    Ok(Some(checkpoint))
}

impl SqliteStore {
    /// A complete read-only projection of what this schema can currently
    /// prove. Unsupported nonempty reference sources return a fixed refusal.
    pub fn workflow_recovery_state(&self, workflow_id: Uuid) -> Result<RecoveryState, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let state = project_recovery_state(&transaction, workflow_id)?;
        transaction.commit()?;
        Ok(state)
    }

    pub fn save_recovery_checkpoint(
        &mut self,
        checkpoint: &RecoveryCheckpoint,
        timestamp: OffsetDateTime,
    ) -> Result<RecoveryCheckpointCommit, StoreError> {
        checkpoint
            .validate()
            .map_err(|error| workflow_error(error.code()))?;
        let transaction = self.immediate()?;
        let current = project_recovery_state(&transaction, checkpoint.state.workflow_id)?;
        if checkpoint.state != current {
            return Err(workflow_error("store_recovery_state_changed"));
        }
        if let Some(existing) = load_checkpoint_in(&transaction, checkpoint.checkpoint_id)? {
            if existing == *checkpoint {
                return Ok(RecoveryCheckpointCommit {
                    checkpoint: existing,
                    duplicate: true,
                    event: None,
                });
            }
            return Err(workflow_error("store_recovery_checkpoint_conflict"));
        }
        let key = format!(
            "workflow_recovery_checkpoint:{}",
            uuid_key(checkpoint.checkpoint_id)
        );
        let already_recorded: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE idempotency_key=?)",
            [&key],
            |row| row.get(0),
        )?;
        if already_recorded {
            return Err(workflow_error("store_recovery_checkpoint_conflict"));
        }
        let (event, sequence) = append(
            &transaction,
            RECOVERY_CHECKPOINT_EVENT,
            key,
            json!({
                "checkpoint_id":checkpoint.checkpoint_id,
                "workflow_id":checkpoint.state.workflow_id,
                "workflow_version":checkpoint.state.workflow_version,
                "state_digest":checkpoint.state_digest,
                "committed_event_cursor":checkpoint.state.committed_event_cursor,
                "next_action":"inspect",
            }),
            timestamp,
        )?;
        transaction.execute(
            "INSERT INTO workflow_checkpoints(checkpoint_id,workflow_id,state_digest,record_json,created_sequence) VALUES (?,?,?,?,?)",
            params![
                uuid_key(checkpoint.checkpoint_id),
                uuid_key(checkpoint.state.workflow_id),
                checkpoint.state_digest.to_hex(),
                serde_json::to_string(checkpoint)?,
                sequence,
            ],
        )?;
        transaction.commit()?;
        Ok(RecoveryCheckpointCommit {
            checkpoint: checkpoint.clone(),
            duplicate: false,
            event: Some(event),
        })
    }

    pub fn load_recovery_checkpoint(
        &self,
        checkpoint_id: Uuid,
    ) -> Result<Option<RecoveryCheckpoint>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let Some(checkpoint) = load_checkpoint_in(&transaction, checkpoint_id)? else {
            return Ok(None);
        };
        let current = project_recovery_state(&transaction, checkpoint.state.workflow_id)?;
        if current != checkpoint.state {
            return Err(workflow_error("store_recovery_state_changed"));
        }
        transaction.commit()?;
        Ok(Some(checkpoint))
    }

    /// Bind a complete, validated plan to its current admitted contracts.
    /// Generation changes require a separate explicit transition; this API
    /// cannot silently replace the first binding.
    pub fn bind_recovery_plan(
        &mut self,
        workflow_id: Uuid,
        full_plan: &Value,
        plan_generation: u64,
        context_settings: ContextSettingsIdentity,
        timestamp: OffsetDateTime,
    ) -> Result<RecoveryBindingCommit, StoreError> {
        if plan_generation == 0
            || context_settings.configured_context_tokens.is_some()
            || context_settings.compaction_settings_digest.is_some()
        {
            return Err(workflow_error("store_recovery_binding_unverified"));
        }
        let encoded = serde_json::to_vec(full_plan)?;
        if encoded.len() > MAX_TRANSIENT_PLAN_BYTES {
            return Err(workflow_error("store_recovery_plan_too_large"));
        }
        let plan: BoundPlan = serde_json::from_slice(&encoded)
            .map_err(|_| workflow_error("store_recovery_plan_invalid"))?;
        if plan.schema_version != 1
            || plan.policy.validate().is_err()
            || plan.specs.is_empty()
            || plan.assignments.len() != plan.specs.len()
            || plan.assignments.keys().ne(plan.specs.keys())
            || plan.assignments.values().any(Vec::is_empty)
        {
            return Err(workflow_error("store_recovery_plan_invalid"));
        }
        let plan_digest = canonical_digest(PLAN_DIGEST_DOMAIN, full_plan)
            .map_err(|_| workflow_error("store_recovery_plan_invalid"))?;
        let transaction = self.immediate()?;
        let workflow = workflow_rows::load_existing_workflow(&transaction, workflow_id)?;
        if plan
            .policy
            .digest()
            .map_err(|_| workflow_error("store_recovery_plan_invalid"))?
            != workflow.record.policy_digest
            || plan.specs.len() != workflow.record.tasks.len()
        {
            return Err(workflow_error("store_recovery_plan_mismatch"));
        }
        if workflow_rows::load_slot(&transaction, plan.reviewer_slot.as_str())?.is_none() {
            return Err(workflow_error("store_recovery_plan_mismatch"));
        }
        let mut task_dependencies = BTreeMap::new();
        let mut task_model_caps = BTreeMap::new();
        for (key, spec) in &plan.specs {
            let task = workflow
                .record
                .task(key)
                .ok_or_else(|| workflow_error("store_recovery_plan_mismatch"))?;
            let artifact = workflow_rows::load_contract(&transaction, task.contract_id)?
                .ok_or_else(|| workflow_error("store_recovery_plan_mismatch"))?;
            if key != &spec.task_key
                || spec.workflow_key != workflow.record.workflow_key
                || spec.base_commit != workflow.record.base_commit
                || spec.contract_version != task.contract_version
                || spec.policy_version != plan.policy.policy_version
                || spec.validate_structure().is_err()
                || spec.clone().normalized() != *spec
                || spec
                    .digest()
                    .map_err(|_| workflow_error("store_recovery_plan_invalid"))?
                    != artifact.task_spec_digest
                || artifact.contract_id != task.contract_id
                || artifact.contract_version != task.contract_version
                || artifact.task_key != key.as_str()
                || artifact.policy_digest != workflow.record.policy_digest
            {
                return Err(workflow_error("store_recovery_plan_mismatch"));
            }
            for slot in &plan.assignments[key] {
                if workflow_rows::load_slot(&transaction, slot.as_str())?.is_none() {
                    return Err(workflow_error("store_recovery_plan_mismatch"));
                }
            }
            task_dependencies.insert(key.clone(), spec.dependencies.clone());
            task_model_caps.insert(key.clone(), spec.resource_budget.max_model_requests);
        }
        let binding = RecoveryPlanBinding {
            workflow_id,
            plan_digest,
            plan_generation,
            context_settings,
            preapproved_resume_policy: None,
            task_dependencies,
            task_model_caps,
        };
        if let Some((stored, _)) = load_binding(&transaction, workflow_id)? {
            if stored == binding {
                return Ok(RecoveryBindingCommit {
                    binding: stored,
                    duplicate: true,
                    event: None,
                });
            }
            return Err(workflow_error("store_recovery_binding_conflict"));
        }
        if workflow.record.phase.is_terminal() {
            return Err(workflow_error("store_recovery_plan_mismatch"));
        }
        let (event, sequence) = append(
            &transaction,
            RECOVERY_BIND_EVENT,
            format!("workflow_recovery_plan:{}", uuid_key(workflow_id)),
            json!({
                "workflow_id":workflow_id,
                "plan_digest":plan_digest,
                "generation":plan_generation,
                "task_count":binding.task_dependencies.len(),
                "context_known":false,
            }),
            timestamp,
        )?;
        transaction.execute(
            "INSERT INTO workflow_recovery_bindings(workflow_id,record_json,created_sequence) VALUES (?,?,?)",
            params![uuid_key(workflow_id), serde_json::to_string(&binding)?, sequence],
        )?;
        transaction.commit()?;
        Ok(RecoveryBindingCommit {
            binding,
            duplicate: false,
            event: Some(event),
        })
    }

    /// Admits only an initial controller bound to one current task contract.
    pub fn admit_controller(
        &mut self,
        controller: &NativeController,
        timestamp: OffsetDateTime,
    ) -> Result<ControllerCommit, StoreError> {
        controller
            .validate()
            .map_err(|error| workflow_error(error.code()))?;
        if *controller
            != NativeController::new(controller.identity, controller.owner)
                .map_err(|error| workflow_error(error.code()))?
        {
            return Err(workflow_error("store_controller_invalid_admission"));
        }
        let transaction = self.immediate()?;
        let workflow =
            workflow_rows::load_existing_workflow(&transaction, controller.identity.workflow_id)?;
        if workflow.record.phase.is_terminal()
            || matches!(
                workflow.record.phase,
                vibemux_workflow::gates::WorkflowPhase::Blocked
                    | vibemux_workflow::gates::WorkflowPhase::CancelRequested
            )
        {
            return Err(workflow_error("store_controller_workflow_unavailable"));
        }
        let task_key = controller_task(controller, &workflow.record)?;
        validate_controller_binding(&transaction, controller, &workflow.record, &task_key)?;
        if let Some(existing) = load_controller(&transaction, controller.identity.controller_id)? {
            if existing.record.identity == controller.identity
                && existing.record.owner == controller.owner
                && existing.task_key == task_key
            {
                return Ok(ControllerCommit {
                    controller_key: existing.controller_key,
                    controller: existing.record,
                    duplicate: true,
                    event: None,
                    effect: None,
                });
            }
            return Err(workflow_error("store_controller_conflict"));
        }
        let key = format!(
            "workflow_controller_admitted:{}",
            uuid_key(controller.identity.controller_id)
        );
        let already_admitted: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE idempotency_key = ?)",
            [&key],
            |row| row.get(0),
        )?;
        if already_admitted {
            return Err(workflow_error("store_controller_replayed_identity"));
        }
        let (event, sequence) = append(
            &transaction,
            CONTROLLER_ADMISSION_EVENT,
            key,
            json!({
                "workflow_id":controller.identity.workflow_id,
                "controller_key":controller.identity.controller_id,
                "controller_id":controller.identity.controller_id,
                "session_id":controller.identity.session_id,
                "task_key":task_key,
                "contract_id":controller.identity.contract_id,
                "contract_version":controller.identity.contract_version,
                "generation":controller.generation,
                "version":controller.version,
                "owner":controller.owner,
            }),
            timestamp,
        )?;
        transaction.execute(
            "INSERT INTO workflow_controllers(controller_key,workflow_id,task_key,session_id,record_json,updated_sequence) VALUES (?,?,?,?,?,?)",
            params![
                uuid_key(controller.identity.controller_id),
                uuid_key(controller.identity.workflow_id),
                task_key.as_str(),
                uuid_key(controller.identity.session_id),
                serde_json::to_string(controller)?,
                sequence,
            ],
        )?;
        transaction.commit()?;
        Ok(ControllerCommit {
            controller_key: controller.identity.controller_id,
            controller: controller.clone(),
            duplicate: false,
            event: Some(event),
            effect: None,
        })
    }

    pub fn workflow_controller(
        &self,
        controller_key: Uuid,
    ) -> Result<Option<NativeController>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let record = load_controller(&transaction, controller_key)?.map(|stored| stored.record);
        transaction.commit()?;
        Ok(record)
    }

    /// Reads one exact submission intent. It never claims or replays it.
    pub fn controller_outbox(
        &self,
        effect_id: Uuid,
    ) -> Result<Option<ControllerOutboxRecord>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let effect = load_submission_outbox(&transaction, effect_id)?;
        transaction.commit()?;
        Ok(effect)
    }

    /// CAS transition plus content-free audit/outbox in one transaction.
    /// A duplicate consumes no new version, event, or outbox row.
    pub fn change_workflow_controller(
        &mut self,
        controller_key: Uuid,
        expected_version: u64,
        fence: ControllerFence,
        actor: ControllerActor,
        command: ControllerCommand,
        timestamp: OffsetDateTime,
    ) -> Result<ControllerCommit, StoreError> {
        if matches!(command, ControllerCommand::BeginSubmission { .. }) {
            return Err(workflow_error("store_controller_claim_required"));
        }
        let transaction = self.immediate()?;
        let stored = load_controller(&transaction, controller_key)?
            .ok_or_else(|| workflow_error("store_controller_not_found"))?;
        if matches!(
            command,
            ControllerCommand::Resume | ControllerCommand::ReserveSubmission(_)
        ) && !workflow_rows::load_existing_workflow(
            &transaction,
            stored.record.identity.workflow_id,
        )?
        .record
        .phase
        .admits_work()
        {
            return Err(workflow_error("store_controller_workflow_unavailable"));
        }
        let changed =
            apply_controller_command(&stored.record, expected_version, fence, actor, command)
                .map_err(|error| workflow_error(error.code()))?;
        if changed.duplicate {
            return Ok(ControllerCommit {
                controller_key,
                controller: stored.record,
                duplicate: true,
                event: None,
                effect: None,
            });
        }
        let next = changed.record;
        let reservation = match command {
            ControllerCommand::ReserveSubmission(reservation) => Some(reservation),
            _ => None,
        };
        let charge_native = reservation.is_some()
            && matches!(
                stored.record.owner,
                ControllerOwner::Native | ControllerOwner::Human { .. }
            );
        let mut charged_workflow = None;
        if charge_native {
            let current = workflow_rows::load_existing_workflow(
                &transaction,
                stored.record.identity.workflow_id,
            )?;
            let (binding, _) = load_binding(&transaction, current.record.workflow_id)?
                .ok_or_else(|| workflow_error("store_recovery_binding_unavailable"))?;
            let task_cap = *binding
                .task_model_caps
                .get(&stored.task_key)
                .ok_or_else(|| workflow_error("store_recovery_binding_mismatch"))?;
            let used = charged_model_requests_for(
                &transaction,
                current.record.workflow_id,
                &stored.task_key,
            )?;
            let mut next_workflow = current.record.clone();
            if !next_workflow.phase.admits_work()
                || next_workflow.budget.model_requests_remaining == 0
                || millis(timestamp) >= next_workflow.budget.deadline_unix_ms
                || used >= task_cap
            {
                return Err(workflow_error("store_controller_budget_exhausted"));
            }
            next_workflow
                .charge_turn(&stored.task_key, false)
                .map_err(|error| workflow_error(error.code()))?;
            next_workflow.budget.model_requests_remaining -= 1;
            next_workflow.version = next_workflow
                .version
                .checked_add(1)
                .ok_or_else(|| workflow_error("store_controller_budget_exhausted"))?;
            next_workflow.updated_at_ms = millis(timestamp).max(next_workflow.updated_at_ms);
            charged_workflow = Some((next_workflow, current.record.version));
        }
        let intent = effect_intent(command);
        let effect_id = (reservation.is_some() || intent.is_some()).then(Uuid::new_v4);
        let old_controller_id = stored.record.identity.controller_id;
        let (event, sequence) = append(
            &transaction,
            CONTROLLER_CHANGE_EVENT,
            format!(
                "workflow_controller_changed:{}:{}",
                uuid_key(controller_key),
                next.version
            ),
            json!({
                "workflow_id":next.identity.workflow_id,
                "controller_key":controller_key,
                "controller_id":old_controller_id,
                "next_controller_id":next.identity.controller_id,
                "task_key":stored.task_key,
                "from_version":stored.record.version,
                "version":next.version,
                "generation":next.generation,
                "phase":next.phase,
                "actor":actor_kind(actor),
                "operator_id":match actor { ControllerActor::Human { operator_id } => Some(operator_id), _ => None },
                "effect_id":effect_id,
                "effect_kind":if reservation.is_some() { Some("begin_submission") } else { intent.as_ref().map(|item| item.0) },
                "native_budget_charged":charge_native,
            }),
            timestamp,
        )?;
        update_controller_row(&transaction, &stored, &next, sequence)?;
        if let Some((workflow, previous_version)) = charged_workflow {
            workflow_rows::update_workflow(&transaction, &workflow, previous_version, sequence)?;
        }
        let outbox = if let (Some(reservation), Some(effect_id)) = (reservation, effect_id) {
            let record = ControllerOutboxRecord {
                effect_id,
                controller_key,
                controller_id: old_controller_id,
                controller_generation: next.generation,
                controller_version: next.version,
                kind: ControllerOutboxKind::BeginSubmission,
                submission_id: reservation.submission_id,
                charged_model_request: charge_native,
                prompt_digest: reservation.prompt_digest,
                prompt_bytes: reservation.prompt_bytes,
                state: ControllerOutboxState::Pending,
            };
            transaction.execute(
                "INSERT INTO workflow_controller_outbox(effect_id,controller_key,controller_version,command_json,state,created_sequence) VALUES (?,?,?,?,?,?)",
                params![
                    uuid_key(effect_id),
                    uuid_key(controller_key),
                    next.version,
                    serde_json::to_string(&record)?,
                    record.state.as_str(),
                    sequence,
                ],
            )?;
            Some(record)
        } else if let (Some((kind, command_json, state)), Some(effect_id)) = (intent, effect_id) {
            transaction.execute(
                "INSERT INTO workflow_controller_outbox(effect_id,controller_key,controller_version,command_json,state,created_sequence) VALUES (?,?,?,?,?,?)",
                params![
                    uuid_key(effect_id),
                    uuid_key(controller_key),
                    next.version,
                    serde_json::to_string(&command_json)?,
                    state,
                    sequence,
                ],
            )?;
            let _ = kind;
            None
        } else {
            None
        };
        transaction.commit()?;
        Ok(ControllerCommit {
            controller_key,
            controller: next,
            duplicate: false,
            event: Some(event),
            effect: outbox,
        })
    }

    /// Claiming is the sole path that changes Reserved to Submitted. The
    /// committed state is ambiguous after a crash, so a Submitted row cannot
    /// be claimed again and never enters an automatic replay queue.
    pub fn claim_controller_outbox(
        &mut self,
        effect_id: Uuid,
        expected_controller_version: u64,
        fence: ControllerFence,
        timestamp: OffsetDateTime,
    ) -> Result<ControllerOutboxCommit, StoreError> {
        let transaction = self.immediate()?;
        let outbox = load_submission_outbox(&transaction, effect_id)?
            .ok_or_else(|| workflow_error("store_controller_outbox_not_found"))?;
        if outbox.state != ControllerOutboxState::Pending {
            return Err(workflow_error("store_controller_outbox_not_claimable"));
        }
        let stored = load_controller(&transaction, outbox.controller_key)?
            .ok_or_else(|| workflow_error("store_controller_not_found"))?;
        if !workflow_rows::load_existing_workflow(&transaction, stored.record.identity.workflow_id)?
            .record
            .phase
            .admits_work()
        {
            return Err(workflow_error("store_controller_workflow_unavailable"));
        }
        if expected_controller_version != outbox.controller_version
            || stored.record.version != outbox.controller_version
            || fence != stored.record.fence()
            || outbox.controller_generation != stored.record.generation
            || outbox.controller_id != stored.record.identity.controller_id
        {
            return Err(workflow_error("store_controller_outbox_stale_fence"));
        }
        let reservation = stored
            .record
            .last_submission
            .ok_or_else(|| workflow_error("store_controller_outbox_mismatch"))?
            .reservation;
        if reservation.submission_id != outbox.submission_id
            || reservation.prompt_digest != outbox.prompt_digest
            || reservation.prompt_bytes != outbox.prompt_bytes
        {
            return Err(workflow_error("store_controller_outbox_mismatch"));
        }
        let next = apply_controller_command(
            &stored.record,
            expected_controller_version,
            fence,
            ControllerActor::Automation,
            ControllerCommand::BeginSubmission {
                submission_id: outbox.submission_id,
            },
        )
        .map_err(|error| workflow_error(error.code()))?
        .record;
        let mut next_outbox = outbox.clone();
        next_outbox.controller_version = next.version;
        next_outbox.state = ControllerOutboxState::Submitted;
        let (event, sequence) = append(
            &transaction,
            OUTBOX_CLAIM_EVENT,
            format!("workflow_controller_outbox_claim:{}", uuid_key(effect_id)),
            json!({
                "workflow_id":next.identity.workflow_id,
                "controller_key":outbox.controller_key,
                "controller_id":next.identity.controller_id,
                "controller_generation":next.generation,
                "controller_version":next.version,
                "effect_id":effect_id,
                "submission_id":outbox.submission_id,
                "prompt_digest":outbox.prompt_digest,
                "prompt_bytes":outbox.prompt_bytes,
                "state":next_outbox.state,
            }),
            timestamp,
        )?;
        update_controller_row(&transaction, &stored, &next, sequence)?;
        update_submission_outbox(&transaction, &outbox, &next_outbox)?;
        transaction.commit()?;
        Ok(ControllerOutboxCommit {
            controller_key: outbox.controller_key,
            controller: next,
            effect: next_outbox,
            duplicate: false,
            event: Some(event),
        })
    }

    /// Records only correlated adapter evidence. Unknown remains paused and
    /// cannot be turned into another submission by retrying the claim.
    pub fn finish_controller_outbox(
        &mut self,
        effect_id: Uuid,
        fence: ControllerFence,
        outcome: ControllerOutboxOutcome,
        timestamp: OffsetDateTime,
    ) -> Result<ControllerOutboxCommit, StoreError> {
        let transaction = self.immediate()?;
        let outbox = load_submission_outbox(&transaction, effect_id)?
            .ok_or_else(|| workflow_error("store_controller_outbox_not_found"))?;
        let stored = load_controller(&transaction, outbox.controller_key)?
            .ok_or_else(|| workflow_error("store_controller_not_found"))?;
        if fence != stored.record.fence()
            || stored.record.version != outbox.controller_version
            || outbox.controller_generation != stored.record.generation
            || outbox.controller_id != stored.record.identity.controller_id
        {
            return Err(workflow_error("store_controller_outbox_stale_fence"));
        }
        let target_state = match outcome {
            ControllerOutboxOutcome::Acknowledged => ControllerOutboxState::Acknowledged,
            ControllerOutboxOutcome::DeliveryUnknown => ControllerOutboxState::Unknown,
        };
        if outbox.state == target_state {
            return Ok(ControllerOutboxCommit {
                controller_key: outbox.controller_key,
                controller: stored.record,
                effect: outbox,
                duplicate: true,
                event: None,
            });
        }
        if outbox.state != ControllerOutboxState::Submitted
            && !(outbox.state == ControllerOutboxState::Unknown
                && target_state == ControllerOutboxState::Acknowledged)
        {
            return Err(workflow_error("store_controller_outbox_not_settleable"));
        }
        let command = match outcome {
            ControllerOutboxOutcome::Acknowledged => ControllerCommand::AcknowledgeSubmission {
                submission_id: outbox.submission_id,
            },
            ControllerOutboxOutcome::DeliveryUnknown => ControllerCommand::DeliveryUnknown {
                submission_id: outbox.submission_id,
            },
        };
        let next = apply_controller_command(
            &stored.record,
            stored.record.version,
            fence,
            ControllerActor::Adapter,
            command,
        )
        .map_err(|error| workflow_error(error.code()))?
        .record;
        let mut next_outbox = outbox.clone();
        next_outbox.controller_version = next.version;
        next_outbox.state = target_state;
        let (event, sequence) = append(
            &transaction,
            OUTBOX_FINISH_EVENT,
            format!(
                "workflow_controller_outbox_finish:{}:{}",
                uuid_key(effect_id),
                next.version
            ),
            json!({
                "workflow_id":next.identity.workflow_id,
                "controller_key":outbox.controller_key,
                "controller_id":next.identity.controller_id,
                "controller_version":next.version,
                "effect_id":effect_id,
                "submission_id":outbox.submission_id,
                "state":target_state,
            }),
            timestamp,
        )?;
        update_controller_row(&transaction, &stored, &next, sequence)?;
        update_submission_outbox(&transaction, &outbox, &next_outbox)?;
        transaction.commit()?;
        Ok(ControllerOutboxCommit {
            controller_key: outbox.controller_key,
            controller: next,
            effect: next_outbox,
            duplicate: false,
            event: Some(event),
        })
    }
}
