//! Operator goal admission and query surface. The daemon owns every effect.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vibemux_store::{WorkflowOperationRecord, WorkflowOperationState as State};
use vibemux_workflow::canonical_json::canonical_digest;

use super::*;

const MAX_GOAL_OWNERS: usize = 4;
pub(super) fn goal_gate() -> Arc<tokio::sync::Semaphore> {
    Arc::new(tokio::sync::Semaphore::new(MAX_GOAL_OWNERS))
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalSubmission {
    pub schema_version: u32,
    pub event_id: Uuid,
    pub request: Value,
    pub policy: Value,
    #[serde(default = "start_by_default")]
    pub auto_start: bool,
}

fn start_by_default() -> bool {
    true
}

/// Only a locally proven refusal is classified as blocked. Missing writer,
/// model, process or internal receipts remain unknown even when their public
/// error code does not contain that word.
fn failure_state(error: &WorkflowError) -> State {
    match error {
        WorkflowError::Unconfigured
        | WorkflowError::ConfigInvalid
        | WorkflowError::Untrusted
        | WorkflowError::VerifierChanged
        | WorkflowError::RequestInvalid
        | WorkflowError::RequestConflict
        | WorkflowError::PolicyInvalid
        | WorkflowError::CompilerUnavailable
        | WorkflowError::ContractRejected { .. }
        | WorkflowError::SlotUnavailable
        | WorkflowError::ContractMismatch
        | WorkflowError::InvalidPhase
        | WorkflowError::AlreadyRunning => State::Blocked,
        _ => State::Unknown,
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct OperationResponse {
    pub schema_version: u32,
    pub event_id: Uuid,
    pub kind: String,
    pub state: State,
    pub workflow_id: Option<Uuid>,
    pub start_contract: Option<Sha256Digest>,
    pub code: Option<String>,
    pub duplicate: bool,
    pub updated_at_ms: u64,
}

impl OperationResponse {
    fn from_record(record: WorkflowOperationRecord, duplicate: bool) -> Self {
        Self {
            schema_version: 1,
            event_id: record.event_id,
            kind: record.kind,
            state: record.state,
            workflow_id: record.workflow_id,
            start_contract: record.start_contract,
            code: record.code,
            duplicate,
            updated_at_ms: record.updated_at_ms,
        }
    }
}

impl WorkflowService {
    /// Commits the event identity before launching bounded asynchronous work.
    /// Retrying the exact event only reads its receipt; it cannot start a second owner.
    pub async fn submit_goal(
        &self,
        submission: GoalSubmission,
    ) -> Result<OperationResponse, WorkflowError> {
        if submission.schema_version != 1 || submission.event_id.is_nil() {
            return Err(WorkflowError::RequestInvalid);
        }
        let mut request = submission.request.clone();
        request
            .as_object_mut()
            .ok_or(WorkflowError::RequestInvalid)?
            .insert(
                "request_key".into(),
                json!(format!("goal_{}", submission.event_id.simple())),
            );
        WorkflowRequest::parse(request.clone())?;
        let policy_bytes =
            serde_json::to_vec(&submission.policy).map_err(|_| WorkflowError::PolicyInvalid)?;
        if policy_bytes.len() > MAX_POLICY_BYTES {
            return Err(WorkflowError::PolicyInvalid);
        }
        OperatorPolicy::parse(&policy_bytes).map_err(|_| WorkflowError::PolicyInvalid)?;
        let fingerprint = canonical_digest("vibemux.workflow.goal.v1", &json!({
            "request": request, "policy": submission.policy, "auto_start": submission.auto_start,
        })).map_err(|_| WorkflowError::RequestInvalid)?;
        let event_id = submission.event_id;
        let writer = self.inner.writer.clone();
        if let Some(existing) = call_writer(move || writer.workflow_operation(event_id)).await? {
            if existing.fingerprint != fingerprint {
                return Err(WorkflowError::Dispatch {
                    code: "workflow_operation_conflict".into(),
                });
            }
            return Ok(OperationResponse::from_record(existing, true));
        }
        let _lifecycle = self
            .inner
            .lifecycle_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::ShuttingDown)?;
        self.accepting()?;
        let permit = Arc::clone(&self.inner.goal_gate)
            .try_acquire_owned()
            .map_err(|_| WorkflowError::Dispatch {
                code: "workflow_operation_capacity".into(),
            })?;
        let writer = self.inner.writer.clone();
        let commit = call_writer(move || {
            writer.admit_workflow_operation(event_id, fingerprint, OffsetDateTime::now_utc())
        })
        .await?;
        if !commit.duplicate {
            let owner = self.clone();
            let mut runs = self.inner.runs();
            while runs.tasks.try_join_next().is_some() {}
            runs.tasks.spawn(async move {
                let _permit = permit;
                owner
                    .execute_goal(event_id, request, submission.policy, submission.auto_start)
                    .await;
            });
        }
        Ok(OperationResponse::from_record(
            commit.operation,
            commit.duplicate,
        ))
    }

    async fn set_operation(
        &self,
        event_id: Uuid,
        state: State,
        workflow_id: Option<Uuid>,
        start_contract: Option<Sha256Digest>,
        code: Option<String>,
    ) -> Result<(), WorkflowError> {
        let writer = self.inner.writer.clone();
        call_writer(move || {
            writer.finish_workflow_operation(
                event_id,
                state,
                workflow_id,
                start_contract,
                code.clone(),
                OffsetDateTime::now_utc(),
            )
        })
        .await?;
        Ok(())
    }

    async fn execute_goal(&self, event_id: Uuid, request: Value, policy: Value, auto_start: bool) {
        // A lost admission/state-update receipt must not be followed by an effect.
        if self
            .set_operation(event_id, State::Preparing, None, None, None)
            .await
            .is_err()
        {
            return;
        }
        let prepared = match self.prepare(request, policy).await {
            Ok(prepared) => prepared,
            Err(error) => {
                let state = failure_state(&error);
                let _ = self
                    .set_operation(event_id, state, None, None, Some(error.code().into()))
                    .await;
                return;
            }
        };
        let workflow_id = Some(prepared.workflow_id);
        let contract = Some(prepared.start_contract);
        let (state, code) = if auto_start {
            // Stable request identity makes coordinator-start reconciliation possible.
            match self.start_workflow(prepared.start_contract, event_id).await {
                Ok(started) if started.phase == WorkflowPhase::Running => (State::Running, None),
                Ok(_) => (State::Blocked, Some("workflow_start_blocked".into())),
                Err(error) => (failure_state(&error), Some(error.code().into())),
            }
        } else {
            (State::Prepared, None)
        };
        let _ = self
            .set_operation(event_id, state, workflow_id, contract, code)
            .await;
    }

    pub async fn operation(&self, event_id: Uuid) -> Result<OperationResponse, WorkflowError> {
        if event_id.is_nil() {
            return Err(WorkflowError::RequestInvalid);
        }
        let writer = self.inner.writer.clone();
        let operation = call_writer(move || writer.workflow_operation(event_id))
            .await?
            .ok_or_else(|| WorkflowError::Dispatch {
                code: "workflow_operation_not_found".into(),
            })?;
        Ok(OperationResponse::from_record(operation, false))
    }

    pub async fn index(&self, after: Option<Uuid>, limit: usize) -> Result<Value, WorkflowError> {
        if !(1..=32).contains(&limit) || after.is_some_and(|id| id.is_nil()) {
            return Err(WorkflowError::RequestInvalid);
        }
        let writer = self.inner.writer.clone();
        let mut records = call_writer(move || writer.workflows_after(after, limit + 1)).await?;
        let more = records.len() > limit;
        records.truncate(limit);
        let next = if more {
            records.last().map(|record| record.workflow_id)
        } else {
            None
        };
        let workflows: Vec<_> = records.into_iter().map(|record| json!({
            "workflow_id": record.workflow_id, "workflow_key": record.workflow_key,
            "phase": record.phase, "version": record.version, "evidence_class": record.evidence_class,
            "updated_at_ms": record.updated_at_ms, "failure_code": record.blocked_reason,
        })).collect();
        Ok(json!({"schema_version":1, "workflows":workflows, "next":next, "more":more}))
    }

    /// Read-only readiness; no provider call or declaration-to-certification promotion.
    pub async fn readiness(&self) -> Result<Value, WorkflowError> {
        let mut blockers = Vec::new();
        let mut start_ready = false;
        let (configured, compiler_available, slots) = match self.inner.setup() {
            Ok(setup) => {
                let compiler = setup.config.config.native_compiler.is_some()
                    || (self.inner.model_factory.is_some()
                        && setup.config.config.gateway.as_ref().is_some_and(|gateway| {
                            gateway
                                .binding(vibemux_workflow::gateway::RouteRole::Compiler)
                                .is_some()
                        }));
                if !compiler {
                    blockers.push("workflow_compiler_unavailable".to_string());
                }
                match self.slots().await {
                    Ok(slots) => {
                        let worker_ready = setup.config.config.slots.iter().any(|configured| {
                            !configured.is_reviewer()
                                && slots.writable.eligible.contains(&configured.slot_id)
                                && setup.slot_protocols.contains_key(&configured.slot_id)
                        });
                        let reviews = evaluate_eligibility(
                            &slots.slots,
                            EligibilityRequest {
                                required: &REVIEW_REQUIREMENTS,
                                class: setup.config.config.evidence_class,
                                interface_mode: InterfaceMode::Structured,
                                now_ms: now_ms(),
                                excluded_slots: &[],
                                allow_unknown_quota: true,
                            },
                        );
                        let reviewer_ready = setup.config.config.slots.iter().any(|configured| {
                            configured.is_reviewer()
                                && reviews.eligible.contains(&configured.slot_id)
                                && setup.slot_protocols.contains_key(&configured.slot_id)
                        });
                        if !worker_ready {
                            blockers.push("no_eligible_slot".into());
                        }
                        if !reviewer_ready {
                            blockers.push("no_eligible_reviewer".into());
                        }
                        let verifier = Arc::clone(&setup);
                        let verifier_ready = match tokio::task::spawn_blocking(move || {
                            verifier.config.check_verifier_unchanged()
                        })
                        .await
                        {
                            Ok(Ok(())) => true,
                            Ok(Err(error)) => {
                                blockers.push(error.code().into());
                                false
                            }
                            Err(_) => {
                                blockers.push("workflow_internal".into());
                                false
                            }
                        };
                        start_ready = worker_ready && reviewer_ready && verifier_ready;
                        (true, compiler, Some(slots))
                    }
                    Err(error) => {
                        blockers.push(error.code().into());
                        (true, compiler, None)
                    }
                }
            }
            Err(error) => {
                blockers.push(error.code().into());
                (false, false, None)
            }
        };
        Ok(
            json!({"schema_version":1,"configured":configured,"compiler_available":compiler_available,
            "start_ready":start_ready,"blockers":blockers,"slots":slots}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_receipts_are_unknown_even_without_unknown_in_the_error_code() {
        for error in [
            WorkflowError::Internal,
            WorkflowError::Gateway,
            WorkflowError::Git,
            WorkflowError::Writer {
                code: "writer_unavailable".into(),
            },
            WorkflowError::Dispatch {
                code: "model_call_deadline_exceeded".into(),
            },
        ] {
            assert_eq!(failure_state(&error), State::Unknown);
        }
        for error in [
            WorkflowError::RequestInvalid,
            WorkflowError::PolicyInvalid,
            WorkflowError::CompilerUnavailable,
            WorkflowError::SlotUnavailable,
        ] {
            assert_eq!(failure_state(&error), State::Blocked);
        }
    }

    #[test]
    fn submitted_documents_cannot_smuggle_operator_authority() {
        let mut request =
            json!({"schema_version":1,"event_id":Uuid::new_v4(),"request":{},"policy":{}});
        let parsed: GoalSubmission = serde_json::from_value(request.clone()).unwrap();
        assert!(parsed.auto_start);
        request["actor"] = json!("trusted_operator");
        assert!(serde_json::from_value::<GoalSubmission>(request).is_err());
    }
}
