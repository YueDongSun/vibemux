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
