//! Receipt evidence required before a claimed model action can become applied.
use super::observations::observation_in;
use super::{workflow_error, workflow_rows};
use crate::StoreError;
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_workflow::{
    gates::WorkflowPhase,
    receipts::WorkflowReceipt,
    supervisor::{SupervisorAction, SupervisorExecution, SupervisorTaskGate, SupervisorTool},
    workflow_record::TaskProgress,
};

pub(super) fn validate_applied_result(
    transaction: &Transaction<'_>,
    execution: &SupervisorExecution,
    action: &SupervisorAction,
    refs: &[Uuid],
    timestamp: OffsetDateTime,
) -> Result<(), StoreError> {
    let invalid = || workflow_error("store_supervisor_result_mismatch");
    if super::context_actions::is_context_action(&action.action) {
        if refs.iter().collect::<BTreeSet<_>>().len() != refs.len() {
            return Err(invalid());
        }
        return super::context_actions::validate_result(
            transaction,
            execution,
            action,
            refs,
            timestamp,
        );
    }
    let workflow = workflow_rows::load_existing_workflow(transaction, action.workflow_id)?.record;
    let claim_sequence: u64 = transaction.query_row(
        "SELECT updated_sequence FROM workflow_supervisor_actions WHERE action_id=?",
        [action.action_id.to_string()],
        |row| row.get(0),
    )?;
    let mut receipts = Vec::new();
    let unique: BTreeSet<_> = refs.iter().collect();
    if unique.len() != refs.len() {
        return Err(invalid());
    }
    for id in refs {
        let row: Option<(String,u64)> = transaction.query_row("SELECT record_json,created_sequence FROM workflow_receipts WHERE workflow_id=? AND receipt_id=?", params![action.workflow_id.to_string(),id.to_string()], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
        let (json, sequence) = row.ok_or_else(invalid)?;
        let receipt: WorkflowReceipt = serde_json::from_str(&json)?;
        if sequence <= claim_sequence || receipt.receipt_id() != *id {
            return Err(invalid());
        }
        receipts.push(receipt);
    }
    match &action.action {
        SupervisorTool::AssignTask { task_key, .. }
        | SupervisorTool::RequestReview { task_key, .. } => {
            let request_id = action.dispatch_request_id.ok_or_else(invalid)?;
            let attempt =
                workflow_rows::load_attempt(transaction, request_id)?.ok_or_else(invalid)?;
            let lease =
                workflow_rows::load_lease(transaction, attempt.lease_id)?.ok_or_else(invalid)?;
            let expected_contract = execution
                .tasks
                .get(task_key)
                .ok_or_else(invalid)?
                .contract_id;
            if attempt.workflow_id != action.workflow_id
                || attempt.task_key != *task_key
                || attempt.contract_id != expected_contract
                || lease.workflow_id != action.workflow_id
                || lease.generation != attempt.lease_generation
                || workflow_rows::dispatch_phase(transaction, request_id)?.as_deref()
                    != Some("completed")
            {
                return Err(invalid());
            }
            let run_id =
                workflow_rows::dispatch_run_id(transaction, request_id)?.ok_or_else(invalid)?;
            let exact = match &action.action {
                SupervisorTool::AssignTask { slot_id, .. } => {
                    if &lease.slot_id != slot_id
                        || attempt.purpose == super::super::AttemptPurpose::Review
                    {
                        return Err(invalid());
                    }
                    matches!(receipts.as_slice(),[WorkflowReceipt::Candidate(candidate)]
                        if candidate.workflow_id==action.workflow_id && candidate.attempt_request_id==request_id
                        && candidate.lease_id==attempt.lease_id && candidate.candidate.lease_generation==attempt.lease_generation
                        && candidate.candidate.worker_session_id==attempt.session_id && candidate.candidate.worker_run_id.to_string()==run_id
                        && candidate.candidate.task_key==*task_key && candidate.candidate.contract_id==expected_contract)
                }
                SupervisorTool::RequestReview {
                    candidate_digest, ..
                } => {
                    if lease.slot_id != execution.reviewer_slot
                        || attempt.purpose != super::super::AttemptPurpose::Review
                    {
                        return Err(invalid());
                    }
                    matches!(receipts.as_slice(),[WorkflowReceipt::Review(review)]
                        if review.workflow_id==action.workflow_id && review.task_key==*task_key && review.contract_id==expected_contract
                        && review.candidate_digest==*candidate_digest && review.reviewer_session_id==attempt.session_id
                        && review.reviewer_run_id.to_string()==run_id && execution.slot_harnesses.get(&lease.slot_id)==Some(&review.reviewer_harness))
                }
                _ => false,
            };
            if !exact {
                return Err(invalid());
            }
        }
        SupervisorTool::RequestVerification {
            task_key,
            candidate_digest,
        } => {
            let contract = execution
                .tasks
                .get(task_key)
                .ok_or_else(invalid)?
                .contract_id;
            if !matches!(receipts.as_slice(),[WorkflowReceipt::Verification(verification)]
                if verification.workflow_id==action.workflow_id && verification.task_key.as_ref()==Some(task_key)
                && verification.subject_digest==*candidate_digest && verification.contract_ids==vec![contract]
                && verification.derived_subject.is_none() && verification.verifier_files_digest==workflow.verifier_digest)
            {
                return Err(invalid());
            }
        }
        SupervisorTool::ProposeCandidateSelection { candidate_digest } => {
            let observed = observation_in(transaction, action.workflow_id)?;
            let expected = observed.selection.ok_or_else(invalid)?;
            let contract = workflow.tasks.first().ok_or_else(invalid)?.contract_id;
            if expected.winner != *candidate_digest
                || !matches!(receipts.as_slice(),[WorkflowReceipt::Selection(selection)]
                if selection.workflow_id==action.workflow_id && selection.contract_id==contract && selection.outcome==expected)
            {
                return Err(invalid());
            }
        }
        SupervisorTool::ProposeIntegration { candidate_digests } => {
            let mut expected = candidate_digests.clone();
            expected.sort();
            let mut accepted: Vec<_> = workflow
                .tasks
                .iter()
                .filter_map(|task| task.accepted_candidate)
                .collect();
            accepted.sort();
            let mut contracts: Vec<_> =
                workflow.tasks.iter().map(|task| task.contract_id).collect();
            contracts.sort();
            contracts.dedup();
            if expected != accepted || expected.is_empty() || receipts.len() != 2 {
                return Err(invalid());
            }
            let integration = receipts
                .iter()
                .find_map(|receipt| match receipt {
                    WorkflowReceipt::Integration(value) => Some(value),
                    _ => None,
                })
                .ok_or_else(invalid)?;
            let verification = receipts
                .iter()
                .find_map(|receipt| match receipt {
                    WorkflowReceipt::Verification(value) => Some(value),
                    _ => None,
                })
                .ok_or_else(invalid)?;
            let mut applied = integration.applied_candidates.clone();
            applied.sort();
            let mut bound = integration.contract_ids.clone();
            bound.sort();
            bound.dedup();
            let mut verified = verification.contract_ids.clone();
            verified.sort();
            verified.dedup();
            if integration.workflow_id != action.workflow_id
                || integration.base_commit != workflow.base_commit
                || applied != expected
                || bound != contracts
                || verified != contracts
                || verification.workflow_id != action.workflow_id
                || verification.task_key.is_some()
                || verification.derived_subject.is_some()
                || verification.subject_digest != integration.integration_tree_digest
                || verification.verifier_files_digest != workflow.verifier_digest
            {
                return Err(invalid());
            }
        }
        SupervisorTool::ProposePlan { tasks } => {
            if !refs.is_empty() || execution.task_order != *tasks {
                return Err(invalid());
            }
        }
        SupervisorTool::PauseRun { task_key } => {
            if !refs.is_empty()
                || execution.task_gates.get(task_key) != Some(&SupervisorTaskGate::Paused)
            {
                return Err(invalid());
            }
        }
        SupervisorTool::RequestCancel {
            task_key: Some(task_key),
        } => {
            if !refs.is_empty()
                || execution.task_gates.get(task_key) != Some(&SupervisorTaskGate::Cancelled)
                || workflow
                    .task(task_key)
                    .is_none_or(|task| task.progress != TaskProgress::Failed)
            {
                return Err(invalid());
            }
        }
        SupervisorTool::RequestCancel { task_key: None } => {
            if !refs.is_empty()
                || !matches!(
                    workflow.phase,
                    WorkflowPhase::CancelRequested | WorkflowPhase::Cancelled
                )
            {
                return Err(invalid());
            }
        }
        SupervisorTool::ReportBlocked { reason_code } => {
            if !refs.is_empty()
                || workflow.phase != WorkflowPhase::Blocked
                || workflow.blocked_reason.as_deref() != Some(reason_code.as_str())
            {
                return Err(invalid());
            }
        }
        SupervisorTool::InspectWorkflow {} | SupervisorTool::ListSlots {} => {
            if !refs.is_empty() {
                return Err(invalid());
            }
        }
        _ => return Err(invalid()),
    }
    Ok(())
}
