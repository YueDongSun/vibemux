//! Worker steps, candidate receipts, independent review, and trusted
//! verification (ADR 031 §5, §6).
//!
//! A worker step holds one lease for one turn: acquire, heartbeat, run the
//! turn, collect the owned worktree twice, record the candidate receipt
//! under the lease fence, and settle. Review and verification never run in
//! the worker's worktree: the collected bytes are materialized into fresh
//! owned worktrees, re-collected before and after, and every receipt names
//! the exact candidate digest and contract it judged.

use std::{collections::BTreeMap, path::PathBuf};

use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{ContentKind, LeaseFence, LeaseRecord, MessageRecord};
use vibemux_types::a2a::RunWorkspace;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    canonical_json::canonical_digest,
    checkpoint::{CheckpointReport, ReviewFinding, parse_checkpoint, parse_review},
    gates::{
        CandidateOrigin, CandidateRecord, RECEIPT_SCHEMA_VERSION, ReviewReceipt, ReviewVerdict,
        VerifierReceipt,
    },
    receipts::{CandidateReceipt, WorkflowReceipt},
    renderer::TurnPurpose,
    slots::EvidenceClass,
    snapshot::{SnapshotChange, SnapshotEntry, SnapshotManifest},
    workflow_record::ContentStoreMode,
};

use super::{
    broker::{Delivery, acknowledge, store_content},
    collector::{Collection, CollectionScope, apply_manifests, collect},
    error::WorkflowError,
    runtime::{RunContext, TurnPlan, TurnResult, TurnSpec, Unit},
    turns::review_session_id,
    verifier_runner::run_suites,
};
use crate::harness_dispatch::call_writer;

const REFUSED_CANDIDATE_DOMAIN: &str = "vibemux.workflow.refused_candidate.v1";
const FINDINGS_DOMAIN: &str = "vibemux.workflow.review_findings.v1";
const MUTATED_AFTER_CHECKPOINT_CODE: &str = "snapshot_mutated_after_checkpoint";
const TURN_NOT_COMPLETED_CODE: &str = "dispatch_not_completed";
/// The trusted negative-control mutation (V04): the first written module
/// throws on import, so a candidate carrying it cannot pass the verifier.
pub const NEGATIVE_MUTATION_ID: &str = "prepend_throw_v1";
const NEGATIVE_MUTATION_PREFIX: &str =
    "throw new Error(\"vibemux injected negative-control mutation\");\n";

/// A recorded candidate of one unit and the text of its written files.
#[derive(Clone, Debug)]
pub(crate) struct CollectedCandidate {
    pub receipt: CandidateReceipt,
    pub texts: BTreeMap<String, String>,
}

impl CollectedCandidate {
    #[must_use]
    pub fn digest(&self) -> Sha256Digest {
        self.receipt.candidate.candidate_digest
    }

    #[must_use]
    pub fn manifest(&self) -> Option<&SnapshotManifest> {
        self.receipt
            .manifest
            .as_ref()
            .filter(|_| self.receipt.admitted)
    }
}

/// One worker unit and what it has produced so far.
#[derive(Debug)]
pub(crate) struct UnitState {
    pub unit: Unit,
    pub workspace: RunWorkspace,
    pub latest: Option<CollectedCandidate>,
}

/// What one worker step left behind.
#[derive(Debug)]
pub(crate) struct StepOutcome {
    pub turn: TurnResult,
    pub checkpoint: Result<CheckpointReport, String>,
    /// The candidate this step recorded, if any.
    pub recorded: Option<CollectedCandidate>,
    /// The messages delivered to this turn.
    pub delivered: Vec<MessageRecord>,
}

