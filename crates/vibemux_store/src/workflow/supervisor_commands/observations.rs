//! Bounded committed facts for model observations and deterministic candidate selection.
use super::rows::{actions_for, load_execution, load_round};
use super::{millis, workflow_error, workflow_rows};
use crate::StoreError;
use rusqlite::{OptionalExtension, Transaction};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_workflow::{
    SpecIdentifier,
    gates::{
        CandidateAcceptance, CandidateRecord, GateFailure, GateRequirements, SuiteStatus,
        candidate_gate,
    },
    receipts::WorkflowReceipt,
    selection::{CandidateMetrics, ComparisonEntry, SelectionPolicy, select},
    slots::{
        EligibilityRequest, EvidenceLevel, InterfaceMode, SlotCapability, evaluate_eligibility,
    },
    supervisor::{SupervisorCandidate, SupervisorObservation},
    task_spec::WorkflowMode,
};
const WORKER_CAPABILITIES: [(SlotCapability, EvidenceLevel); 2] = [
    (
        SlotCapability::StructuredTurn,
        EvidenceLevel::WriteModeCertified,
    ),
    (
        SlotCapability::WritableWorktree,
        EvidenceLevel::WriteModeCertified,
    ),
];

pub(super) fn evidence_gate(
    record: &vibemux_workflow::workflow_record::WorkflowRecord,
    candidate: &CandidateRecord,
    receipts: &[WorkflowReceipt],
) -> Result<Result<CandidateAcceptance, Vec<GateFailure>>, StoreError> {
    let task = record
        .task(&candidate.task_key)
        .ok_or_else(|| workflow_error("store_supervisor_projection_mismatch"))?;
    if candidate.contract_id != task.contract_id {
        return Err(workflow_error("store_supervisor_projection_mismatch"));
    }
    let review = receipts.iter().rev().find_map(|receipt| match receipt {
        WorkflowReceipt::Review(review)
            if review.candidate_digest == candidate.candidate_digest
                && review.task_key == candidate.task_key =>
        {
            Some(review)
        }
        _ => None,
    });
    let verification = receipts.iter().rev().find_map(|receipt| match receipt {
        WorkflowReceipt::Verification(value)
            if value.subject_digest == candidate.candidate_digest
                && value.task_key.as_ref() == Some(&candidate.task_key)
                && value.derived_subject.is_none() =>
        {
            Some(value)
        }
        _ => None,
    });
    Ok(candidate_gate(
        candidate,
        review,
        verification,
        GateRequirements {
            required_suites: &task.required_suites,
            admitted_verifier_digest: record.verifier_digest,
        },
    ))
}

