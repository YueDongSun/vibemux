//! Typed workflow operations on the single writer (ADR 031 §4).
//!
//! Each method runs one `SqliteStore` workflow operation on the writer
//! thread. Every gate (lease fencing, attempt admission, candidate and
//! acceptance gates, message sequencing) is re-checked inside the store
//! transaction, so a caller holding a stale view cannot commit a stale
//! effect. Every operation is idempotent or fenced, so the bounded retry in
//! `call_writer` is safe.

use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{
    ContentEntry, LeaseCommit, LeaseFence, LeaseRecord, LeaseRequest, MessageChange, MessageCommit,
    MessageRecord, PolicyVersionRecord, ReceiptCommit, WorkflowAttemptAdmission,
    WorkflowAttemptCommit, WorkflowAttemptRecord, WorkflowCommit, WorkflowSnapshot,
};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    contract::ContractArtifact,
    gates::WorkflowPhase,
    leases::LeaseEvent,
    messages::{MessageEnvelope, Participant},
    optimizer::OptimizablePolicy,
    receipts::WorkflowReceipt,
    slots::SlotFacts,
    workflow_record::WorkflowRecord,
};

use crate::{WriterError, WriterHandle};

impl WriterHandle {
    pub fn record_supervisor_context_read(
        &self,
        action_id: Uuid,
        generation: u64,
        full_plan: serde_json::Value,
        result_refs: Vec<Uuid>,
        timestamp: OffsetDateTime,
    ) -> Result<(), WriterError> {
        self.with_store(move |store| {
            store.record_supervisor_context_read(
                action_id,
                generation,
                &full_plan,
                &result_refs,
                timestamp,
            )
        })
    }
    pub fn admit_supervisor_message(
        &self,
        action_id: Uuid,
        generation: u64,
        full_plan: serde_json::Value,
        envelope: MessageEnvelope,
        timestamp: OffsetDateTime,
    ) -> Result<MessageCommit, WriterError> {
        self.with_store(move |store| {
            store.admit_supervisor_message(action_id, generation, &full_plan, &envelope, timestamp)
        })
    }
    pub fn prepare_contract_edit(
        &self,
        admission: vibemux_store::ContractEditAdmission,
    ) -> Result<vibemux_store::ContractEditCommit, WriterError> {
        self.with_store(move |store| store.prepare_contract_edit(&admission))
    }