/// The scope of one task's candidates.
pub(crate) fn task_scope<'a>(
    context: &'a RunContext,
    task_key: &SpecIdentifier,
) -> Result<CollectionScope<'a>, WorkflowError> {
    let spec = context
        .plan
        .specs
        .get(task_key)
        .ok_or(WorkflowError::Internal)?;
    Ok(CollectionScope {
        owned: &spec.owned_paths,
        forbidden: &spec.forbidden_paths,
        protected: &context.plan.policy.protected_paths,
        limits: context.setup.config.config.candidate_limits,
    })
}

fn spec_digest(
    context: &RunContext,
    task_key: &SpecIdentifier,
) -> Result<Sha256Digest, WorkflowError> {
    context
        .plan
        .specs
        .get(task_key)
        .ok_or(WorkflowError::Internal)?
        .digest()
        .map_err(|_| WorkflowError::Internal)
}

/// What one worker step is asked to do.
#[derive(Debug)]
pub(crate) struct StepInput {
    pub purpose: TurnPurpose,
    pub notes: Vec<String>,
    /// Admitted messages (and their bundles) delivered at this turn
    /// boundary; acknowledged when the turn completes.
    pub deliveries: Vec<Delivery>,
}

/// Runs one worker turn under its own lease and records its candidate.
/// Answer turns record a candidate only when the worktree changed.
pub(crate) async fn worker_step(
    context: &RunContext,
    state: &mut UnitState,
    input: StepInput,
) -> Result<StepOutcome, WorkflowError> {
    // A pause or cancel stops automatic input at the turn boundary: no
    // new turn starts once either is signalled.
    if context.stopping() {
        return Err(WorkflowError::StopRequested);
    }
    let unit = state.unit.clone();
    let lease = context
        .acquire_lease(
            &unit.task_key,
            &unit.slot.slot_id,
            "worker",
            &PathBuf::from(&state.workspace.path),
        )
        .await?;
    let heartbeat = context.heartbeat(&lease);
    let result = step_under_lease(context, state, &lease, input).await;
    drop(heartbeat);
    let settled = context.settle_lease(&lease, "worker_step_settled").await;
    let outcome = result?;
    settled?;
    Ok(outcome)
}

async fn step_under_lease(
    context: &RunContext,
    state: &mut UnitState,
    lease: &LeaseRecord,
    input: StepInput,
) -> Result<StepOutcome, WorkflowError> {
    let unit = state.unit.clone();
    let purpose = input.purpose;
    let mut spec = TurnSpec {
        task_key: unit.task_key.clone(),
        contract_id: unit.contract_id,
        session_id: unit.session_id,
        harness: unit.slot.harness,
        lease: lease.clone(),
        working_directory: PathBuf::from(&state.workspace.path),
        purpose,
        notes: input.notes,
        context_blocks: Vec::new(),
        reviewed_candidate: None,
    };
    let identity = context.turn_identity(&spec).await?;
    // The prompt is fixed before admission. The writer binds this whole
    // batch to the admitted attempt before dispatch launches; an expired or
    // mismatched message cancels the attempt without a partial delivery.
    let planned_deliveries = input.deliveries;
    for delivery in &planned_deliveries {
        spec.notes.extend(delivery.note.clone());
        spec.context_blocks.extend(delivery.block.clone());
    }
    let plan = context.render_plan(spec, &identity)?;
    retain_prompt(context, &plan).await?;
    let turn = context.run_turn(&plan, &planned_deliveries).await?;
    let delivered = if turn.delivery_applied {
        planned_deliveries
    } else {
        Vec::new()
    };
    if turn.completed() {
        acknowledge(context, &unit.participant(), &delivered, turn.request_id).await?;
    }
    let delivered: Vec<MessageRecord> = delivered
        .into_iter()
        .map(|delivery| delivery.record)
        .collect();
    let spec_digest = spec_digest(context, &unit.task_key)?;
    let checkpoint = match &turn.final_text {
        Some(text) => parse_checkpoint(text, spec_digest).map_err(|error| error.code().to_string()),
        None => Err("report_missing".to_string()),
    };
    if !turn.completed() && context.stopping() {
        // A cancelled or interrupted turn is not a candidate.
        return Ok(StepOutcome {
            turn,
            checkpoint,
            recorded: None,
            delivered,
        });
    }
    let scope = task_scope(context, &unit.task_key)?;
    let blobs = context.setup.state.blobs();
    let first = collect(&context.manager, &state.workspace, scope, &blobs).await?;
    let second = collect(&context.manager, &state.workspace, scope, &blobs).await?;
    let changed = state
        .latest
        .as_ref()
        .is_none_or(|latest| Some(latest.digest()) != second.digest());
    if purpose == TurnPurpose::Answer && !changed {
        return Ok(StepOutcome {
            turn,
            checkpoint,
            recorded: None,
            delivered,
        });
    }
    let mutated = first.manifest != second.manifest;
    let receipt = candidate_receipt(context, &unit, lease, &turn, &second, mutated)?;
    let workflow_id = context.workflow_id;
    let writer = context.writer.clone();
    let fence = LeaseFence {
        lease_id: lease.lease_id,
        generation: lease.generation,
    };
    let recorded = WorkflowReceipt::Candidate(receipt.clone());
    call_writer(move || {
        writer.record_workflow_receipt(
            workflow_id,
            recorded.clone(),
            Some(fence),
            OffsetDateTime::now_utc(),
        )
    })
    .await?;
    let candidate = CollectedCandidate {
        receipt,
        texts: second.texts,
    };
    state.latest = Some(candidate.clone());
    Ok(StepOutcome {
        turn,
        checkpoint,
        recorded: Some(candidate),
        delivered,
    })
}

