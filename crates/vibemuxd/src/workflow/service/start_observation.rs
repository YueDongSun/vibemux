//! Read-only positive correlation of the latest durable start admission.
use super::*;
use vibemux_workflow::supervisor::SupervisorExecutionState;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartObservationStatus {
    Observed,
    NotObserved,
}

#[derive(Clone, Debug, Serialize)]
pub struct ObservedWorkflowStart {
    pub workflow_id: Uuid,
    pub generation: u64,
    pub mode: SupervisorMode,
    pub execution_state: SupervisorExecutionState,
    pub phase: WorkflowPhase,
    pub version: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConditionalPhaseControl {
    pub workflow_id: Uuid,
    pub phase: WorkflowPhase,
    pub signalled: bool,
    pub request_id: Uuid,
    pub generation: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StartObservation {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub start_contract: Sha256Digest,
    pub status: StartObservationStatus,
    /// Neither a positive observation nor a latest-row miss authorizes another start.
    pub retry_allowed: bool,
    pub workflow: Option<ObservedWorkflowStart>,
}

impl WorkflowService {
    /// Observes durable admission only. It does not reconstruct a launch acknowledgment,
    /// run admission checks with effects, settle leases, or recover/restart coordinators.
    pub async fn start_observation(
        &self,
        contract: Sha256Digest,
        request_id: Uuid,
    ) -> Result<StartObservation, WorkflowError> {
        let _lifecycle_permit = self
            .inner
            .lifecycle_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::Internal)?;
        self.start_observation_owned(contract, request_id).await
    }

    /// Caller holds lifecycle_gate through this read and any following conditional effect.
    async fn start_observation_owned(
        &self,
        contract: Sha256Digest,
        request_id: Uuid,
    ) -> Result<StartObservation, WorkflowError> {
        let setup = self.inner.setup()?;
        let (selected, plan) = self.find_by_binding(&setup, contract).await?;
        let workflow_id = selected.workflow_id;
        let writer = self.inner.writer.clone();
        let (record, execution) =
            call_writer(move || writer.workflow_start_snapshot(workflow_id)).await?;
        let record = record.ok_or(WorkflowError::NotFound)?;
        if start_binding(&record, &plan)? != contract {
            return Err(WorkflowError::ContractMismatch);
        }
        let mut observation = StartObservation {
            schema_version: 1,
            request_id,
            start_contract: contract,
            status: StartObservationStatus::NotObserved,
            retry_allowed: false,
            workflow: None,
        };
        let Some(execution) = execution else {
            return Ok(observation);
        };
        // Storage retains the latest generation only. Mismatch is not proof of absence.
        if execution.start_request_id != request_id {
            return Ok(observation);
        }
        if execution.workflow_id != workflow_id || execution.plan_digest != plan.digest()? {
            return Err(WorkflowError::ContractMismatch);
        }
        if execution.generation == 0 || record.version == 0 {
            return Err(WorkflowError::Internal);
        }
        observation.status = StartObservationStatus::Observed;
        observation.workflow = Some(ObservedWorkflowStart {
            workflow_id,
            generation: execution.generation,
            mode: execution.mode,
            execution_state: execution.state,
            phase: record.phase,
            version: record.version,
        });
        Ok(observation)
    }

    /// Checks the exact latest start identity while excluding a new generation,
    /// then records/signals cancellation under the same lifecycle permit.
    pub async fn cancel_start(
        &self,
        contract: Sha256Digest,
        request_id: Uuid,
        generation: u64,
    ) -> Result<ConditionalPhaseControl, WorkflowError> {
        if generation == 0 {
            return Err(WorkflowError::RequestInvalid);
        }
        let lifecycle_permit = self
            .inner
            .lifecycle_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::Internal)?;
        let observed = self.start_observation_owned(contract, request_id).await?;
        let current = observed
            .workflow
            .filter(|workflow| workflow.generation == generation)
            .ok_or(WorkflowError::StartIdentityMismatch)?;
        let result = self
            .cancel_owned(current.workflow_id, &lifecycle_permit)
            .await?;
        Ok(ConditionalPhaseControl {
            workflow_id: result.workflow_id,
            phase: result.phase,
            signalled: result.signalled,
            request_id,
            generation,
        })
    }
}
