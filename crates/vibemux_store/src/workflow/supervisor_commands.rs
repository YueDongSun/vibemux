//! Atomic model-supervisor ownership, observation-bound decisions, and effect intents.

mod context_actions;
mod context_reads;
mod observations;
mod results;
mod rows;

use super::{append, millis, model_calls, workflow_error, workflow_rows};
use crate::{SqliteStore, StoreError};
use observations::observation_in;
use results::validate_applied_result;
use rows::{
    actions_for, active_execution, load_action, load_execution, load_round, save_action,
    save_execution, save_round, unresolved_round,
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::AgentKind;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    canonical_json::canonical_digest,
    gates::WorkflowPhase,
    gateway::RouteRole,
    model_call::{MAX_MODEL_CALLS_PER_OWNER, ModelCallAdmission, ModelCallState},
    policy::OperatorPolicy,
    supervisor::{
        MAX_PROPOSAL_BYTES, SupervisorAction, SupervisorActionState, SupervisorDecision,
        SupervisorExecution, SupervisorExecutionState, SupervisorMessageBinding, SupervisorMode,
        SupervisorObservation, SupervisorProposal, SupervisorRound, SupervisorRoundState,
        SupervisorStatus, SupervisorTaskBinding, SupervisorTaskGate, SupervisorTool,
        validate_runtime_proposal,
    },
    task_spec::TaskSpec,
    workflow_record::TaskProgress,
};

const PLAN_DOMAIN: &str = "vibemux.workflow.plan.v1";
const MAX_PLAN_BYTES: usize = 4 * 1024 * 1024;
const MAX_STATUS_ITEMS: usize = 512;