fn candidate_receipt(
    context: &RunContext,
    unit: &Unit,
    lease: &LeaseRecord,
    turn: &TurnResult,
    collection: &Collection,
    mutated: bool,
) -> Result<CandidateReceipt, WorkflowError> {
    let mut violation_codes = match &collection.manifest {
        Ok(_) => Vec::new(),
        Err(codes) => codes.clone(),
    };
    if !turn.completed() {
        violation_codes.push(TURN_NOT_COMPLETED_CODE.to_string());
    }
    if mutated {
        violation_codes.push(MUTATED_AFTER_CHECKPOINT_CODE.to_string());
    }
    violation_codes.sort();
    violation_codes.dedup();
    let admitted = violation_codes.is_empty();
    let manifest = collection.manifest.as_ref().ok().cloned();
    let candidate_digest = match (&manifest, admitted) {
        (Some(manifest), true) => manifest.digest().map_err(|_| WorkflowError::Snapshot)?,
        _ => Sha256Digest::of_fields(REFUSED_CANDIDATE_DOMAIN, &[turn.request_id.as_bytes()]),
    };
    let origin = match context.setup.config.config.evidence_class {
        EvidenceClass::Fixture => CandidateOrigin::FixtureWorker,
        EvidenceClass::Live => CandidateOrigin::LiveWorker,
    };
    Ok(CandidateReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        receipt_id: Uuid::new_v4(),
        workflow_id: context.workflow_id,
        attempt_request_id: turn.request_id,
        lease_id: lease.lease_id,
        candidate: CandidateRecord {
            candidate_digest,
            contract_id: unit.contract_id,
            task_key: unit.task_key.clone(),
            worker_run_id: turn.run_id,
            worker_session_id: unit.session_id,
            worker_route: context.route_label(&unit.slot),
            worker_harness: unit.slot.harness,
            lease_generation: lease.generation,
            origin,
        },
        manifest: if admitted { manifest } else { None },
        admitted,
        violation_codes,
        mutated_after_checkpoint: mutated,
    })
}

