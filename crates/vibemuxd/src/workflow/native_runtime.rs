//! Fenced controller effects over an explicitly owned adapter and the writer.
//!
//! No terminal backend is supplied here. An installed adapter must certify its
//! inventory and literal-input behavior, retain exact process/resource ownership,
//! and recheck that ownership inside each effect operation. Fixture adapters live
//! in tests only. Acknowledged input is not task completion. A dropped future or
//! ambiguous acknowledgement never authorizes replay of submitted input.

use std::{fmt, future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use thiserror::Error;
use time::OffsetDateTime;
use tokio::{
    sync::{Semaphore, watch},
    time::Instant,
};
use uuid::Uuid;
use vibemux_store::{ControllerOutboxOutcome, ControllerOutboxRecord, ControllerOutboxState};
use vibemux_types::ProjectId;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    native_controller::{
        ApprovalChallenge, ApprovalDecision, ControllerActor, ControllerCommand, ControllerFence,
        ControllerObservation, ControllerOwner, ControllerPhase, HandoffTarget, HandoffTransfer,
        InterruptionState, MAX_INPUT_BYTES, NativeController, apply_controller_command,
    },
    slots::EvidenceClass,
};

use crate::{WriterError, WriterHandle};

const MAX_IN_FLIGHT_OPERATIONS: usize = 32;
const MAX_OPERATION_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug)]
pub struct NativeRuntimeLimits {
    pub operation_timeout: Duration,
    pub inventory_timeout: Duration,
    pub max_in_flight: usize,
}

impl Default for NativeRuntimeLimits {
    fn default() -> Self {
        Self {
            operation_timeout: Duration::from_secs(10),
            inventory_timeout: Duration::from_secs(2),
            max_in_flight: 8,
        }
    }
}

/// Opaque resource identity; the adapter binds this to its actual owned handles,
/// backend inventory, and canonical workspace. A pane number alone cannot match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerResource {
    pub project_id: ProjectId,
    pub workflow_id: Uuid,
    pub session_id: Uuid,
    pub controller: ControllerFence,
    pub backend_id: SpecIdentifier,
    pub resource_id: Uuid,
    pub ownership_token: Uuid,
    pub workspace_identity: Sha256Digest,
}