    pub fn workflow_contract_edit(
        &self,
        edit_id: Uuid,
    ) -> Result<Option<vibemux_store::ContractEditRecord>, WriterError> {
        self.with_store(move |store| store.workflow_contract_edit(edit_id))
    }
    pub fn begin_supervisor_execution(
        &self,
        start: vibemux_store::SupervisorStart,
    ) -> Result<vibemux_workflow::supervisor::SupervisorExecution, WriterError> {
        self.with_store(move |store| store.begin_supervisor_execution(&start))
    }
    pub fn supervisor_observation(
        &self,
        workflow_id: Uuid,
    ) -> Result<vibemux_workflow::supervisor::SupervisorObservation, WriterError> {
        self.with_store(move |store| store.supervisor_observation(workflow_id))
    }
    pub fn reserve_supervisor_round(
        &self,
        admission: vibemux_store::SupervisorRoundAdmission,
    ) -> Result<vibemux_workflow::supervisor::SupervisorRound, WriterError> {
        self.with_store(move |store| store.reserve_supervisor_round(&admission))
    }
    pub fn commit_supervisor_proposal(
        &self,
        round_id: Uuid,
        proposal: vibemux_workflow::supervisor::SupervisorProposal,
        timestamp: OffsetDateTime,
    ) -> Result<vibemux_store::SupervisorProposalCommit, WriterError> {
        self.with_store(move |store| {
            store.commit_supervisor_proposal(round_id, &proposal, timestamp)
        })
    }
    pub fn reject_supervisor_round(
        &self,
        round_id: Uuid,
        reason: String,
        malformed: bool,
        timestamp: OffsetDateTime,
    ) -> Result<vibemux_workflow::supervisor::SupervisorRound, WriterError> {
        self.with_store(move |store| {
            store.reject_supervisor_round(round_id, &reason, malformed, timestamp)
        })
    }
    pub fn claim_supervisor_action(
        &self,
        action_id: Uuid,
        generation: u64,
        timestamp: OffsetDateTime,
    ) -> Result<vibemux_workflow::supervisor::SupervisorAction, WriterError> {
        self.with_store(move |store| {
            store.claim_supervisor_action(action_id, generation, timestamp)
        })
    }
    pub fn finish_supervisor_action(
        &self,
        finish: vibemux_store::SupervisorActionFinish,
    ) -> Result<vibemux_workflow::supervisor::SupervisorAction, WriterError> {
        self.with_store(move |store| store.finish_supervisor_action(&finish))
    }
    pub fn supervisor_status(
        &self,
        workflow_id: Uuid,
    ) -> Result<Option<vibemux_workflow::supervisor::SupervisorStatus>, WriterError> {
        self.with_store(move |store| store.supervisor_status(workflow_id))
    }
    pub fn end_supervisor_execution(
        &self,
        workflow_id: Uuid,
        generation: u64,
        timestamp: OffsetDateTime,
    ) -> Result<(), WriterError> {
        self.with_store(move |store| {
            store.end_supervisor_execution(workflow_id, generation, timestamp)
        })
    }
    pub fn recover_supervisors(&self, timestamp: OffsetDateTime) -> Result<u32, WriterError> {
        self.with_store(move |store| store.recover_supervisors(timestamp))
    }
    pub fn admit_controller(
        &self,
        controller: vibemux_workflow::native_controller::NativeController,
        timestamp: OffsetDateTime,
    ) -> Result<vibemux_store::ControllerCommit, WriterError> {
        self.with_store(move |store| store.admit_controller(&controller, timestamp))
    }
    pub fn workflow_controller(
        &self,
        key: Uuid,
    ) -> Result<Option<vibemux_workflow::native_controller::NativeController>, WriterError> {
        self.with_store(move |store| store.workflow_controller(key))
    }
    pub fn controller_outbox(
        &self,
        effect_id: Uuid,
    ) -> Result<Option<vibemux_store::ControllerOutboxRecord>, WriterError> {
        self.with_store(move |store| store.controller_outbox(effect_id))
    }
    pub fn change_workflow_controller(
        &self,
        key: Uuid,
        expected_version: u64,
        fence: vibemux_workflow::native_controller::ControllerFence,
        actor: vibemux_workflow::native_controller::ControllerActor,
        command: vibemux_workflow::native_controller::ControllerCommand,
        timestamp: OffsetDateTime,
    ) -> Result<vibemux_store::ControllerCommit, WriterError> {
        self.with_store(move |store| {
            store.change_workflow_controller(
                key,
                expected_version,
                fence,
                actor,
                command,
                timestamp,
            )
        })
    }
    pub fn claim_controller_outbox(
        &self,
        effect_id: Uuid,
        expected_version: u64,
        fence: vibemux_workflow::native_controller::ControllerFence,
        timestamp: OffsetDateTime,
    ) -> Result<vibemux_store::ControllerOutboxCommit, WriterError> {
        self.with_store(move |store| {
            store.claim_controller_outbox(effect_id, expected_version, fence, timestamp)
        })
    }
    pub fn finish_controller_outbox(
        &self,
        effect_id: Uuid,
        fence: vibemux_workflow::native_controller::ControllerFence,
        outcome: vibemux_store::ControllerOutboxOutcome,
        timestamp: OffsetDateTime,
    ) -> Result<vibemux_store::ControllerOutboxCommit, WriterError> {
        self.with_store(move |store| {
            store.finish_controller_outbox(effect_id, fence, outcome, timestamp)
        })
    }
    pub fn bind_recovery_plan(
        &self,
        workflow_id: Uuid,
        plan: serde_json::Value,
        generation: u64,
        settings: vibemux_workflow::checkpoint::ContextSettingsIdentity,
        timestamp: OffsetDateTime,
    ) -> Result<vibemux_store::RecoveryBindingCommit, WriterError> {
        self.with_store(move |store| {
            store.bind_recovery_plan(workflow_id, &plan, generation, settings, timestamp)
        })
    }
    pub fn workflow_recovery_state(
        &self,
        workflow_id: Uuid,
    ) -> Result<vibemux_workflow::checkpoint::RecoveryState, WriterError> {
        self.with_store(move |store| store.workflow_recovery_state(workflow_id))
    }
    pub fn save_recovery_checkpoint(
        &self,
        checkpoint: vibemux_workflow::checkpoint::RecoveryCheckpoint,
        timestamp: OffsetDateTime,
    ) -> Result<vibemux_store::RecoveryCheckpointCommit, WriterError> {
        self.with_store(move |store| store.save_recovery_checkpoint(&checkpoint, timestamp))
    }
    pub fn load_recovery_checkpoint(
        &self,
        checkpoint_id: Uuid,
    ) -> Result<Option<vibemux_workflow::checkpoint::RecoveryCheckpoint>, WriterError> {
        self.with_store(move |store| store.load_recovery_checkpoint(checkpoint_id))
    }
    pub fn reserve_model_call(
        &self,
        admission: vibemux_workflow::model_call::ModelCallAdmission,
    ) -> Result<vibemux_workflow::model_call::ModelCallRecord, WriterError> {
        self.with_store(move |store| store.reserve_model_call(&admission))
    }
    pub fn submit_model_call(
        &self,
        call_id: Uuid,
    ) -> Result<vibemux_workflow::model_call::ModelCallRecord, WriterError> {
        self.with_store(move |store| store.submit_model_call(call_id))
    }
    pub fn finish_model_call(
        &self,
        call_id: Uuid,
        receipt: Option<vibemux_workflow::gateway::ModelCallReceipt>,
        error_code: Option<String>,
    ) -> Result<vibemux_workflow::model_call::ModelCallRecord, WriterError> {
        self.with_store(move |store| store.finish_model_call(call_id, receipt, error_code))
    }
    pub fn model_call(
        &self,
        call_id: Uuid,
    ) -> Result<Option<vibemux_workflow::model_call::ModelCallRecord>, WriterError> {
        self.with_store(move |store| store.model_call(call_id))
    }
    pub fn recover_model_calls(&self) -> Result<u32, WriterError> {
        self.with_store(move |store| store.recover_model_calls())
    }
    pub fn bind_workflow_contract_authority(
        &self,
        contract_id: Sha256Digest,
        spec: vibemux_workflow::task_spec::TaskSpec,
        timestamp: OffsetDateTime,
    ) -> Result<(), WriterError> {
        self.with_store(move |store| {
            store.bind_workflow_contract_authority(contract_id, &spec, timestamp)
        })
    }
    pub fn prepare_workflow(
        &self,
        record: WorkflowRecord,
        contracts: Vec<ContractArtifact>,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowCommit, WriterError> {
        self.with_store(move |store| store.prepare_workflow(&record, &contracts, timestamp))
    }

    pub fn workflow_record(
        &self,
        workflow_id: Uuid,
    ) -> Result<Option<WorkflowRecord>, WriterError> {
        self.with_store(move |store| store.workflow_record(workflow_id))
    }

    pub fn workflow_by_request_key(
        &self,
        request_key: String,
    ) -> Result<Option<WorkflowRecord>, WriterError> {
        self.with_store(move |store| store.workflow_by_request_key(&request_key))
    }

    pub fn workflows(&self, limit: usize) -> Result<Vec<WorkflowRecord>, WriterError> {
        self.with_store(move |store| store.workflows(limit))
    }

    pub fn workflows_after(
        &self,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<WorkflowRecord>, WriterError> {
        self.with_store(move |store| store.list_workflows_after(after, limit))
    }

    pub fn workflow_contract(
        &self,
        contract_id: Sha256Digest,
    ) -> Result<Option<ContractArtifact>, WriterError> {
        self.with_store(move |store| store.workflow_contract(contract_id))
    }

    pub fn workflow_snapshot(
        &self,
        workflow_id: Uuid,
    ) -> Result<Option<WorkflowSnapshot>, WriterError> {
        self.with_store(move |store| store.workflow_snapshot(workflow_id))
    }

    pub fn transition_workflow(
        &self,
        workflow_id: Uuid,
        expected_version: u64,
        phase: WorkflowPhase,
        reason_code: Option<String>,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowCommit, WriterError> {
        self.with_store(move |store| {
            store.transition_workflow(
                workflow_id,
                expected_version,
                phase,
                reason_code.as_deref(),
                timestamp,
            )
        })
    }

    pub fn add_contract_generation(
        &self,
        workflow_id: Uuid,
        expected_version: u64,
        artifact: ContractArtifact,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowCommit, WriterError> {
        self.with_store(move |store| {
            store.add_contract_generation(workflow_id, expected_version, &artifact, timestamp)
        })
    }

    pub fn upsert_workflow_slot(
        &self,
        slot: SlotFacts,
        timestamp: OffsetDateTime,
    ) -> Result<(), WriterError> {
        self.with_store(move |store| store.upsert_workflow_slot(&slot, timestamp).map(|_| ()))
    }

    pub fn workflow_slots(&self) -> Result<Vec<SlotFacts>, WriterError> {
        self.with_store(|store| store.workflow_slots())
    }

    pub fn acquire_workflow_lease(
        &self,
        request: LeaseRequest,
    ) -> Result<LeaseCommit, WriterError> {
        self.with_store(move |store| store.acquire_workflow_lease(&request))
    }

    pub fn workflow_lease(&self, lease_id: Uuid) -> Result<Option<LeaseRecord>, WriterError> {
        self.with_store(move |store| store.workflow_lease(lease_id))
    }

    pub fn change_workflow_lease(
        &self,
        lease_id: Uuid,
        event: LeaseEvent,
        heartbeat_ms: u64,
        reason_code: String,
        timestamp: OffsetDateTime,
    ) -> Result<LeaseCommit, WriterError> {
        self.with_store(move |store| {
            store.change_workflow_lease(lease_id, event, heartbeat_ms, &reason_code, timestamp)
        })
    }

    pub fn admit_workflow_attempt(
        &self,
        admission: WorkflowAttemptAdmission,
    ) -> Result<WorkflowAttemptCommit, WriterError> {
        self.with_store(move |store| store.admit_workflow_attempt(&admission))
    }

    pub fn workflow_attempt(
        &self,
        request_id: Uuid,
    ) -> Result<Option<WorkflowAttemptRecord>, WriterError> {
        self.with_store(move |store| store.workflow_attempt(request_id))
    }

    pub fn record_workflow_receipt(
        &self,
        workflow_id: Uuid,
        receipt: WorkflowReceipt,
        fence: Option<LeaseFence>,
        timestamp: OffsetDateTime,
    ) -> Result<ReceiptCommit, WriterError> {
        self.with_store(move |store| {
            store.record_workflow_receipt(workflow_id, &receipt, fence, timestamp)
        })
    }

    pub fn accept_workflow_candidate(
        &self,
        workflow_id: Uuid,
        expected_version: u64,
        task_key: SpecIdentifier,
        candidate_digest: Sha256Digest,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowCommit, WriterError> {
        self.with_store(move |store| {
            store.accept_workflow_candidate(
                workflow_id,
                expected_version,
                &task_key,
                candidate_digest,
                timestamp,
            )
        })
    }

    pub fn accept_workflow(
        &self,
        workflow_id: Uuid,
        expected_version: u64,
        timestamp: OffsetDateTime,
    ) -> Result<WorkflowCommit, WriterError> {
        self.with_store(move |store| {
            store.accept_workflow(workflow_id, expected_version, timestamp)
        })
    }

    pub fn admit_workflow_message(
        &self,
        envelope: MessageEnvelope,
        timestamp: OffsetDateTime,
    ) -> Result<MessageCommit, WriterError> {
        self.with_store(move |store| store.admit_workflow_message(&envelope, timestamp))
    }

    pub fn record_workflow_message_rejection(
        &self,
        workflow_id: Uuid,
        message_id: Uuid,
        sender: Participant,
        code: String,
        timestamp: OffsetDateTime,
    ) -> Result<(), WriterError> {
        self.with_store(move |store| {
            store
                .record_workflow_message_rejection(
                    workflow_id,
                    message_id,
                    &sender,
                    &code,
                    timestamp,
                )
                .map(|_| ())
        })
    }

    pub fn change_workflow_message(
        &self,
        message_id: Uuid,
        change: MessageChange,
        timestamp: OffsetDateTime,
    ) -> Result<MessageCommit, WriterError> {
        self.with_store(move |store| store.change_workflow_message(message_id, &change, timestamp))
    }

    pub fn workflow_message(&self, message_id: Uuid) -> Result<Option<MessageRecord>, WriterError> {
        self.with_store(move |store| store.workflow_message(message_id))
    }

    pub fn deliver_workflow_messages(
        &self,
        workflow_id: Uuid,
        message_ids: Vec<Uuid>,
        attempt: Uuid,
        timestamp: OffsetDateTime,
    ) -> Result<Vec<MessageCommit>, WriterError> {
        self.with_store(move |store| {
            store.deliver_workflow_messages(workflow_id, &message_ids, attempt, timestamp)
        })
    }

    pub fn record_workflow_content(
        &self,
        entry: ContentEntry,
        timestamp: OffsetDateTime,
    ) -> Result<(), WriterError> {
        self.with_store(move |store| store.record_workflow_content(&entry, timestamp).map(|_| ()))
    }

    pub fn delete_workflow_content(
        &self,
        sha256: Sha256Digest,
        reason_code: String,
        timestamp: OffsetDateTime,
    ) -> Result<Option<ContentEntry>, WriterError> {
        self.with_store(move |store| store.delete_workflow_content(sha256, &reason_code, timestamp))
    }

    pub fn workflow_content(
        &self,
        sha256: Sha256Digest,
    ) -> Result<Option<ContentEntry>, WriterError> {
        self.with_store(move |store| store.workflow_content(sha256))
    }

    pub fn prompt_policy_versions(&self) -> Result<Vec<PolicyVersionRecord>, WriterError> {
        self.with_store(|store| store.prompt_policy_versions())
    }

    pub fn promote_prompt_policy(
        &self,
        policy: OptimizablePolicy,
        timestamp: OffsetDateTime,
    ) -> Result<PolicyVersionRecord, WriterError> {
        self.with_store(move |store| store.promote_prompt_policy(&policy, timestamp))
    }

    pub fn rollback_prompt_policy(
        &self,
        timestamp: OffsetDateTime,
    ) -> Result<PolicyVersionRecord, WriterError> {
        self.with_store(move |store| store.rollback_prompt_policy(timestamp))
    }
}