/// Materializes `manifests` into the owned worktree recorded under `key`
/// and collects it under `scope`.
pub(crate) async fn materialize(
    context: &RunContext,
    key: &str,
    manifests: &[SnapshotManifest],
    scope: CollectionScope<'_>,
) -> Result<(RunWorkspace, Collection), WorkflowError> {
    let workspace = context.workspace(key).await?;
    let target = PathBuf::from(&workspace.path);
    let owned = manifests.to_vec();
    let blobs = context.setup.state.blobs();
    tokio::task::spawn_blocking(move || {
        let references: Vec<&SnapshotManifest> = owned.iter().collect();
        apply_manifests(&target, &references, &blobs)
    })
    .await
    .map_err(|_| WorkflowError::Internal)??;
    let collection = collect(
        &context.manager,
        &workspace,
        scope,
        &context.setup.state.blobs(),
    )
    .await?;
    Ok((workspace, collection))
}

/// What one independent review left behind.
#[derive(Debug)]
pub(crate) struct ReviewOutcome {
    pub receipt: ReviewReceipt,
    pub findings: Vec<ReviewFinding>,
}

/// Reviews `candidate` in a fresh read-only session of the reviewer slot,
/// on a materialized copy of the collected bytes.
pub(crate) async fn review_candidate(
    context: &RunContext,
    unit: &Unit,
    candidate: &CollectedCandidate,
) -> Result<ReviewOutcome, WorkflowError> {
    let manifest = candidate.manifest().ok_or(WorkflowError::Snapshot)?.clone();
    let digest = candidate.digest();
    let scope = task_scope(context, &unit.task_key)?;
    let key = format!("review:{}:{}", unit.task_key, digest.to_hex());
    let (workspace, collection) = materialize(context, &key, &[manifest], scope).await?;
    if collection.digest() != Some(digest) {
        return Err(WorkflowError::Snapshot);
    }
    let reviewer_slot = context
        .setup
        .config
        .slot(&context.plan.reviewer_slot)
        .cloned()
        .ok_or(WorkflowError::SlotUnavailable)?;
    let _turn_guard = context.reviewer_turns.lock().await;
    if context.stopping() {
        return Err(WorkflowError::StopRequested);
    }
    let lease = context
        .acquire_lease(
            &unit.task_key,
            &reviewer_slot.slot_id,
            "reviewer",
            &PathBuf::from(&workspace.path),
        )
        .await?;
    let heartbeat = context.heartbeat(&lease);
    let session_id = review_session_id(context.workflow_id, lease.lease_id);
    let result = async {
        let plan = context
            .plan_turn(TurnSpec {
                task_key: unit.task_key.clone(),
                contract_id: unit.contract_id,
                session_id,
                harness: reviewer_slot.harness,
                lease: lease.clone(),
                working_directory: PathBuf::from(&workspace.path),
                purpose: TurnPurpose::Review,
                notes: Vec::new(),
                context_blocks: Vec::new(),
                reviewed_candidate: Some(digest),
            })
            .await?;
        retain_prompt(context, &plan).await?;
        context.run_turn(&plan, &[]).await
    }
    .await;
    drop(heartbeat);
    let settled = context.settle_lease(&lease, "review_settled").await;
    let turn = result?;
    settled?;
    let spec_digest = spec_digest(context, &unit.task_key)?;
    let report = match (&turn.final_text, turn.completed()) {
        (Some(text), true) => parse_review(text, spec_digest).map_err(|error| error.code()),
        _ => Err("report_missing"),
    };
    let (verdict, findings) = match report {
        Ok(report) => (report.verdict, report.findings),
        Err(_) => (ReviewVerdict::Blocked, Vec::new()),
    };
    let after = collect(
        &context.manager,
        &workspace,
        scope,
        &context.setup.state.blobs(),
    )
    .await?;
    let receipt = ReviewReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        receipt_id: Uuid::new_v4(),
        workflow_id: context.workflow_id,
        task_key: unit.task_key.clone(),
        contract_id: unit.contract_id,
        candidate_digest: digest,
        reviewer_run_id: turn.run_id,
        reviewer_session_id: session_id,
        reviewer_route: context.route_label(&reviewer_slot),
        reviewer_harness: reviewer_slot.harness,
        verdict,
        findings_count: u32::try_from(findings.len()).unwrap_or(u32::MAX),
        findings_digest: canonical_digest(FINDINGS_DOMAIN, &findings)
            .map_err(|_| WorkflowError::Internal)?,
        candidate_unchanged: after.digest() == Some(digest),
    };
    let writer = context.writer.clone();
    let workflow_id = context.workflow_id;
    let recorded = WorkflowReceipt::Review(receipt.clone());
    call_writer(move || {
        writer.record_workflow_receipt(
            workflow_id,
            recorded.clone(),
            None,
            OffsetDateTime::now_utc(),
        )
    })
    .await?;
    Ok(ReviewOutcome { receipt, findings })
}