pub(crate) const MIGRATION_V8: &str = r#"
CREATE TABLE workflow_supervisors (
    workflow_id TEXT PRIMARY KEY REFERENCES workflows(workflow_id),
    generation INTEGER NOT NULL CHECK(generation > 0),
    state TEXT NOT NULL CHECK(state IN ('running','stopped','recovery_required')),
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE TABLE workflow_supervisor_rounds (
    round_id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id),
    generation INTEGER NOT NULL,
    call_id TEXT NOT NULL UNIQUE REFERENCES workflow_model_calls(call_id),
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE TABLE workflow_supervisor_actions (
    action_id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id),
    generation INTEGER NOT NULL,
    round_id TEXT NOT NULL REFERENCES workflow_supervisor_rounds(round_id),
    idempotency_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','claimed','applied','blocked','unknown')),
    dispatch_request_id TEXT UNIQUE,
    record_json TEXT NOT NULL,
    updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
    UNIQUE(workflow_id, idempotency_key)
);
INSERT INTO schema_migrations(version, applied_at)
VALUES (8, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
"#;

#[derive(Clone)]
pub struct SupervisorStart {
    pub workflow_id: Uuid,
    pub start_request_id: Uuid,
    pub expected_version: u64,
    pub mode: SupervisorMode,
    pub full_plan: Value,
    pub config_digest: Sha256Digest,
    pub slot_harnesses: BTreeMap<SpecIdentifier, AgentKind>,
    pub timestamp: OffsetDateTime,
}

#[derive(Clone)]
pub struct SupervisorRoundAdmission {
    pub workflow_id: Uuid,
    pub generation: u64,
    pub observed_version: u64,
    pub observation_digest: Sha256Digest,
    pub context_digest: Sha256Digest,
    pub model_call: ModelCallAdmission,
    pub timestamp: OffsetDateTime,
}

#[derive(Clone, Debug)]
pub struct SupervisorProposalCommit {
    pub round: SupervisorRound,
    pub actions: Vec<SupervisorAction>,
    pub accepted: bool,
    pub duplicate: bool,
}

#[derive(Clone, Debug)]
pub struct SupervisorActionFinish {
    pub action_id: Uuid,
    pub generation: u64,
    pub state: SupervisorActionState,
    pub result_refs: Vec<Uuid>,
    pub result_code: Option<SpecIdentifier>,
    pub timestamp: OffsetDateTime,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundPlan {
    schema_version: u32,
    #[serde(rename = "request_sha256")]
    _request_sha256: Sha256Digest,
    policy: OperatorPolicy,
    specs: BTreeMap<SpecIdentifier, TaskSpec>,
    assignments: BTreeMap<SpecIdentifier, Vec<SpecIdentifier>>,
    reviewer_slot: SpecIdentifier,
    #[serde(rename = "template_overrides")]
    _template_overrides: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    negative_control: bool,
}

fn action_fingerprint(
    execution: &SupervisorExecution,
    decision: &SupervisorDecision,
) -> Result<Sha256Digest, StoreError> {
    canonical_digest("vibemux.workflow.supervisor_action.v1", &json!({
        "decision":decision,"workflow_id":execution.workflow_id,"generation":execution.generation,
        "plan_digest":execution.plan_digest,"config_digest":execution.config_digest,"tasks":execution.tasks,
        "slot_harnesses":execution.slot_harnesses,"reviewer_slot":execution.reviewer_slot,
    })).map_err(|_| workflow_error("supervisor_proposal_malformed"))
}

impl SqliteStore {
    pub fn begin_supervisor_execution(
        &mut self,
        start: &SupervisorStart,
    ) -> Result<SupervisorExecution, StoreError> {
        if start.start_request_id.is_nil() {
            return Err(workflow_error("store_supervisor_start_invalid"));
        }
        let bytes = serde_json::to_vec(&start.full_plan)?;
        if bytes.len() > MAX_PLAN_BYTES {
            return Err(workflow_error("store_supervisor_plan_invalid"));
        }
        let plan: BoundPlan = serde_json::from_slice(&bytes)
            .map_err(|_| workflow_error("store_supervisor_plan_invalid"))?;
        if plan.schema_version != 1
            || plan.policy.validate().is_err()
            || plan.specs.is_empty()
            || plan.assignments.keys().ne(plan.specs.keys())
        {
            return Err(workflow_error("store_supervisor_plan_invalid"));
        }
        if start.mode == SupervisorMode::Model && plan.negative_control {
            return Err(workflow_error("supervisor_negative_control_unavailable"));
        }
        let plan_digest = canonical_digest(PLAN_DOMAIN, &start.full_plan)
            .map_err(|_| workflow_error("store_supervisor_plan_invalid"))?;
        let transaction = self.immediate()?;
        let stored = workflow_rows::load_existing_workflow(&transaction, start.workflow_id)?;
        let binding_json: Option<String> = transaction
            .query_row(
                "SELECT record_json FROM workflow_recovery_bindings WHERE workflow_id=?",
                [start.workflow_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let binding: super::RecoveryPlanBinding = serde_json::from_str(
            &binding_json.ok_or_else(|| workflow_error("store_supervisor_plan_unbound"))?,
        )?;
        if binding.workflow_id != start.workflow_id || binding.plan_digest != plan_digest {
            return Err(workflow_error("store_supervisor_plan_changed"));
        }
        if stored.record.version != start.expected_version || !stored.record.phase.admits_work() {
            return Err(workflow_error("store_workflow_version_conflict"));
        }
        let previous = load_execution(&transaction, start.workflow_id)?;
        if let Some(existing) = &previous {
            if existing.state == SupervisorExecutionState::Running {
                if existing.start_request_id == start.start_request_id
                    && existing.mode == start.mode
                    && existing.plan_digest == plan_digest
                    && existing.config_digest == start.config_digest
                    && existing.slot_harnesses == start.slot_harnesses
                {
                    return Ok(existing.clone());
                }
                return Err(workflow_error("store_supervisor_already_running"));
            }
            if existing.state == SupervisorExecutionState::RecoveryRequired
                || unresolved_round(&transaction, start.workflow_id)?
            {
                return Err(workflow_error("store_supervisor_effect_unresolved"));
            }
            if actions_for(&transaction, start.workflow_id)?
                .iter()
                .any(|action| {
                    matches!(
                        action.state,
                        SupervisorActionState::Claimed | SupervisorActionState::Unknown
                    )
                })
            {
                return Err(workflow_error("store_supervisor_effect_unresolved"));
            }
            if existing.plan_digest != plan_digest || existing.config_digest != start.config_digest
            {
                return Err(workflow_error("store_supervisor_plan_changed"));
            }
        }
        for (slot_id, harness) in &start.slot_harnesses {
            if workflow_rows::load_slot(&transaction, slot_id.as_str())?
                .is_none_or(|(facts, _)| facts.harness != *harness)
            {
                return Err(workflow_error("store_supervisor_plan_invalid"));
            }
        }
        if !start.slot_harnesses.contains_key(&plan.reviewer_slot) {
            return Err(workflow_error("store_supervisor_plan_invalid"));
        }
        if plan
            .policy
            .digest()
            .map_err(|_| workflow_error("store_supervisor_plan_invalid"))?
            != stored.record.policy_digest
            || plan.specs.len() != stored.record.tasks.len()
            || workflow_rows::load_slot(&transaction, plan.reviewer_slot.as_str())?.is_none()
        {
            return Err(workflow_error("store_supervisor_plan_invalid"));
        }
        let mut tasks = BTreeMap::new();
        for (key, spec) in &plan.specs {
            let task = stored
                .record
                .task(key)
                .ok_or_else(|| workflow_error("store_supervisor_plan_invalid"))?;
            let contract = workflow_rows::load_contract(&transaction, task.contract_id)?
                .ok_or_else(|| workflow_error("store_supervisor_plan_invalid"))?;
            if spec.task_key != *key
                || spec.workflow_key != stored.record.workflow_key
                || spec.base_commit != stored.record.base_commit
                || spec.contract_version != task.contract_version
                || spec.validate_structure().is_err()
                || spec.clone().normalized() != *spec
                || spec
                    .digest()
                    .map_err(|_| workflow_error("store_supervisor_plan_invalid"))?
                    != contract.task_spec_digest
                || plan.assignments[key].is_empty()
                || spec
                    .dependencies
                    .iter()
                    .any(|dependency| !plan.specs.contains_key(dependency))
            {
                return Err(workflow_error("store_supervisor_plan_invalid"));
            }
            for slot in &plan.assignments[key] {
                let facts = workflow_rows::load_slot(&transaction, slot.as_str())?
                    .ok_or_else(|| workflow_error("store_supervisor_plan_invalid"))?
                    .0;
                if start.slot_harnesses.get(slot) != Some(&facts.harness) {
                    return Err(workflow_error("store_supervisor_plan_invalid"));
                }
            }
            tasks.insert(
                key.clone(),
                SupervisorTaskBinding {
                    contract_id: task.contract_id,
                    contract_version: task.contract_version,
                    allowed_slots: plan.assignments[key].clone(),
                    dependencies: spec.dependencies.clone(),
                },
            );
        }
        let generation = previous
            .as_ref()
            .map_or(0, |value| value.generation)
            .checked_add(1)
            .ok_or_else(|| workflow_error("store_supervisor_generation_exhausted"))?;
        let task_gates = tasks
            .keys()
            .map(|key| {
                (
                    key.clone(),
                    if previous.as_ref().is_some_and(|value| {
                        value.task_gates.get(key) == Some(&SupervisorTaskGate::Cancelled)
                    }) {
                        SupervisorTaskGate::Cancelled
                    } else {
                        SupervisorTaskGate::Active
                    },
                )
            })
            .collect();
        let record = SupervisorExecution {
            workflow_id: start.workflow_id,
            generation,
            start_request_id: start.start_request_id,
            mode: start.mode,
            state: SupervisorExecutionState::Running,
            plan_digest,
            config_digest: start.config_digest,
            task_order: previous.as_ref().map_or_else(
                || tasks.keys().cloned().collect(),
                |value| value.task_order.clone(),
            ),
            tasks,
            slot_harnesses: start.slot_harnesses.clone(),
            reviewer_slot: plan.reviewer_slot,
            task_gates,
            next_round: previous.as_ref().map_or(0, |value| value.next_round),
            max_model_calls: previous
                .as_ref()
                .map_or(MAX_MODEL_CALLS_PER_OWNER, |value| value.max_model_calls),
            deadline_unix_ms: stored.record.budget.deadline_unix_ms,
            active_round: None,
        };
        let (_, sequence) = append(
            &transaction,
            "workflow_supervisor_started",
            format!("supervisor_start:{}:{generation}", start.workflow_id),
            json!({"workflow_id":start.workflow_id,"generation":generation,"mode":start.mode,"plan_digest":plan_digest}),
            start.timestamp,
        )?;
        save_execution(&transaction, &record, sequence)?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn supervisor_observation(
        &mut self,
        workflow_id: Uuid,
    ) -> Result<SupervisorObservation, StoreError> {
        let transaction = self.immediate()?;
        let observation = observation_in(&transaction, workflow_id)?;
        transaction.commit()?;
        Ok(observation)
    }

    pub fn reserve_supervisor_round(
        &mut self,
        admission: &SupervisorRoundAdmission,
    ) -> Result<SupervisorRound, StoreError> {
        let transaction = self.immediate()?;
        let mut execution =
            active_execution(&transaction, admission.workflow_id, admission.generation)?;
        if execution.active_round.is_some()
            || actions_for(&transaction, admission.workflow_id)?
                .iter()
                .any(|action| {
                    matches!(
                        action.state,
                        SupervisorActionState::Pending
                            | SupervisorActionState::Claimed
                            | SupervisorActionState::Unknown
                    )
                })
        {
            return Err(workflow_error("store_supervisor_busy"));
        }
        let observation = observation_in(&transaction, admission.workflow_id)?;
        if observation.workflow.version != admission.observed_version
            || observation
                .digest()
                .map_err(|_| workflow_error("store_supervisor_context_invalid"))?
                != admission.observation_digest
        {
            return Err(workflow_error("store_supervisor_stale_observation"));
        }
        let expected_owner = format!("supervisor_{}", admission.workflow_id.simple());
        if admission.model_call.workflow_id != Some(admission.workflow_id)
            || admission.model_call.identity.role != RouteRole::Supervisor
            || admission.model_call.owner_key.as_str() != expected_owner
            || admission.model_call.identity.config_sha256 != execution.config_digest
            || admission.model_call.max_calls != execution.max_model_calls
            || admission.model_call.deadline_unix_ms != execution.deadline_unix_ms
            || observation.workflow.budget.decisions_remaining == 0
        {
            return Err(workflow_error("store_supervisor_call_invalid"));
        }
        let call = model_calls::reserve_model_call_in(&transaction, &admission.model_call)?;
        if call.state != ModelCallState::Reserved {
            return Err(workflow_error("store_supervisor_call_not_replayable"));
        }
        let current = workflow_rows::load_existing_workflow(&transaction, admission.workflow_id)?;
        if admission.observed_version.checked_add(1) != Some(current.record.version) {
            return Err(workflow_error(
                "store_supervisor_reservation_version_invalid",
            ));
        }
        let round = SupervisorRound {
            round_id: call.call_id,
            workflow_id: admission.workflow_id,
            generation: admission.generation,
            call_id: call.call_id,
            observed_version: admission.observed_version,
            reservation_version: current.record.version,
            observation_digest: admission.observation_digest,
            target_digest: observation
                .target_digest()
                .map_err(|_| workflow_error("store_supervisor_context_invalid"))?,
            context_digest: admission.context_digest,
            state: SupervisorRoundState::Reserved,
            proposal_digest: None,
            rationale_digest: None,
            rejection_codes: vec![],
        };
        execution.active_round = Some(round.round_id);
        execution.next_round = execution
            .next_round
            .checked_add(1)
            .ok_or_else(|| workflow_error("store_supervisor_generation_exhausted"))?;
        let (_, sequence) = append(
            &transaction,
            "workflow_supervisor_round_reserved",
            format!("supervisor_round:{}", round.round_id),
            json!({"workflow_id":round.workflow_id,"round_id":round.round_id,"generation":round.generation,"observed_version":round.observed_version,"reservation_version":round.reservation_version,"context_digest":round.context_digest}),
            admission.timestamp,
        )?;
        save_round(&transaction, &round, sequence)?;
        save_execution(&transaction, &execution, sequence)?;
        transaction.commit()?;
        Ok(round)
    }

    pub fn commit_supervisor_proposal(
        &mut self,
        round_id: Uuid,
        proposal: &SupervisorProposal,
        timestamp: OffsetDateTime,
    ) -> Result<SupervisorProposalCommit, StoreError> {
        if serde_json::to_vec(proposal)?.len() > MAX_PROPOSAL_BYTES {
            return Err(workflow_error("supervisor_proposal_too_large"));
        }
        let digest = canonical_digest("vibemux.workflow.supervisor_proposal.v1", proposal)
            .map_err(|_| workflow_error("supervisor_proposal_malformed"))?;
        let transaction = self.immediate()?;
        let mut round = load_round(&transaction, round_id)?;
        if round.state != SupervisorRoundState::Reserved {
            if round.proposal_digest != Some(digest) {
                return Err(workflow_error("store_supervisor_proposal_conflict"));
            }
            let actions = actions_for(&transaction, round.workflow_id)?
                .into_iter()
                .filter(|action| action.round_id == round_id)
                .collect();
            let accepted = round.state == SupervisorRoundState::Accepted;
            return Ok(SupervisorProposalCommit {
                round,
                actions,
                accepted,
                duplicate: true,
            });
        }
        let mut execution = active_execution(&transaction, round.workflow_id, round.generation)?;
        if execution.active_round != Some(round_id) {
            return Err(workflow_error("store_supervisor_stale_round"));
        }
        let call = model_calls::load_call(&transaction, round.call_id)?
            .ok_or_else(|| workflow_error("model_call_unknown"))?;
        if call.state != ModelCallState::Completed
            || call.admission.identity.role != RouteRole::Supervisor
            || call.admission.workflow_id != Some(round.workflow_id)
        {
            return Err(workflow_error("store_supervisor_response_unverified"));
        }
        let mut observation = observation_in(&transaction, round.workflow_id)?;
        let mut codes = Vec::new();
        if observation.workflow.version != round.reservation_version {
            codes.push("supervisor_stale_version".to_string());
        }
        observation.workflow.version = round.observed_version;
        observation.workflow.budget.model_requests_remaining = observation
            .workflow
            .budget
            .model_requests_remaining
            .checked_add(1)
            .ok_or_else(|| workflow_error("store_supervisor_context_invalid"))?;
        if observation
            .target_digest()
            .map_err(|_| workflow_error("store_supervisor_context_invalid"))?
            != round.target_digest
        {
            codes.push("supervisor_stale_observation".into());
        }
        if let Err(rejections) = validate_runtime_proposal(proposal, &observation) {
            codes.extend(rejections);
        }
        codes.extend(context_actions::validate_proposal_sources(
            &transaction,
            &execution,
            proposal,
        )?);
        let stored = workflow_rows::load_existing_workflow(&transaction, round.workflow_id)?;
        let mut workflow = stored.record.clone();
        let count = u32::try_from(proposal.decisions.len())
            .map_err(|_| workflow_error("supervisor_proposal_too_large"))?;
        if count == 0 || count > workflow.budget.decisions_remaining {
            codes.push("supervisor_budget_exhausted".to_string());
        }
        let mut existing_actions = Vec::new();
        for decision in &proposal.decisions {
            let fingerprint = action_fingerprint(&execution, decision)?;
            let existing:Option<String>=transaction.query_row("SELECT action_id FROM workflow_supervisor_actions WHERE workflow_id=? AND idempotency_key=?",params![round.workflow_id.to_string(),decision.idempotency_key],|row| row.get(0)).optional()?;
            if let Some(id) = existing {
                let action = load_action(
                    &transaction,
                    Uuid::parse_str(&id)
                        .map_err(|_| workflow_error("store_supervisor_projection_mismatch"))?,
                )?;
                if action.fingerprint != fingerprint || action.round_id != round_id {
                    codes.push("supervisor_idempotency_conflict".into());
                }
                existing_actions.push(action);
            }
        }
        if !existing_actions.is_empty() && existing_actions.len() != proposal.decisions.len() {
            codes.push("supervisor_mixed_replay_batch".into());
        }
        codes.sort();
        codes.dedup();
        let accepted = codes.is_empty();
        if !accepted {
            workflow.budget.malformed_repairs_remaining = workflow
                .budget
                .malformed_repairs_remaining
                .saturating_sub(1);
        }
        round.state = if accepted {
            SupervisorRoundState::Accepted
        } else {
            SupervisorRoundState::Rejected
        };
        round.proposal_digest = Some(digest);
        round.rationale_digest = Some(Sha256Digest::of(proposal.rationale.as_bytes()));
        round.rejection_codes = codes;
        execution.active_round = None;
        workflow.budget.decisions_remaining = workflow
            .budget
            .decisions_remaining
            .saturating_sub(count.max(1));
        workflow.version = workflow
            .version
            .checked_add(1)
            .ok_or_else(|| workflow_error("workflow_record_invalid"))?;
        workflow.updated_at_ms = millis(timestamp).max(workflow.updated_at_ms);
        let (_, sequence) = append(
            &transaction,
            "workflow_supervisor_proposal_validated",
            format!("supervisor_proposal:{round_id}"),
            json!({"workflow_id":round.workflow_id,"round_id":round_id,"accepted":accepted,"proposal_digest":digest,"rationale_digest":round.rationale_digest,"rejection_codes":round.rejection_codes,"decision_count":count}),
            timestamp,
        )?;
        let mut actions = Vec::new();
        if accepted && actions.is_empty() {
            for decision in &proposal.decisions {
                let fingerprint = action_fingerprint(&execution, decision)?;
                let action_id = Uuid::new_v4();
                let dispatch_request_id = matches!(
                    decision.action,
                    SupervisorTool::AssignTask { .. } | SupervisorTool::RequestReview { .. }
                )
                .then_some(action_id);
                let action = SupervisorAction {
                    action_id,
                    workflow_id: round.workflow_id,
                    generation: round.generation,
                    round_id,
                    idempotency_key: decision.idempotency_key.clone(),
                    fingerprint,
                    action: match &decision.action {
                        SupervisorTool::SendMessage { to, .. } => SupervisorTool::SendMessage {
                            to: to.clone(),
                            text: String::new(),
                        },
                        other => other.clone(),
                    },
                    message_body: match &decision.action {
                        SupervisorTool::SendMessage { text, .. } => {
                            Some(SupervisorMessageBinding::for_text(text))
                        }
                        _ => None,
                    },
                    state: SupervisorActionState::Pending,
                    dispatch_request_id,
                    result_refs: vec![],
                    result_code: None,
                };
                transaction.execute("INSERT INTO workflow_supervisor_actions(action_id,workflow_id,generation,round_id,idempotency_key,fingerprint,state,dispatch_request_id,record_json,updated_sequence) VALUES (?,?,?,?,?,?,'pending',?,?,?)",params![action_id.to_string(),round.workflow_id.to_string(),round.generation,round_id.to_string(),action.idempotency_key,action.fingerprint.to_hex(),action.dispatch_request_id.map(|id|id.to_string()),serde_json::to_string(&action)?,sequence])?;
                actions.push(action);
            }
        }
        workflow_rows::update_workflow(&transaction, &workflow, stored.record.version, sequence)?;
        save_round(&transaction, &round, sequence)?;
        save_execution(&transaction, &execution, sequence)?;
        transaction.commit()?;
        Ok(SupervisorProposalCommit {
            round,
            actions,
            accepted,
            duplicate: false,
        })
    }

    pub fn reject_supervisor_round(
        &mut self,
        round_id: Uuid,
        reason: &str,
        malformed: bool,
        timestamp: OffsetDateTime,
    ) -> Result<SupervisorRound, StoreError> {
        let reason = SpecIdentifier::new(reason)
            .map_err(|_| workflow_error("store_supervisor_reason_invalid"))?;
        let transaction = self.immediate()?;
        let mut round = load_round(&transaction, round_id)?;
        if round.state != SupervisorRoundState::Reserved {
            return Ok(round);
        }
        let mut execution = active_execution(&transaction, round.workflow_id, round.generation)?;
        let stored = workflow_rows::load_existing_workflow(&transaction, round.workflow_id)?;
        let mut workflow = stored.record.clone();
        if malformed {
            workflow.budget.malformed_repairs_remaining = workflow
                .budget
                .malformed_repairs_remaining
                .saturating_sub(1);
        }
        round.state = if malformed {
            SupervisorRoundState::Rejected
        } else {
            SupervisorRoundState::Unknown
        };
        round.rejection_codes = vec![reason.to_string()];
        execution.active_round = None;
        if !malformed {
            execution.state = SupervisorExecutionState::RecoveryRequired;
        }
        let (_, sequence) = append(
            &transaction,
            "workflow_supervisor_round_rejected",
            format!("supervisor_round_rejected:{round_id}"),
            json!({"workflow_id":round.workflow_id,"round_id":round_id,"reason_code":reason,"malformed":malformed}),
            timestamp,
        )?;
        workflow.version = workflow
            .version
            .checked_add(1)
            .ok_or_else(|| workflow_error("workflow_record_invalid"))?;
        workflow_rows::update_workflow(&transaction, &workflow, stored.record.version, sequence)?;
        save_round(&transaction, &round, sequence)?;
        save_execution(&transaction, &execution, sequence)?;
        transaction.commit()?;
        Ok(round)
    }

    pub fn claim_supervisor_action(
        &mut self,
        action_id: Uuid,
        generation: u64,
        timestamp: OffsetDateTime,
    ) -> Result<SupervisorAction, StoreError> {
        let transaction = self.immediate()?;
        let mut action = load_action(&transaction, action_id)?;
        let mut execution = active_execution(&transaction, action.workflow_id, generation)?;
        if action.generation != generation || action.state != SupervisorActionState::Pending {
            return Err(workflow_error("store_supervisor_action_not_pending"));
        }
        let stored = workflow_rows::load_existing_workflow(&transaction, action.workflow_id)?;
        if !stored.record.phase.admits_work() {
            return Err(workflow_error("store_workflow_not_running"));
        }
        let mut workflow = stored.record.clone();
        match &action.action {
            SupervisorTool::ProposePlan { tasks } => execution.task_order = tasks.clone(),
            SupervisorTool::PauseRun { task_key } => {
                execution
                    .task_gates
                    .insert(task_key.clone(), SupervisorTaskGate::Paused);
            }
            SupervisorTool::RequestCancel {
                task_key: Some(task_key),
            } => {
                execution
                    .task_gates
                    .insert(task_key.clone(), SupervisorTaskGate::Cancelled);
                let task = workflow
                    .task_mut(task_key)
                    .map_err(|_| workflow_error("workflow_unknown_task"))?;
                task.progress = TaskProgress::Failed;
                task.failure_code = Some("supervisor_task_cancelled".into());
            }
            SupervisorTool::RequestCancel { task_key: None } => {
                if !workflow
                    .phase
                    .can_transition_to(WorkflowPhase::CancelRequested)
                {
                    return Err(workflow_error("workflow_invalid_transition"));
                }
                workflow.phase = WorkflowPhase::CancelRequested;
            }
            SupervisorTool::ReportBlocked { reason_code } => {
                if !workflow.phase.can_transition_to(WorkflowPhase::Blocked) {
                    return Err(workflow_error("workflow_invalid_transition"));
                }
                workflow.phase = WorkflowPhase::Blocked;
                workflow.blocked_reason = Some(reason_code.to_string());
            }
            SupervisorTool::AssignTask { task_key, slot_id } => {
                let observation = observation_in(&transaction, action.workflow_id)?;
                if execution.task_gates.get(task_key) != Some(&SupervisorTaskGate::Active)
                    || !observation
                        .eligible_slots
                        .get(task_key)
                        .is_some_and(|slots| slots.contains(slot_id))
                {
                    return Err(workflow_error("supervisor_ineligible_slot"));
                }
                let task = workflow
                    .task(task_key)
                    .ok_or_else(|| workflow_error("workflow_unknown_task"))?;
                if matches!(task.progress, TaskProgress::Accepted | TaskProgress::Failed)
                    || task.turns_used >= task.max_turns
                {
                    return Err(workflow_error("supervisor_task_not_active"));
                }
                if execution.tasks.get(task_key).is_some_and(|binding| {
                    binding.dependencies.iter().any(|dependency| {
                        workflow
                            .task(dependency)
                            .is_none_or(|task| task.accepted_candidate.is_none())
                    })
                }) {
                    return Err(workflow_error("supervisor_dependencies_unready"));
                }
            }
            SupervisorTool::RequestReview {
                task_key,
                candidate_digest,
            }
            | SupervisorTool::RequestVerification {
                task_key,
                candidate_digest,
            } => {
                if execution.task_gates.get(task_key) != Some(&SupervisorTaskGate::Active)
                    || !observation_in(&transaction, action.workflow_id)?
                        .candidates
                        .iter()
                        .any(|value| {
                            &value.candidate.task_key == task_key
                                && &value.candidate.candidate_digest == candidate_digest
                        })
                {
                    return Err(workflow_error("supervisor_candidate_not_accepted"));
                }
            }
            SupervisorTool::ProposeIntegration { .. }
            | SupervisorTool::ProposeCandidateSelection { .. } => {
                let observation = observation_in(&transaction, action.workflow_id)?;
                let proposal = SupervisorProposal {
                    decisions: vec![SupervisorDecision {
                        idempotency_key: action.idempotency_key.clone(),
                        expected_workflow_version: workflow.version,
                        action: action.action.clone(),
                    }],
                    rationale: String::new(),
                    evidence_refs: vec![],
                };
                if validate_runtime_proposal(&proposal, &observation).is_err() {
                    return Err(workflow_error("supervisor_target_changed"));
                }
            }
            _ => {}
        }
        action.state = SupervisorActionState::Claimed;
        let (_, sequence) = append(
            &transaction,
            "workflow_supervisor_action_claimed",
            format!("supervisor_action_claimed:{action_id}"),
            json!({"workflow_id":action.workflow_id,"action_id":action_id,"generation":generation,"tool":action.action.name(),"dispatch_request_id":action.dispatch_request_id}),
            timestamp,
        )?;
        workflow.version = workflow
            .version
            .checked_add(1)
            .ok_or_else(|| workflow_error("workflow_record_invalid"))?;
        workflow_rows::update_workflow(&transaction, &workflow, stored.record.version, sequence)?;
        save_action(&transaction, &action, sequence)?;
        save_execution(&transaction, &execution, sequence)?;
        transaction.commit()?;
        Ok(action)
    }

    pub fn finish_supervisor_action(
        &mut self,
        finish: &SupervisorActionFinish,
    ) -> Result<SupervisorAction, StoreError> {
        if !matches!(
            finish.state,
            SupervisorActionState::Applied
                | SupervisorActionState::Blocked
                | SupervisorActionState::Unknown
        ) || finish.result_refs.len() > 16
            || finish.result_refs.iter().any(Uuid::is_nil)
        {
            return Err(workflow_error("store_supervisor_finish_invalid"));
        }
        let transaction = self.immediate()?;
        let mut action = load_action(&transaction, finish.action_id)?;
        let execution = load_execution(&transaction, action.workflow_id)?
            .ok_or_else(|| workflow_error("store_supervisor_unknown"))?;
        if action.generation != finish.generation || execution.generation != finish.generation {
            return Err(workflow_error("store_supervisor_stale_generation"));
        }
        if action.state != SupervisorActionState::Claimed {
            if action.state == finish.state
                && action.result_refs == finish.result_refs
                && action.result_code == finish.result_code
            {
                return Ok(action);
            }
            return Err(workflow_error("store_supervisor_finish_conflict"));
        }
        for id in &finish.result_refs {
            if context_actions::is_context_action(&action.action) {
                break;
            }
            let exists:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM workflow_receipts WHERE workflow_id=? AND receipt_id=?)",params![action.workflow_id.to_string(),id.to_string()],|row|row.get(0))?;
            if !exists {
                return Err(workflow_error("store_supervisor_result_unknown"));
            }
        }
        if finish.state == SupervisorActionState::Applied {
            validate_applied_result(
                &transaction,
                &execution,
                &action,
                &finish.result_refs,
                finish.timestamp,
            )?;
        }
        action.state = finish.state;
        action.result_refs = finish.result_refs.clone();
        action.result_code = finish.result_code.clone();
        let (_, sequence) = append(
            &transaction,
            "workflow_supervisor_action_finished",
            format!("supervisor_action_finished:{}", action.action_id),
            json!({"workflow_id":action.workflow_id,"action_id":action.action_id,"state":action.state,"result_refs":action.result_refs,"result_code":action.result_code}),
            finish.timestamp,
        )?;
        save_action(&transaction, &action, sequence)?;
        transaction.commit()?;
        Ok(action)
    }

    pub fn supervisor_status(
        &self,
        workflow_id: Uuid,
    ) -> Result<Option<SupervisorStatus>, StoreError> {
        let Some(execution) = load_execution(&self.connection, workflow_id)? else {
            return Ok(None);
        };
        let mut query=self.connection.prepare("SELECT round_id FROM workflow_supervisor_rounds WHERE workflow_id=? ORDER BY rowid LIMIT ?")?;
        let ids = query
            .query_map(
                params![workflow_id.to_string(), MAX_STATUS_ITEMS as u32 + 1],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if ids.len() > MAX_STATUS_ITEMS {
            return Err(workflow_error("store_supervisor_status_too_large"));
        }
        let rounds = ids
            .into_iter()
            .map(|id| {
                load_round(
                    &self.connection,
                    Uuid::parse_str(&id)
                        .map_err(|_| workflow_error("store_supervisor_projection_mismatch"))?,
                )
            })
            .collect::<Result<_, _>>()?;
        Ok(Some(SupervisorStatus {
            schema_version: 1,
            execution,
            rounds,
            actions: actions_for(&self.connection, workflow_id)?,
        }))
    }

    pub fn end_supervisor_execution(
        &mut self,
        workflow_id: Uuid,
        generation: u64,
        timestamp: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let transaction = self.immediate()?;
        let Some(mut execution) = load_execution(&transaction, workflow_id)? else {
            return Ok(());
        };
        if execution.generation != generation {
            return Err(workflow_error("store_supervisor_stale_generation"));
        }
        if execution.state != SupervisorExecutionState::Running && execution.active_round.is_none()
        {
            let unresolved = unresolved_round(&transaction, workflow_id)?
                || actions_for(&transaction, workflow_id)?
                    .iter()
                    .any(|action| {
                        matches!(
                            action.state,
                            SupervisorActionState::Claimed | SupervisorActionState::Unknown
                        )
                    });
            if !unresolved || execution.state == SupervisorExecutionState::RecoveryRequired {
                return Ok(());
            }
        }
        execution.state = SupervisorExecutionState::Stopped;
        for mut action in actions_for(&transaction, workflow_id)?
            .into_iter()
            .filter(|action| {
                action.generation == generation
                    && matches!(
                        action.state,
                        SupervisorActionState::Pending | SupervisorActionState::Claimed
                    )
            })
        {
            action.state = if action.state == SupervisorActionState::Claimed {
                execution.state = SupervisorExecutionState::RecoveryRequired;
                SupervisorActionState::Unknown
            } else {
                SupervisorActionState::Blocked
            };
            action.result_code = Some(
                SpecIdentifier::new("supervisor_execution_ended")
                    .map_err(|_| workflow_error("store_supervisor_invalid"))?,
            );
            let (_, sequence) = append(
                &transaction,
                "workflow_supervisor_action_interrupted",
                format!("supervisor_interrupted:{}", action.action_id),
                json!({"workflow_id":workflow_id,"action_id":action.action_id,"state":action.state}),
                timestamp,
            )?;
            save_action(&transaction, &action, sequence)?;
        }
        if let Some(round_id) = execution.active_round.take() {
            let mut round = load_round(&transaction, round_id)?;
            round.state = SupervisorRoundState::Unknown;
            round.rejection_codes = vec!["supervisor_response_not_committed".into()];
            let (_, sequence) = append(
                &transaction,
                "workflow_supervisor_round_interrupted",
                format!("supervisor_round_interrupted:{round_id}"),
                json!({"workflow_id":workflow_id,"round_id":round_id}),
                timestamp,
            )?;
            save_round(&transaction, &round, sequence)?;
            execution.state = SupervisorExecutionState::RecoveryRequired;
        }
        if unresolved_round(&transaction, workflow_id)?
            || actions_for(&transaction, workflow_id)?
                .iter()
                .any(|action| {
                    matches!(
                        action.state,
                        SupervisorActionState::Claimed | SupervisorActionState::Unknown
                    )
                })
        {
            execution.state = SupervisorExecutionState::RecoveryRequired;
        }
        let (_, sequence) = append(
            &transaction,
            "workflow_supervisor_ended",
            format!("supervisor_ended:{workflow_id}:{generation}"),
            json!({"workflow_id":workflow_id,"generation":generation,"state":execution.state}),
            timestamp,
        )?;
        save_execution(&transaction, &execution, sequence)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn recover_supervisors(&mut self, timestamp: OffsetDateTime) -> Result<u32, StoreError> {
        let mut after = String::new();
        let mut count = 0_u32;
        loop {
            let ids: Vec<(String, u64)> = {
                let mut query=self.connection.prepare("SELECT workflow_id,generation FROM workflow_supervisors WHERE state='running' AND workflow_id>? ORDER BY workflow_id LIMIT 128")?;
                query
                    .query_map([&after], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<_, _>>()?
            };
            if ids.is_empty() {
                break;
            }
            for (id, generation) in ids {
                self.end_supervisor_execution(
                    Uuid::parse_str(&id)
                        .map_err(|_| workflow_error("store_supervisor_projection_mismatch"))?,
                    generation,
                    timestamp,
                )?;
                after = id;
                count = count
                    .checked_add(1)
                    .ok_or_else(|| workflow_error("store_supervisor_status_too_large"))?;
            }
        }
        Ok(count)
    }
}

/// A task gate and claimed action bind a model driver's child admission. The
/// deterministic driver has no action binding and keeps its established path.
pub(super) fn check_dispatch(
    transaction: &Transaction<'_>,
    admission: &super::WorkflowAttemptAdmission,
) -> Result<(), StoreError> {
    let Some(execution) = load_execution(transaction, admission.workflow_id)? else {
        return Ok(());
    };
    if execution.mode != SupervisorMode::Model {
        return Ok(());
    }
    if execution.state != SupervisorExecutionState::Running
        || execution.task_gates.get(&admission.task_key) != Some(&SupervisorTaskGate::Active)
    {
        return Err(workflow_error("store_supervisor_task_not_active"));
    }
    let action_id:Option<String>=transaction.query_row("SELECT action_id FROM workflow_supervisor_actions WHERE workflow_id=? AND dispatch_request_id=?",params![admission.workflow_id.to_string(),admission.dispatch.request_id.to_string()],|row|row.get(0)).optional()?;
    let action = load_action(
        transaction,
        Uuid::parse_str(
            &action_id.ok_or_else(|| workflow_error("store_supervisor_dispatch_unbound"))?,
        )
        .map_err(|_| workflow_error("store_supervisor_projection_mismatch"))?,
    )?;
    if action.generation != execution.generation || action.state != SupervisorActionState::Claimed {
        return Err(workflow_error("store_supervisor_dispatch_unbound"));
    }
    let valid = match &action.action {
        SupervisorTool::AssignTask { task_key, slot_id } => {
            task_key == &admission.task_key
                && !matches!(admission.purpose, super::AttemptPurpose::Review)
                && workflow_rows::load_lease(transaction, admission.lease_id)?
                    .is_some_and(|lease| &lease.slot_id == slot_id)
        }
        SupervisorTool::RequestReview { task_key, .. } => {
            task_key == &admission.task_key
                && admission.purpose == super::AttemptPurpose::Review
                && workflow_rows::load_lease(transaction, admission.lease_id)?
                    .is_some_and(|lease| lease.slot_id == execution.reviewer_slot)
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(workflow_error("store_supervisor_dispatch_unbound"))
    }
}

pub(super) fn check_task_gate(
    transaction: &Transaction<'_>,
    workflow_id: Uuid,
    task_key: Option<&SpecIdentifier>,
) -> Result<(), StoreError> {
    let Some(execution) = load_execution(transaction, workflow_id)? else {
        return Ok(());
    };
    if execution.mode != SupervisorMode::Model {
        return Ok(());
    }
    let allowed = execution.state == SupervisorExecutionState::Running
        && match task_key {
            Some(key) => execution.task_gates.get(key) == Some(&SupervisorTaskGate::Active),
            None => execution
                .task_gates
                .values()
                .all(|gate| *gate == SupervisorTaskGate::Active),
        };
    if allowed {
        Ok(())
    } else {
        Err(workflow_error("store_supervisor_task_not_active"))
    }
}
