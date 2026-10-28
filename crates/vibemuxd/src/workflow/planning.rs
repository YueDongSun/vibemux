//! One-shot high-level planning owned by the daemon's pinned profile.
//!
//! This is a service child so effect ownership and the existing lifecycle
//! controls stay private. Duplicate requests only observe retained evidence.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use vibemux_workflow::task_spec::{TaskSpec, WorkflowMode};

use super::*;
use crate::workflow::{
    model_compiler::{compiler_call, run_compiler_candidates},
    planning_profile::{
        PLANNING_SCHEMA_VERSION, PlannedWorkflow, PlanningObservation, PlanningProfile,
        PlanningQuery, PlanningRequest, PlanningStatus,
    },
    planning_state::{PlanningCompiler, PlanningFiles, PlanningIntent, PlanningPrepared},
};

fn request_key(request_id: Uuid) -> String {
    format!("profile_{}", request_id.simple())
}

fn profile_for<'a>(
    setup: &'a WorkflowSetup,
    query: &PlanningQuery,
) -> Result<&'a PlanningProfile, WorkflowError> {
    let profile = setup
        .config
        .config
        .planning_profiles
        .iter()
        .find(|profile| profile.profile_id == query.profile_id)
        .ok_or(WorkflowError::NotFound)?;
    if profile.digest(setup.config.digest)? != query.profile_digest {
        return Err(WorkflowError::ContractMismatch);
    }
    Ok(profile)
}

/// Model ordering never selects slots. Each admitted task key is assigned in
/// lexical order to an approved worker; comparison uses the entire approved set.
fn assignments_for(
    profile: &PlanningProfile,
    task_keys: &[SpecIdentifier],
) -> Result<BTreeMap<SpecIdentifier, Vec<SpecIdentifier>>, WorkflowError> {
    let mut keys = task_keys.to_vec();
    keys.sort();
    let original_count = keys.len();
    keys.dedup();
    let mut slots = profile.worker_slots.clone();
    slots.sort();
    if keys.is_empty() || keys.len() != original_count || keys.len() > profile.max_tasks {
        return Err(WorkflowError::ContractRejected {
            codes: vec!["workflow_planning_task_count_invalid".into()],
        });
    }
    match profile.mode {
        WorkflowMode::Cooperate if keys.len() <= slots.len() => Ok(keys
            .into_iter()
            .zip(slots)
            .map(|(task, slot)| (task, vec![slot]))
            .collect()),
        WorkflowMode::Compare if keys.len() == 1 && slots.len() >= 2 => {
            Ok(keys.into_iter().map(|task| (task, slots.clone())).collect())
        }
        _ => Err(WorkflowError::ContractRejected {
            codes: vec!["workflow_planning_task_count_invalid".into()],
        }),
    }
}

fn compiled_request(
    profile: &PlanningProfile,
    request: &PlanningRequest,
    candidates: Vec<TaskSpec>,
) -> Result<WorkflowRequest, WorkflowError> {
    if candidates.iter().any(|candidate| {
        candidate.workflow_key != profile.workflow_key || candidate.workflow_mode != profile.mode
    }) {
        return Err(WorkflowError::ContractRejected {
            codes: vec!["workflow_spec_mode_mismatch".into()],
        });
    }
    let keys = candidates
        .iter()
        .map(|candidate| candidate.task_key.clone())
        .collect::<Vec<_>>();
    let assignments = assignments_for(profile, &keys)?;
    WorkflowRequest::parse(json!({
        "schema_version": 1,
        "request_key": request_key(request.request_id),
        "workflow_key": profile.workflow_key,
        "mode": profile.mode,
        "request_text": request.request_text,
        "task_specs": candidates,
        "integration_suites": profile.integration_suites,
        "assignments": assignments,
        "reviewer_slot": profile.reviewer_slot,
        "shared_contract_paths": profile.shared_contract_paths,
        "negative_control": false,
    }))
}

fn empty_observation(query: &PlanningQuery, status: PlanningStatus) -> PlanningObservation {
    PlanningObservation {
        schema_version: PLANNING_SCHEMA_VERSION,
        request_id: query.request_id,
        profile_id: query.profile_id.clone(),
        profile_digest: query.profile_digest,
        request_sha256: query.request_sha256,
        execute: query.execute,
        status,
        retry_allowed: false,
        workflow: None,
        execution: None,
    }
}