/// What the trusted verifier ran on.
pub(crate) struct VerifySubject<'a> {
    pub workspace: &'a RunWorkspace,
    pub scope: CollectionScope<'a>,
    pub digest: Sha256Digest,
    pub task_key: Option<SpecIdentifier>,
    pub contract_ids: Vec<Sha256Digest>,
    pub suites: &'a [SpecIdentifier],
}

/// Runs the trusted suites on a materialized subject and re-collects it
/// afterwards. The receipt is returned, not recorded.
pub(crate) async fn verify_subject(
    context: &RunContext,
    subject: VerifySubject<'_>,
) -> Result<VerifierReceipt, WorkflowError> {
    let trampoline = context
        .setup
        .trampoline
        .clone()
        .ok_or(WorkflowError::Verifier)?;
    let scratch = context.setup.state.scratch(context.workflow_id);
    let suites = run_suites(
        &context.setup.config,
        &trampoline,
        subject.suites,
        &PathBuf::from(&subject.workspace.path),
        &scratch,
    )
    .await?;
    let after = collect(
        &context.manager,
        subject.workspace,
        subject.scope,
        &context.setup.state.blobs(),
    )
    .await?;
    Ok(VerifierReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        receipt_id: Uuid::new_v4(),
        workflow_id: context.workflow_id,
        task_key: subject.task_key,
        contract_ids: subject.contract_ids,
        subject_digest: subject.digest,
        suites: suites.results,
        verifier_files_digest: context.setup.config.verifier_digest,
        tool_versions: suites.tool_versions,
        subject_unchanged: after.digest() == Some(subject.digest),
    })
}

/// Verifies one collected candidate in its own fresh worktree and records
/// the receipt.
pub(crate) async fn verify_candidate(
    context: &RunContext,
    unit: &Unit,
    candidate: &CollectedCandidate,
) -> Result<VerifierReceipt, WorkflowError> {
    let manifest = candidate.manifest().ok_or(WorkflowError::Snapshot)?.clone();
    let digest = candidate.digest();
    let scope = task_scope(context, &unit.task_key)?;
    let key = format!("verify:{}:{}", unit.task_key, digest.to_hex());
    let (workspace, collection) = materialize(context, &key, &[manifest], scope).await?;
    if collection.digest() != Some(digest) {
        return Err(WorkflowError::Snapshot);
    }
    let suites = context
        .record()
        .await?
        .task(&unit.task_key)
        .ok_or(WorkflowError::Internal)?
        .required_suites
        .clone();
    let receipt = verify_subject(
        context,
        VerifySubject {
            workspace: &workspace,
            scope,
            digest,
            task_key: Some(unit.task_key.clone()),
            contract_ids: vec![unit.contract_id],
            suites: &suites,
        },
    )
    .await?;
    let writer = context.writer.clone();
    let workflow_id = context.workflow_id;
    let recorded = WorkflowReceipt::Verification(receipt.clone());
    call_writer(move || {
        writer.record_workflow_receipt(
            workflow_id,
            recorded.clone(),
            None,
            OffsetDateTime::now_utc(),
        )
    })
    .await?;
    Ok(receipt)
}