impl ControllerResource {
    fn matches_controller(&self, controller: &NativeController) -> bool {
        !self.resource_id.is_nil()
            && !self.ownership_token.is_nil()
            && self.workflow_id == controller.identity.workflow_id
            && self.session_id == controller.identity.session_id
            && self.controller == controller.fence()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterResourceState {
    /// A certified structured readiness observation, never guessed pane text.
    Ready,
    Running,
    Detached,
    Terminal,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerInventory {
    pub resource: ControllerResource,
    pub owner: ControllerOwner,
    pub state: AdapterResourceState,
    pub pending_approval: Option<ApprovalChallenge>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerAdapterCapabilities {
    pub literal_input: bool,
    pub correlated_acknowledgements: bool,
    pub interruption: bool,
    pub same_session_resume_evidence: Option<Sha256Digest>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerOperationKind {
    Input,
    Interruption,
    Attach,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerAcknowledgement {
    pub resource: ControllerResource,
    pub operation: ControllerOperationKind,
    pub operation_id: Uuid,
    pub payload_digest: Option<Sha256Digest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdapterDelivery {
    Acknowledged(Box<ControllerAcknowledgement>),
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ControllerAdapterError {
    #[error("owned controller resource is unavailable")]
    Unavailable,
    #[error("owned controller resource changed")]
    OwnershipChanged,
    #[error("adapter operation is unsupported")]
    Unsupported,
    #[error("adapter delivery is uncertain")]
    DeliveryUnknown,
}

/// Effects must use typed API/stdin data, not shell interpolation. Each method
/// must check `expected` against the adapter's owned inventory while holding its
/// input-ownership guard; callers' observations alone do not close that race.
#[async_trait]
pub trait ControllerAdapter: Send + Sync {
    fn evidence_class(&self) -> EvidenceClass;
    fn capabilities(&self) -> ControllerAdapterCapabilities;
    async fn inventory(
        &self,
        resource_id: Uuid,
    ) -> Result<ControllerInventory, ControllerAdapterError>;
    async fn submit_literal(
        &self,
        expected: &ControllerResource,
        submission_id: Uuid,
        bytes: &[u8],
    ) -> Result<AdapterDelivery, ControllerAdapterError>;
    async fn interrupt(
        &self,
        expected: &ControllerResource,
        submission_id: Uuid,
    ) -> Result<AdapterDelivery, ControllerAdapterError>;
    async fn attach(
        &self,
        old: &ControllerResource,
        next: &ControllerResource,
        transfer: HandoffTransfer,
    ) -> Result<AdapterDelivery, ControllerAdapterError>;
}

/// Prompt bytes are deliberately absent from Debug and all durable receipts.
pub struct LiteralInput(Vec<u8>);

impl LiteralInput {
    pub fn new(text: String) -> Result<Self, NativeRuntimeError> {
        if text.is_empty() || text.len() as u64 > MAX_INPUT_BYTES || text.contains('\0') {
            return Err(NativeRuntimeError::InvalidInput);
        }
        Ok(Self(text.into_bytes()))
    }

    pub fn digest(&self) -> Sha256Digest {
        Sha256Digest::of(&self.0)
    }
    pub fn byte_count(&self) -> u64 {
        self.0.len() as u64
    }
}

impl fmt::Debug for LiteralInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiteralInput")
            .field("byte_count", &self.byte_count())
            .field("digest", &self.digest())
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct NativeInputRequest {
    pub controller_key: Uuid,
    pub effect_id: Uuid,
    pub expected_version: u64,
    pub actor: ControllerActor,
    pub resource: ControllerResource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeInputStatus {
    Acknowledged,
    DeliveryUnknown,
    ReplaySuppressed,
}

#[derive(Clone, Debug)]
pub struct NativeInputResult {
    pub controller: NativeController,
    pub effect: ControllerOutboxRecord,
    pub status: NativeInputStatus,
}

#[derive(Clone, Debug)]
pub struct NativeControlResult {
    pub controller: NativeController,
    pub acknowledged: bool,
}

#[derive(Clone, Debug)]
pub struct NativeHandoffResult {
    pub controller: NativeController,
    pub transfer: HandoffTransfer,
    pub acknowledged: bool,
}

#[derive(Clone, Debug)]
pub struct ApprovalDecisionResult {
    pub controller: NativeController,
    pub challenge: ApprovalChallenge,
    /// No approval transport is implemented without a durable effect receipt.
    pub delivery_unknown: bool,
}

#[derive(Debug, Error)]
pub enum NativeRuntimeError {
    #[error("native runtime bounds are invalid")]
    InvalidLimits,
    #[error("native runtime capacity is exhausted")]
    Busy,
    #[error("controller or outbox was not found")]
    NotFound,
    #[error("controller resource, project, or session does not match")]
    ResourceMismatch,
    #[error("controller inventory is not ready or its ownership is unknown")]
    InventoryUnverified,
    #[error("literal input differs from the admitted reservation")]
    InvalidInput,
    #[error("controller or outbox fence is stale")]
    Stale,
    #[error("adapter capability or evidence class is incompatible")]
    Unsupported,
    #[error("operation is refused by controller ownership or state")]
    StateRefused,
    #[error("operation was cancelled")]
    Cancelled,
    #[error("operation deadline expired")]
    Deadline,
    #[error("writer task could not be joined")]
    WriterJoin,
    #[error(transparent)]
    Writer(#[from] WriterError),
    #[error(transparent)]
    Adapter(#[from] ControllerAdapterError),
}

/// One explicitly installed adapter; no implicit backend, native TUI, or live
/// certification is synthesized. The writer remains the only durable owner.
pub struct NativeControllerRuntime {
    writer: WriterHandle,
    adapter: Arc<dyn ControllerAdapter>,
    limits: NativeRuntimeLimits,
    capacity: Semaphore,
}

impl NativeControllerRuntime {
    pub fn new(
        writer: WriterHandle,
        adapter: Arc<dyn ControllerAdapter>,
        limits: NativeRuntimeLimits,
    ) -> Result<Self, NativeRuntimeError> {
        if limits.max_in_flight == 0
            || limits.max_in_flight > MAX_IN_FLIGHT_OPERATIONS
            || limits.operation_timeout.is_zero()
            || limits.operation_timeout > MAX_OPERATION_TIMEOUT
            || limits.inventory_timeout.is_zero()
            || limits.inventory_timeout > limits.operation_timeout
        {
            return Err(NativeRuntimeError::InvalidLimits);
        }
        Ok(Self {
            writer,
            adapter,
            limits,
            capacity: Semaphore::new(limits.max_in_flight),
        })
    }

    /// Only the winner of the durable Pending -> Submitted claim writes bytes.
    /// A writer timeout after that claim is uncertain and never triggers replay.
    pub async fn submit(
        &self,
        request: NativeInputRequest,
        input: LiteralInput,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<NativeInputResult, NativeRuntimeError> {
        let _permit = self
            .capacity
            .try_acquire()
            .map_err(|_| NativeRuntimeError::Busy)?;
        let capabilities = self.adapter.capabilities();
        if !capabilities.literal_input || !capabilities.correlated_acknowledgements {
            return Err(NativeRuntimeError::Unsupported);
        }
        let controller = self
            .checked_controller(request.controller_key, &request.resource)
            .await?;
        let writer = self.writer.clone();
        let effect_id = request.effect_id;
        let effect = writer_once(move || writer.controller_outbox(effect_id))
            .await?
            .ok_or(NativeRuntimeError::NotFound)?;
        self.check_input(&request, &input, &controller, &effect)?;
        if effect.state != ControllerOutboxState::Pending {
            return Ok(NativeInputResult {
                controller,
                effect,
                status: NativeInputStatus::ReplaySuppressed,
            });
        }
        if request.expected_version != controller.version {
            return Err(NativeRuntimeError::Stale);
        }
        apply_controller_command(
            &controller,
            controller.version,
            controller.fence(),
            request.actor,
            ControllerCommand::BeginSubmission {
                submission_id: effect.submission_id,
            },
        )
        .map_err(|_| NativeRuntimeError::StateRefused)?;
        self.checked_inventory(
            &request.resource,
            &controller,
            &[AdapterResourceState::Ready],
            &mut cancel,
        )
        .await?;
        let writer = self.writer.clone();
        let fence = request.resource.controller;
        let expected_version = request.expected_version;
        let claimed = writer_once(move || {
            writer.claim_controller_outbox(
                effect_id,
                expected_version,
                fence,
                OffsetDateTime::now_utc(),
            )
        })
        .await?;
        if claimed.duplicate {
            return Ok(NativeInputResult {
                controller: claimed.controller,
                effect: claimed.effect,
                status: NativeInputStatus::ReplaySuppressed,
            });
        }
        let inventory_ok = self
            .checked_inventory(
                &request.resource,
                &claimed.controller,
                &[AdapterResourceState::Ready],
                &mut cancel,
            )
            .await
            .is_ok();
        let delivery = if inventory_ok {
            bounded(
                self.adapter
                    .submit_literal(&request.resource, effect.submission_id, &input.0),
                self.limits.operation_timeout,
                &mut cancel,
            )
            .await
        } else {
            Err(NativeRuntimeError::InventoryUnverified)
        };
        let acknowledged = matches!(delivery, Ok(Ok(AdapterDelivery::Acknowledged(ref ack)))
            if ack.resource == request.resource && ack.operation == ControllerOperationKind::Input
                && ack.operation_id == effect.submission_id && ack.payload_digest == Some(input.digest()));
        let outcome = if acknowledged {
            ControllerOutboxOutcome::Acknowledged
        } else {
            ControllerOutboxOutcome::DeliveryUnknown
        };
        let writer = self.writer.clone();
        let finished = writer_once(move || {
            writer.finish_controller_outbox(effect_id, fence, outcome, OffsetDateTime::now_utc())
        })
        .await?;
        Ok(NativeInputResult {
            controller: finished.controller,
            effect: finished.effect,
            status: if acknowledged {
                NativeInputStatus::Acknowledged
            } else {
                NativeInputStatus::DeliveryUnknown
            },
        })
    }

    fn check_input(
        &self,
        request: &NativeInputRequest,
        input: &LiteralInput,
        controller: &NativeController,
        effect: &ControllerOutboxRecord,
    ) -> Result<(), NativeRuntimeError> {
        let submission = controller
            .last_submission
            .as_ref()
            .ok_or(NativeRuntimeError::InvalidInput)?;
        if effect.controller_key != request.controller_key
            || effect.controller_id != request.resource.controller.controller_id
            || effect.controller_generation != request.resource.controller.generation
            || effect.submission_id != submission.reservation.submission_id
        {
            return Err(NativeRuntimeError::Stale);
        }
        if input.digest() != effect.prompt_digest
            || input.byte_count() != effect.prompt_bytes
            || input.digest() != submission.reservation.prompt_digest
            || input.byte_count() != submission.reservation.prompt_bytes
        {
            return Err(NativeRuntimeError::InvalidInput);
        }
        Ok(())
    }

    /// Persist the interruption request before invoking the adapter. Its
    /// acknowledgement does not settle the submission or mark the task complete.
    pub async fn interrupt(
        &self,
        controller_key: Uuid,
        expected_version: u64,
        resource: ControllerResource,
        actor: ControllerActor,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<NativeControlResult, NativeRuntimeError> {
        let _permit = self
            .capacity
            .try_acquire()
            .map_err(|_| NativeRuntimeError::Busy)?;
        if !self.adapter.capabilities().interruption {
            return Err(NativeRuntimeError::Unsupported);
        }
        let controller = self.checked_controller(controller_key, &resource).await?;
        if controller.version != expected_version {
            return Err(NativeRuntimeError::Stale);
        }
        if let ControllerOwner::Human { operator_id } = controller.owner
            && actor != (ControllerActor::Human { operator_id })
        {
            return Err(NativeRuntimeError::StateRefused);
        }
        let submission_id = controller
            .last_submission
            .as_ref()
            .ok_or(NativeRuntimeError::StateRefused)?
            .reservation
            .submission_id;
        self.checked_inventory(
            &resource,
            &controller,
            &[AdapterResourceState::Ready, AdapterResourceState::Running],
            &mut cancel,
        )
        .await?;
        let writer = self.writer.clone();
        let fence = resource.controller;
        let claimed = writer_once(move || {
            writer.change_workflow_controller(
                controller_key,
                expected_version,
                fence,
                actor,
                ControllerCommand::RequestInterruption { submission_id },
                OffsetDateTime::now_utc(),
            )
        })
        .await?;
        if claimed.duplicate {
            let acknowledged =
                claimed.controller.interruption == Some(InterruptionState::Acknowledged);
            return Ok(NativeControlResult {
                controller: claimed.controller,
                acknowledged,
            });
        }
        let delivery = bounded(
            self.adapter.interrupt(&resource, submission_id),
            self.limits.operation_timeout,
            &mut cancel,
        )
        .await;
        let acknowledged = matches!(delivery, Ok(Ok(AdapterDelivery::Acknowledged(ref ack)))
            if ack.resource == resource && ack.operation == ControllerOperationKind::Interruption
                && ack.operation_id == submission_id && ack.payload_digest.is_none());
        let writer = self.writer.clone();
        let version = claimed.controller.version;
        let committed = writer_once(move || {
            writer.change_workflow_controller(
                controller_key,
                version,
                fence,
                ControllerActor::Adapter,
                ControllerCommand::ObserveInterruption {
                    submission_id,
                    acknowledged,
                },
                OffsetDateTime::now_utc(),
            )
        })
        .await?;
        Ok(NativeControlResult {
            controller: committed.controller,
            acknowledged,
        })
    }

    /// Starts quiescing only. An active turn must be settled separately; this
    /// operation does not close a pane, terminate a process, or invent a checkpoint.
    pub async fn begin_handoff(
        &self,
        controller_key: Uuid,
        expected_version: u64,
        resource: ControllerResource,
        target: HandoffTarget,
        actor: ControllerActor,
    ) -> Result<NativeController, NativeRuntimeError> {
        let controller = self.checked_controller(controller_key, &resource).await?;
        self.check_transfer(target.transfer)?;
        if controller.version != expected_version {
            return Err(NativeRuntimeError::Stale);
        }
        let writer = self.writer.clone();
        let fence = resource.controller;
        Ok(writer_once(move || {
            writer.change_workflow_controller(
                controller_key,
                expected_version,
                fence,
                actor,
                ControllerCommand::BeginHandoff(target),
                OffsetDateTime::now_utc(),
            )
        })
        .await?
        .controller)
    }

    /// Observes the old controller as terminal before claiming the new identity.
    /// Attach is claimed through CAS before its external effect. Failure leaves
    /// the successor paused; a second call cannot replay the old handoff.
    pub async fn finish_handoff(
        &self,
        controller_key: Uuid,
        expected_version: u64,
        old: ControllerResource,
        next: ControllerResource,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<NativeHandoffResult, NativeRuntimeError> {
        let _permit = self
            .capacity
            .try_acquire()
            .map_err(|_| NativeRuntimeError::Busy)?;
        let mut controller = self.checked_controller(controller_key, &old).await?;
        if controller.version != expected_version {
            return Err(NativeRuntimeError::Stale);
        }
        let target = controller.handoff.ok_or(NativeRuntimeError::StateRefused)?;
        self.check_transfer(target.transfer)?;
        if next.project_id != old.project_id
            || next.workflow_id != old.workflow_id
            || next.backend_id != old.backend_id
            || next.workspace_identity != old.workspace_identity
            || next.controller.controller_id != target.controller_id
            || next.session_id != target.session_id
            || controller.generation.checked_add(1) != Some(next.controller.generation)
            || next.resource_id.is_nil()
            || next.ownership_token.is_nil()
        {
            return Err(NativeRuntimeError::ResourceMismatch);
        }
        self.checked_inventory(
            &old,
            &controller,
            &[AdapterResourceState::Terminal],
            &mut cancel,
        )
        .await?;
        if controller.phase == ControllerPhase::Quiescing {
            let writer = self.writer.clone();
            let fence = old.controller;
            let version = controller.version;
            controller = writer_once(move || {
                writer.change_workflow_controller(
                    controller_key,
                    version,
                    fence,
                    ControllerActor::Adapter,
                    ControllerCommand::ObserveOldController(ControllerObservation::Terminal),
                    OffsetDateTime::now_utc(),
                )
            })
            .await?
            .controller;
        }
        if controller.phase != ControllerPhase::AwaitingAttach {
            return Err(NativeRuntimeError::StateRefused);
        }
        let target_inventory = bounded(
            self.adapter.inventory(next.resource_id),
            self.limits.inventory_timeout,
            &mut cancel,
        )
        .await??;
        if target_inventory.resource != next
            || target_inventory.owner != target.owner
            || target_inventory.state != AdapterResourceState::Detached
            || target_inventory.pending_approval.is_some()
        {
            return Err(NativeRuntimeError::InventoryUnverified);
        }
        let writer = self.writer.clone();
        let version = controller.version;
        let fence = old.controller;
        let claimed = writer_once(move || {
            writer.change_workflow_controller(
                controller_key,
                version,
                fence,
                ControllerActor::Adapter,
                ControllerCommand::Attach {
                    controller_id: target.controller_id,
                    session_id: target.session_id,
                },
                OffsetDateTime::now_utc(),
            )
        })
        .await?;
        let delivery = bounded(
            self.adapter.attach(&old, &next, target.transfer),
            self.limits.operation_timeout,
            &mut cancel,
        )
        .await;
        let mut acknowledged = matches!(delivery, Ok(Ok(AdapterDelivery::Acknowledged(ref ack)))
            if ack.resource == next && ack.operation == ControllerOperationKind::Attach && ack.operation_id == target.controller_id && ack.payload_digest.is_none());
        if acknowledged {
            acknowledged = self
                .checked_inventory(
                    &next,
                    &claimed.controller,
                    &[AdapterResourceState::Ready],
                    &mut cancel,
                )
                .await
                .is_ok();
        }
        Ok(NativeHandoffResult {
            controller: claimed.controller,
            transfer: target.transfer,
            acknowledged,
        })
    }

    /// Records a scoped human decision without claiming it reached the adapter.
    /// Approval transport needs its own durable effect/outcome contract; until
    /// then the exact decision is retained and automatic delivery is unavailable.
    pub async fn record_approval_decision(
        &self,
        controller_key: Uuid,
        expected_version: u64,
        resource: ControllerResource,
        operator_id: Uuid,
        challenge: ApprovalChallenge,
        decision: ApprovalDecision,
    ) -> Result<ApprovalDecisionResult, NativeRuntimeError> {
        let controller = self.checked_controller(controller_key, &resource).await?;
        if controller.version != expected_version || controller.pending_approval != Some(challenge)
        {
            return Err(NativeRuntimeError::Stale);
        }
        let writer = self.writer.clone();
        let fence = resource.controller;
        let committed = writer_once(move || {
            writer.change_workflow_controller(
                controller_key,
                expected_version,
                fence,
                ControllerActor::Human { operator_id },
                ControllerCommand::ResolveApproval {
                    challenge,
                    decision,
                },
                OffsetDateTime::now_utc(),
            )
        })
        .await?;
        Ok(ApprovalDecisionResult {
            controller: committed.controller,
            challenge,
            delivery_unknown: true,
        })
    }

    fn check_transfer(&self, transfer: HandoffTransfer) -> Result<(), NativeRuntimeError> {
        if let HandoffTransfer::SameSession {
            capability_evidence,
        } = transfer
            && self.adapter.capabilities().same_session_resume_evidence != Some(capability_evidence)
        {
            return Err(NativeRuntimeError::Unsupported);
        }
        Ok(())
    }

    async fn checked_controller(
        &self,
        controller_key: Uuid,
        resource: &ControllerResource,
    ) -> Result<NativeController, NativeRuntimeError> {
        let writer = self.writer.clone();
        let (project_id, controller) = writer_once(move || {
            Ok((
                writer.project_id()?,
                writer.workflow_controller(controller_key)?,
            ))
        })
        .await?;
        let controller = controller.ok_or(NativeRuntimeError::NotFound)?;
        controller
            .validate()
            .map_err(|_| NativeRuntimeError::StateRefused)?;
        if project_id != Some(resource.project_id) || !resource.matches_controller(&controller) {
            return Err(NativeRuntimeError::ResourceMismatch);
        }
        let writer = self.writer.clone();
        let workflow_id = controller.identity.workflow_id;
        let workflow = writer_once(move || writer.workflow_record(workflow_id))
            .await?
            .ok_or(NativeRuntimeError::NotFound)?;
        if workflow.evidence_class != self.adapter.evidence_class() {
            return Err(NativeRuntimeError::Unsupported);
        }
        if workflow.phase.is_terminal() {
            return Err(NativeRuntimeError::StateRefused);
        }
        Ok(controller)
    }

    async fn checked_inventory(
        &self,
        resource: &ControllerResource,
        controller: &NativeController,
        states: &[AdapterResourceState],
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), NativeRuntimeError> {
        let inventory = bounded(
            self.adapter.inventory(resource.resource_id),
            self.limits.inventory_timeout,
            cancel,
        )
        .await??;
        if inventory.resource != *resource || inventory.owner != controller.owner {
            return Err(NativeRuntimeError::ResourceMismatch);
        }
        if !states.contains(&inventory.state)
            || inventory.pending_approval != controller.pending_approval
        {
            return Err(NativeRuntimeError::InventoryUnverified);
        }
        Ok(())
    }
}

/// No automatic retry: a lost writer response may already have committed a claim.
async fn writer_once<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, WriterError> + Send + 'static,
) -> Result<T, NativeRuntimeError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| NativeRuntimeError::WriterJoin)?
        .map_err(NativeRuntimeError::Writer)
}

async fn bounded<T>(
    future: impl Future<Output = T>,
    timeout: Duration,
    cancel: &mut watch::Receiver<bool>,
) -> Result<T, NativeRuntimeError> {
    if *cancel.borrow() {
        return Err(NativeRuntimeError::Cancelled);
    }
    tokio::select! {
        biased;
        _ = cancelled(cancel) => Err(NativeRuntimeError::Cancelled),
        _ = tokio::time::sleep_until(Instant::now() + timeout) => Err(NativeRuntimeError::Deadline),
        value = future => Ok(value),
    }
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if cancel.changed().await.is_err() || *cancel.borrow_and_update() {
            return;
        }
    }
}