impl WorkflowService {
    /// Lists fixed profile metadata. Source text, host paths and policy remain
    /// inside the startup owner.
    pub async fn planning_profiles(&self) -> Result<Value, WorkflowError> {
        let setup = self.inner.setup()?;
        let profiles = setup
            .config
            .config
            .planning_profiles
            .iter()
            .map(|profile| {
                Ok(json!({
                    "profile_id": profile.profile_id,
                    "profile_digest": profile.digest(setup.config.digest)?,
                    "workflow_key": profile.workflow_key,
                    "mode": profile.mode,
                    "max_tasks": profile.max_tasks,
                    "allow_execution": profile.allow_execution,
                }))
            })
            .collect::<Result<Vec<_>, WorkflowError>>()?;
        Ok(json!({"schema_version": PLANNING_SCHEMA_VERSION, "profiles": profiles}))
    }

    /// Claims the complete original request once, compiles once, admits through
    /// the existing prepare owner and optionally makes one fenced start attempt.
    pub async fn plan_request(
        &self,
        request: PlanningRequest,
    ) -> Result<PlanningObservation, WorkflowError> {
        let query = request.query()?;
        let setup = self.inner.setup()?;
        let profile = profile_for(&setup, &query)?.clone();
        if request.execute && !profile.allow_execution {
            return Err(WorkflowError::RequestInvalid);
        }
        self.accepting()?;
        let _compiler_permit = self
            .inner
            .compiler_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::ShuttingDown)?;
        self.accepting()?;
        let files = PlanningFiles::new(setup.state.planning_root());
        let intent = PlanningIntent::new(&query, setup.config.digest);
        let checking_files = files.clone();
        let request_id = query.request_id;
        let existing_intent =
            tokio::task::spawn_blocking(move || checking_files.intent(request_id))
                .await
                .map_err(|_| WorkflowError::Internal)??;
        let workflow_id = workflow_id_for(&request_key(request.request_id));
        if let Some(existing_intent) = existing_intent {
            if existing_intent != intent {
                return Err(WorkflowError::RequestConflict);
            }
            return self.planning_observation(query).await;
        }
        {
            let writer = self.inner.writer.clone();
            let existing = call_writer(move || writer.workflow_record(workflow_id)).await?;
            let writer = self.inner.writer.clone();
            let prior_call = call_writer(move || writer.compiler_model_call(workflow_id)).await?;
            if existing.is_some() || prior_call.is_some() {
                return Err(WorkflowError::RequestConflict);
            }
        }
        let claiming_files = files.clone();
        let claiming_intent = intent.clone();
        let claimed = tokio::task::spawn_blocking(move || claiming_files.claim(&claiming_intent))
            .await
            .map_err(|_| WorkflowError::Internal)??;
        if !claimed {
            return self.planning_observation(query).await;
        }

        let input = json!({
            "schema_version": PLANNING_SCHEMA_VERSION,
            "request_key": request_key(request.request_id),
            "workflow_key": profile.workflow_key,
            "mode": profile.mode,
            "request_text": request.request_text,
            "planning_request": {
                "profile_id": query.profile_id,
                "profile_digest": query.profile_digest,
                "request_id": query.request_id,
                "request_sha256": query.request_sha256,
                "execute": query.execute,
            },
            "operator_policy": profile.policy,
            "max_tasks": profile.max_tasks,
            "worker_slot_count": profile.worker_slots.len(),
            "integration_suites": profile.integration_suites,
            "shared_contract_paths": profile.shared_contract_paths,
        });
        let result = async {
            let call = compiler_call(&setup, workflow_id, input).await?;
            let compiler = PlanningCompiler {
                schema_version: PLANNING_SCHEMA_VERSION,
                intent_digest: intent.digest()?,
                call_id: call.admission.call_id,
                request_fingerprint: call.admission.identity.request_fingerprint,
            };
            let compiler_files = files.clone();
            let compiler_intent = intent.clone();
            tokio::task::spawn_blocking(move || {
                compiler_files.publish_compiler(&compiler_intent, &compiler)
            })
            .await
            .map_err(|_| WorkflowError::Internal)??;
            let candidates = run_compiler_candidates(
                &self.inner.writer,
                &setup,
                self.inner.model_factory.as_ref(),
                workflow_id,
                call,
            )
            .await?;
            let complete_request = compiled_request(&profile, &request, candidates)?;
            let overrides = self.active_template_overrides().await?;
            self.prepare_planning_with(
                serde_json::to_value(complete_request).map_err(|_| WorkflowError::Internal)?,
                serde_json::to_value(&profile.policy).map_err(|_| WorkflowError::PolicyInvalid)?,
                overrides,
            )
            .await
        }
        .await;