pub(super) fn observation_in(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
) -> Result<SupervisorObservation, StoreError> {
    let execution = load_execution(transaction, workflow_id)?
        .ok_or_else(|| workflow_error("store_supervisor_unknown"))?;
    let stored = workflow_rows::load_existing_workflow(transaction, workflow_id)?;
    let slots = workflow_rows::all_slots(transaction)?;
    let now_ms = millis(OffsetDateTime::now_utc());
    let eligibility = evaluate_eligibility(
        &slots,
        EligibilityRequest {
            required: &WORKER_CAPABILITIES,
            class: stored.record.evidence_class,
            interface_mode: InterfaceMode::Structured,
            now_ms,
            excluded_slots: &[],
            allow_unknown_quota: true,
        },
    );
    let eligible_slots = execution
        .tasks
        .iter()
        .map(|(key, binding)| {
            let allowed = binding
                .allowed_slots
                .iter()
                .filter(|slot| {
                    eligibility.eligible.contains(slot)
                        && slots.iter().any(|facts| {
                            &facts.slot_id == *slot
                                && execution.slot_harnesses.get(*slot) == Some(&facts.harness)
                        })
                })
                .cloned()
                .collect();
            (key.clone(), allowed)
        })
        .collect();
    let receipts = workflow_rows::receipts_for(transaction, workflow_id, None)?;
    let mut latest: BTreeMap<(SpecIdentifier, Uuid), _> = BTreeMap::new();
    for receipt in &receipts {
        if let WorkflowReceipt::Candidate(candidate) = receipt
            && candidate.admitted
        {
            latest.insert(
                (
                    candidate.candidate.task_key.clone(),
                    candidate.candidate.worker_session_id,
                ),
                candidate,
            );
        }
    }
    let candidates: Vec<_> = latest
        .into_values()
        .map(|candidate| {
            let digest = candidate.candidate.candidate_digest;
            let review = receipts.iter().rev().find_map(|receipt| match receipt {
                WorkflowReceipt::Review(review)
                    if review.candidate_digest == digest
                        && review.task_key == candidate.candidate.task_key =>
                {
                    Some(review)
                }
                _ => None,
            });
            let verification = receipts.iter().rev().find_map(|receipt| match receipt {
                WorkflowReceipt::Verification(verify)
                    if verify.subject_digest == digest
                        && verify.task_key.as_ref() == Some(&candidate.candidate.task_key)
                        && verify.derived_subject.is_none() =>
                {
                    Some(verify)
                }
                _ => None,
            });
            Ok(SupervisorCandidate {
                candidate: candidate.candidate.clone(),
                request_id: candidate.attempt_request_id,
                reviewed: review.is_some(),
                verified: verification.is_some(),
                gate_passed: evidence_gate(&stored.record, &candidate.candidate, &receipts)?
                    .is_ok(),
                tests_passed: verification.map_or(0, |value| {
                    value
                        .suites
                        .iter()
                        .filter(|suite| suite.status == SuiteStatus::Passed)
                        .map(|suite| suite.tests_passed)
                        .sum()
                }),
                changed_bytes: candidate
                    .manifest
                    .as_ref()
                    .map_or(0, |manifest| manifest.total_bytes()),
            })
        })
        .collect::<Result<_, StoreError>>()?;
    let selection = if stored.record.mode == WorkflowMode::Compare {
        let entries: Vec<_> = candidates
            .iter()
            .map(|candidate| {
                Ok(ComparisonEntry {
                    candidate: candidate.candidate.clone(),
                    gate: evidence_gate(&stored.record, &candidate.candidate, &receipts)?,
                    metrics: CandidateMetrics {
                        tests_passed: candidate.tests_passed,
                        model_requests: None,
                        elapsed_ms: None,
                        changed_bytes: candidate.changed_bytes,
                    },
                })
            })
            .collect::<Result<_, StoreError>>()?;
        stored
            .record
            .tasks
            .first()
            .map(|task| select(&entries, &SelectionPolicy::baseline(), task.contract_id))
    } else {
        None
    };
    let last_round_id:Option<String>=transaction.query_row("SELECT round_id FROM workflow_supervisor_rounds WHERE workflow_id=? ORDER BY rowid DESC LIMIT 1",[workflow_id.to_string()],|row|row.get(0)).optional()?;
    let last_round = last_round_id
        .map(|id| {
            load_round(
                transaction,
                Uuid::parse_str(&id)
                    .map_err(|_| workflow_error("store_supervisor_projection_mismatch"))?,
            )
        })
        .transpose()?;
    let last_actions = actions_for(transaction, workflow_id)?
        .into_iter()
        .rev()
        .take(8)
        .collect();
    let supervisor_cursor: u64 = transaction.query_row("SELECT COALESCE(MAX(sequence),0) FROM (SELECT updated_sequence AS sequence FROM workflow_supervisors WHERE workflow_id=?1 UNION ALL SELECT updated_sequence FROM workflow_supervisor_actions WHERE workflow_id=?1 UNION ALL SELECT updated_sequence FROM workflow_supervisor_rounds WHERE workflow_id=?1)", [workflow_id.to_string()], |row| row.get(0))?;
    Ok(SupervisorObservation {
        execution,
        workflow: stored.record,
        event_cursor: stored.sequence.max(supervisor_cursor),
        eligible_slots,
        candidates,
        selection,
        last_round,
        last_actions,
    })
}