/// The trusted negative control of a comparison (V04): a separately
/// identified mutation of a collected candidate, verified in its own
/// worktree. It is never recorded as a candidate and never promotable.
pub(crate) struct NegativeControl {
    pub record: CandidateRecord,
    pub verification: VerifierReceipt,
    pub changed_bytes: u64,
}

pub(crate) async fn negative_control(
    context: &RunContext,
    unit: &Unit,
    source: &CollectedCandidate,
) -> Result<Option<NegativeControl>, WorkflowError> {
    let Some(manifest) = source.manifest() else {
        return Ok(None);
    };
    let blobs = context.setup.state.blobs();
    let mut entries = manifest.entries.clone();
    let Some(target) = entries.iter_mut().find(|entry| {
        entry.path.ends_with(".mjs") && matches!(entry.change, SnapshotChange::Write { .. })
    }) else {
        return Ok(None);
    };
    let SnapshotChange::Write { sha256, .. } = target.change else {
        return Ok(None);
    };
    let original = {
        let blobs = blobs.clone();
        tokio::task::spawn_blocking(move || blobs.get(sha256))
            .await
            .map_err(|_| WorkflowError::Internal)??
    };
    let mut mutated = NEGATIVE_MUTATION_PREFIX.as_bytes().to_vec();
    mutated.extend_from_slice(&original);
    let byte_count = mutated.len() as u64;
    let mutated_sha = {
        let blobs = blobs.clone();
        tokio::task::spawn_blocking(move || blobs.put(&mutated))
            .await
            .map_err(|_| WorkflowError::Internal)??
    };
    *target = SnapshotEntry {
        path: target.path.clone(),
        change: SnapshotChange::Write {
            sha256: mutated_sha,
            byte_count,
        },
    };
    let mutated_manifest = SnapshotManifest {
        schema_version: manifest.schema_version,
        base_commit: manifest.base_commit.clone(),
        entries,
    };
    let digest = mutated_manifest
        .digest()
        .map_err(|_| WorkflowError::Snapshot)?;
    let scope = task_scope(context, &unit.task_key)?;
    let key = format!("negative:{}:{}", unit.task_key, digest.to_hex());
    let changed_bytes = mutated_manifest.total_bytes();
    let (workspace, collection) = materialize(context, &key, &[mutated_manifest], scope).await?;
    if collection.digest() != Some(digest) {
        return Err(WorkflowError::Snapshot);
    }
    let suites = context
        .record()
        .await?
        .task(&unit.task_key)
        .ok_or(WorkflowError::Internal)?
        .required_suites
        .clone();
    let verification = verify_subject(
        context,
        VerifySubject {
            workspace: &workspace,
            scope,
            digest,
            task_key: Some(unit.task_key.clone()),
            contract_ids: vec![unit.contract_id],
            suites: &suites,
        },
    )
    .await?;
    let mut record = source.receipt.candidate.clone();
    record.candidate_digest = digest;
    record.origin = CandidateOrigin::InjectedMutation {
        source_candidate: source.digest(),
        mutation_id: NEGATIVE_MUTATION_ID.to_string(),
    };
    Ok(Some(NegativeControl {
        record,
        verification,
        changed_bytes,
    }))
}

/// Keeps the exact rendered prompt in the opt-in content store, so an
/// operator can inspect what a session was sent. Off by default.
async fn retain_prompt(context: &RunContext, plan: &TurnPlan) -> Result<(), WorkflowError> {
    let record = context.record().await?;
    if let ContentStoreMode::Enabled { retention_days } = record.content_store {
        store_content(
            context,
            plan.prompt.clone().into_bytes(),
            ContentKind::RenderedPrompt,
            None,
            retention_days,
        )
        .await?;
    }
    Ok(())
}