        let prepared = match result {
            Ok(prepared) => prepared,
            Err(WorkflowError::ContractRejected { .. }) => {
                let writer = self.inner.writer.clone();
                let existing = call_writer(move || writer.workflow_record(workflow_id)).await?;
                if existing.is_none() {
                    let rejection_files = files.clone();
                    let rejection_intent = intent.clone();
                    tokio::task::spawn_blocking(move || {
                        rejection_files.publish_rejected(&rejection_intent)
                    })
                    .await
                    .map_err(|_| WorkflowError::Internal)??;
                }
                return self.planning_observation(query).await;
            }
            // A failed or completed-but-unretained compiler call is never replayed.
            // Its ledger preserves the effect facts; the planning claim stays unknown.
            Err(_) => return self.planning_observation(query).await,
        };
        let state = setup.state.clone();
        let plan = tokio::task::spawn_blocking(move || state.read_plan(workflow_id))
            .await
            .map_err(|_| WorkflowError::Internal)??;
        let receipt = PlanningPrepared {
            schema_version: PLANNING_SCHEMA_VERSION,
            intent_digest: intent.digest()?,
            workflow_id,
            start_contract: prepared.start_contract,
            plan_digest: plan.digest()?,
        };
        let publication_files = files.clone();
        let publication_intent = intent.clone();
        tokio::task::spawn_blocking(move || {
            publication_files.publish_prepared(&publication_intent, &receipt)
        })
        .await
        .map_err(|_| WorkflowError::Internal)??;

        if request.execute {
            let start_files = files.clone();
            let start_intent = intent.clone();
            let start_contract = prepared.start_contract;
            let first_send = tokio::task::spawn_blocking(move || {
                start_files.mark_start(&start_intent, start_contract)
            })
            .await
            .map_err(|_| WorkflowError::Internal)??;
            if first_send {
                // Native profile, verifier, lease and lifecycle gates remain in
                // start_workflow. Its acknowledgment is not reconstructed here.
                let _start_result = self
                    .start_workflow(start_contract, request.request_id)
                    .await;
            }
        }
        self.planning_observation(query).await
    }

    /// Reads retained metadata and one writer-owned record/execution snapshot.
    /// It never creates a claim, repairs a file, calls a model or starts a run.
    pub async fn planning_observation(
        &self,
        query: PlanningQuery,
    ) -> Result<PlanningObservation, WorkflowError> {
        query.validate()?;
        let setup = self.inner.setup()?;
        let profile = profile_for(&setup, &query)?.clone();
        let expected_intent = PlanningIntent::new(&query, setup.config.digest);
        let files = PlanningFiles::new(setup.state.planning_root());
        let request_id = query.request_id;
        let metadata = tokio::task::spawn_blocking(move || {
            Ok::<_, WorkflowError>((
                files.intent(request_id)?,
                files.compiler(request_id)?,
                files.prepared(request_id)?,
                files.rejected(request_id)?,
                files.start_marker(request_id)?,
            ))
        })
        .await
        .map_err(|_| WorkflowError::Internal)??;
        let (intent, compiler, prepared, rejected, start_marker) = metadata;
        let Some(intent) = intent else {
            return Ok(empty_observation(&query, PlanningStatus::NotObserved));
        };
        if intent != expected_intent {
            return Err(WorkflowError::RequestConflict);
        }
        let intent_digest = intent.digest()?;
        if let Some(rejected) = &rejected
            && (rejected.schema_version != PLANNING_SCHEMA_VERSION
                || rejected.intent_digest != intent_digest
                || prepared.is_some()
                || start_marker.is_some())
        {
            return Err(WorkflowError::RequestConflict);
        }
        let workflow_id = workflow_id_for(&request_key(request_id));
        let Some(compiler) = compiler else {
            if prepared.is_some() || rejected.is_some() || start_marker.is_some() {
                return Err(WorkflowError::RequestConflict);
            }
            return Ok(empty_observation(&query, PlanningStatus::OutcomeUnknown));
        };
        if compiler.schema_version != PLANNING_SCHEMA_VERSION
            || compiler.intent_digest != intent_digest
            || compiler.call_id.is_nil()
        {
            return Err(WorkflowError::RequestConflict);
        }
        let writer = self.inner.writer.clone();
        let compiler_record = call_writer(move || writer.compiler_model_call(workflow_id)).await?;
        let Some(compiler_record) = compiler_record else {
            return Ok(empty_observation(&query, PlanningStatus::OutcomeUnknown));
        };
        if compiler_record.call_id != compiler.call_id
            || compiler_record.admission.identity.request_fingerprint
                != compiler.request_fingerprint
            || compiler_record.admission.identity.config_sha256 != intent.config_digest
        {
            return Err(WorkflowError::RequestConflict);
        }
        let compiler_completed =
            compiler_record.state == vibemux_workflow::model_call::ModelCallState::Completed;
        let writer = self.inner.writer.clone();
        let (record, execution) =
            call_writer(move || writer.workflow_start_snapshot(workflow_id)).await?;
        let Some(record) = record else {
            return Ok(empty_observation(
                &query,
                if rejected.is_some() {
                    PlanningStatus::Rejected
                } else {
                    PlanningStatus::OutcomeUnknown
                },
            ));
        };
        if rejected.is_some() {
            return Err(WorkflowError::RequestConflict);
        }
        if !compiler_completed {
            return Ok(empty_observation(&query, PlanningStatus::OutcomeUnknown));
        }
        let state = setup.state.clone();
        let plan = tokio::task::spawn_blocking(move || state.read_plan(workflow_id))
            .await
            .map_err(|_| WorkflowError::Internal)?;
        let plan = match plan {
            Ok(plan) => plan,
            Err(WorkflowError::NotFound) => {
                return Ok(empty_observation(&query, PlanningStatus::OutcomeUnknown));
            }
            Err(error) => return Err(error),
        };
        let keys = plan.specs.keys().cloned().collect::<Vec<_>>();
        let expected_assignments = assignments_for(&profile, &keys)?;
        if record.request_key != request_key(request_id)
            || record.workflow_key != profile.workflow_key
            || record.mode != profile.mode
            || record.evidence_class != setup.config.config.evidence_class
            || record.verifier_digest != setup.config.verifier_digest
            || record.policy_digest
                != profile
                    .policy
                    .digest()
                    .map_err(|_| WorkflowError::Internal)?
            || record.integration_suites != profile.integration_suites
            || record.version == 0
            || plan.schema_version != 1
            || plan.request_sha256 != query.request_sha256
            || plan.policy != profile.policy
            || plan.reviewer_slot != profile.reviewer_slot
            || plan.assignments != expected_assignments
            || plan.negative_control
        {
            return Err(WorkflowError::ContractMismatch);
        }
        let start_contract = start_binding(&record, &plan)?;
        let plan_digest = plan.digest()?;
        if let Some(prepared) = prepared
            && (prepared.schema_version != PLANNING_SCHEMA_VERSION
                || prepared.intent_digest != intent_digest
                || prepared.workflow_id != workflow_id
                || prepared.start_contract != start_contract
                || prepared.plan_digest != plan_digest)
        {
            return Err(WorkflowError::ContractMismatch);
        }
        if let Some(marker) = &start_marker
            && (marker.schema_version != PLANNING_SCHEMA_VERSION
                || marker.intent_digest != intent_digest
                || marker.request_id != request_id
                || marker.start_contract != start_contract
                || !query.execute)
        {
            return Err(WorkflowError::RequestConflict);
        }

        let mut observation = empty_observation(&query, PlanningStatus::Prepared);
        observation.workflow = Some(PlannedWorkflow {
            workflow_id,
            start_contract,
            phase: record.phase,
            version: record.version,
        });
        if query.execute {
            let mut start = StartObservation {
                schema_version: 1,
                request_id,
                start_contract,
                status: StartObservationStatus::NotObserved,
                retry_allowed: false,
                workflow: None,
            };
            if let Some(execution) = execution
                && execution.start_request_id == request_id
                && start_marker.is_some()
            {
                if execution.workflow_id != workflow_id
                    || execution.plan_digest != plan_digest
                    || execution.generation == 0
                    || execution.mode != SupervisorMode::Deterministic
                {
                    return Err(WorkflowError::ContractMismatch);
                }
                start.status = StartObservationStatus::Observed;
                start.workflow = Some(ObservedWorkflowStart {
                    workflow_id,
                    generation: execution.generation,
                    mode: execution.mode,
                    execution_state: execution.state,
                    phase: record.phase,
                    version: record.version,
                });
            } else {
                observation.status = PlanningStatus::OutcomeUnknown;
            }
            observation.execution = Some(start);
        }
        Ok(observation)
    }
}
