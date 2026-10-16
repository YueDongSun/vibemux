//! Pure ownership and input-delivery rules for a native or structured controller.
//!
//! The daemon authenticates actors and obtains correlated adapter evidence before
//! supplying commands. A terminal observation concerns the old input controller,
//! not terminal text or an idle pane. Persist each transition before its effect:
//! in particular `BeginSubmission` precedes writing input and cannot be replayed.
//! These rules certify no vendor, terminal backend, or live Windows operation.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::Sha256Digest;

pub const CONTROLLER_SCHEMA_VERSION: u32 = 1;
pub const MAX_INPUT_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum ControllerOwner {
    Structured,
    Native,
    Human { operator_id: Uuid },
}

impl ControllerOwner {
    fn valid(self) -> bool {
        !matches!(self, Self::Human { operator_id } if operator_id.is_nil())
    }
}

/// The authenticated source of a command, supplied by the daemon, never prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerActor {
    Automation,
    Adapter,
    Human { operator_id: Uuid },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPhase {
    Active,
    Paused,
    Quiescing,
    AwaitingAttach,
    Closed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerIdentity {
    pub workflow_id: Uuid,
    pub session_id: Uuid,
    pub controller_id: Uuid,
    pub contract_id: Sha256Digest,
    pub contract_version: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerFence {
    pub controller_id: Uuid,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Reserved,
    Submitted,
    Acknowledged,
    DeliveryUnknown,
    Settled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionOutcome {
    Completed,
    Cancelled,
    Failed,
    /// Requires affirmative reconciliation evidence, not process absence alone.
    NotDelivered,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputReservation {
    pub submission_id: Uuid,
    pub sequence: u64,
    pub contract_id: Sha256Digest,
    pub prompt_digest: Sha256Digest,
    pub prompt_bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubmissionRecord {
    pub reservation: InputReservation,
    pub state: DeliveryState,
    pub outcome: Option<SubmissionOutcome>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptionState {
    Requested,
    Acknowledged,
    Unknown,
    Settled,
}

/// All fields are part of approval identity. Digests describe the exact scope
/// and operation without putting prompts, credentials, or tool arguments here.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalChallenge {
    pub challenge_id: Uuid,
    pub requester_id: Uuid,
    pub session_id: Uuid,
    pub controller: ControllerFence,
    pub submission_id: Uuid,
    pub operation_digest: Sha256Digest,
    pub scope_digest: Sha256Digest,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalResolution {
    pub challenge: ApprovalChallenge,
    /// None means terminal settlement expired the challenge, never approval.
    pub decision: Option<ApprovalDecision>,
    pub operator_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum HandoffTransfer {
    /// The caller must have a current certification for the exact adapter.
    SameSession { capability_evidence: Sha256Digest },
    /// A fresh session receiving an attributed, authorized context bundle.
    ContextTransfer { bundle_id: Uuid },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffTarget {
    pub controller_id: Uuid,
    pub session_id: Uuid,
    pub owner: ControllerOwner,
    pub transfer: HandoffTransfer,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeController {
    pub schema_version: u32,
    pub identity: ControllerIdentity,
    pub generation: u64,
    pub version: u64,
    pub owner: ControllerOwner,
    pub phase: ControllerPhase,
    /// Retained after settlement so the latest reservation cannot be replayed.
    pub last_submission: Option<SubmissionRecord>,
    pub interruption: Option<InterruptionState>,
    pub pending_approval: Option<ApprovalChallenge>,
    pub last_approval_resolution: Option<ApprovalResolution>,
    pub handoff: Option<HandoffTarget>,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ControllerError {
    #[error("controller record or command is invalid")]
    Invalid,
    #[error("controller identity or generation is stale")]
    StaleFence,
    #[error("controller version is stale")]
    StaleVersion,
    #[error("actor does not own this operation")]
    ActorNotAllowed,
    #[error("controller phase does not allow this operation")]
    PhaseForbids,
    #[error("another submission or approval remains unresolved")]
    Unsettled,
    #[error("submission identity conflicts or is out of order")]
    SubmissionConflict,
    #[error("possibly delivered input cannot be replayed")]
    DeliveryNotReplayable,
    #[error("approval does not match the exact pending challenge")]
    ApprovalMismatch,
    #[error("handoff requires terminal evidence for the old controller")]
    OldControllerNotTerminal,
    #[error("unsupported resume requires a new session and context bundle")]
    InvalidContextTransfer,
    #[error("controller counter cannot advance")]
    CounterExhausted,
}

impl ControllerError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Invalid => "controller_invalid",
            Self::StaleFence => "controller_stale_fence",
            Self::StaleVersion => "controller_stale_version",
            Self::ActorNotAllowed => "controller_actor_not_allowed",
            Self::PhaseForbids => "controller_phase_forbids",
            Self::Unsettled => "controller_unsettled",
            Self::SubmissionConflict => "controller_submission_conflict",
            Self::DeliveryNotReplayable => "controller_delivery_not_replayable",
            Self::ApprovalMismatch => "controller_approval_mismatch",
            Self::OldControllerNotTerminal => "controller_old_owner_not_terminal",
            Self::InvalidContextTransfer => "controller_invalid_context_transfer",
            Self::CounterExhausted => "controller_counter_exhausted",
        }
    }
}

impl NativeController {
    pub fn new(
        identity: ControllerIdentity,
        owner: ControllerOwner,
    ) -> Result<Self, ControllerError> {
        let record = Self {
            schema_version: CONTROLLER_SCHEMA_VERSION,
            identity,
            generation: 1,
            version: 1,
            owner,
            phase: ControllerPhase::Paused,
            last_submission: None,
            interruption: None,
            pending_approval: None,
            last_approval_resolution: None,
            handoff: None,
        };
        record.validate()?;
        Ok(record)
    }

    #[must_use]
    pub const fn fence(&self) -> ControllerFence {
        ControllerFence {
            controller_id: self.identity.controller_id,
            generation: self.generation,
        }
    }

    #[must_use]
    pub fn has_unsettled_input(&self) -> bool {
        self.last_submission
            .is_some_and(|submission| submission.state != DeliveryState::Settled)
    }

    pub fn validate(&self) -> Result<(), ControllerError> {
        if self.schema_version != CONTROLLER_SCHEMA_VERSION
            || self.identity.workflow_id.is_nil()
            || self.identity.session_id.is_nil()
            || self.identity.controller_id.is_nil()
            || self.identity.contract_version == 0
            || self.generation == 0
            || self.version == 0
            || self.version < self.generation
            || !self.owner.valid()
        {
            return Err(ControllerError::Invalid);
        }
        if let Some(submission) = self.last_submission {
            self.validate_reservation(submission.reservation)?;
            if (submission.state == DeliveryState::Settled) != submission.outcome.is_some() {
                return Err(ControllerError::Invalid);
            }
        } else if self.interruption.is_some() || self.pending_approval.is_some() {
            return Err(ControllerError::Invalid);
        }
        if let Some(approval) = self.pending_approval {
            self.validate_challenge(approval)?;
        }
        if let Some(resolution) = self.last_approval_resolution
            && (resolution.decision.is_some() != resolution.operator_id.is_some()
                || resolution.operator_id.is_some_and(|id| id.is_nil())
                || resolution.challenge.challenge_id.is_nil())
        {
            return Err(ControllerError::Invalid);
        }
        let handing_off = matches!(
            self.phase,
            ControllerPhase::Quiescing | ControllerPhase::AwaitingAttach
        );
        if handing_off != self.handoff.is_some() {
            return Err(ControllerError::Invalid);
        }
        if let Some(target) = self.handoff {
            self.validate_target(target)?;
        }
        if matches!(
            self.phase,
            ControllerPhase::AwaitingAttach | ControllerPhase::Closed
        ) && (self.has_unsettled_input() || self.pending_approval.is_some())
        {
            return Err(ControllerError::Unsettled);
        }
        if self.interruption == Some(InterruptionState::Settled) && self.has_unsettled_input() {
            return Err(ControllerError::Invalid);
        }
        Ok(())
    }

    fn validate_reservation(&self, reservation: InputReservation) -> Result<(), ControllerError> {
        if reservation.submission_id.is_nil()
            || reservation.sequence == 0
            || reservation.prompt_bytes == 0
            || reservation.prompt_bytes > MAX_INPUT_BYTES
            || reservation.contract_id != self.identity.contract_id
        {
            return Err(ControllerError::Invalid);
        }
        Ok(())
    }

    fn validate_challenge(&self, challenge: ApprovalChallenge) -> Result<(), ControllerError> {
        let valid_submission = self.last_submission.is_some_and(|submission| {
            submission.reservation.submission_id == challenge.submission_id
                && matches!(
                    submission.state,
                    DeliveryState::Submitted | DeliveryState::Acknowledged
                )
        });
        if challenge.challenge_id.is_nil()
            || challenge.requester_id.is_nil()
            || challenge.session_id != self.identity.session_id
            || challenge.controller != self.fence()
            || !valid_submission
        {
            return Err(ControllerError::ApprovalMismatch);
        }
        Ok(())
    }

    fn validate_target(&self, target: HandoffTarget) -> Result<(), ControllerError> {
        if target.controller_id.is_nil()
            || target.controller_id == self.identity.controller_id
            || target.session_id.is_nil()
            || !target.owner.valid()
        {
            return Err(ControllerError::Invalid);
        }
        let valid = match target.transfer {
            HandoffTransfer::SameSession { .. } => target.session_id == self.identity.session_id,
            HandoffTransfer::ContextTransfer { bundle_id } => {
                target.session_id != self.identity.session_id && !bundle_id.is_nil()
            }
        };
        if !valid {
            return Err(ControllerError::InvalidContextTransfer);
        }
        Ok(())
    }

    fn check_input_actor(&self, actor: ControllerActor) -> Result<(), ControllerError> {
        let allowed = match (self.owner, actor) {
            (
                ControllerOwner::Structured | ControllerOwner::Native,
                ControllerActor::Automation,
            ) => true,
            (
                ControllerOwner::Human { operator_id },
                ControllerActor::Human {
                    operator_id: actual,
                },
            ) => operator_id == actual,
            _ => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(ControllerError::ActorNotAllowed)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerObservation {
    Terminal,
    Present,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerCommand {
    Pause,
    Resume,
    ReserveSubmission(InputReservation),
    BeginSubmission {
        submission_id: Uuid,
    },
    AcknowledgeSubmission {
        submission_id: Uuid,
    },
    DeliveryUnknown {
        submission_id: Uuid,
    },
    SettleSubmission {
        submission_id: Uuid,
        outcome: SubmissionOutcome,
    },
    RequestInterruption {
        submission_id: Uuid,
    },
    ObserveInterruption {
        submission_id: Uuid,
        acknowledged: bool,
    },
    RaiseApproval(ApprovalChallenge),
    ResolveApproval {
        challenge: ApprovalChallenge,
        decision: ApprovalDecision,
    },
    BeginHandoff(HandoffTarget),
    ObserveOldController(ControllerObservation),
    Attach {
        controller_id: Uuid,
        session_id: Uuid,
    },
    Close,
}

/// `duplicate` means no effect is permitted and no version was consumed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerTransition {
    pub record: NativeController,
    pub duplicate: bool,
}

/// Reduces one authenticated command without performing I/O. The store must
/// atomically persist the returned version and any corresponding outbox effect.
pub fn apply_controller_command(
    record: &NativeController,
    expected_version: u64,
    fence: ControllerFence,
    actor: ControllerActor,
    command: ControllerCommand,
) -> Result<ControllerTransition, ControllerError> {
    record.validate()?;
    if fence != record.fence() {
        return Err(ControllerError::StaleFence);
    }
    if expected_version != record.version {
        return Err(ControllerError::StaleVersion);
    }
    if record.phase == ControllerPhase::Closed {
        return Err(ControllerError::PhaseForbids);
    }
    let adapter_command = matches!(
        command,
        ControllerCommand::AcknowledgeSubmission { .. }
            | ControllerCommand::DeliveryUnknown { .. }
            | ControllerCommand::SettleSubmission { .. }
            | ControllerCommand::ObserveInterruption { .. }
            | ControllerCommand::RaiseApproval(_)
            | ControllerCommand::ObserveOldController(_)
            | ControllerCommand::Attach { .. }
    );
    if adapter_command != (actor == ControllerActor::Adapter)
        || matches!(actor, ControllerActor::Human { operator_id } if operator_id.is_nil())
    {
        return Err(ControllerError::ActorNotAllowed);
    }
    let mut next = record.clone();
    let mut duplicate = false;
    match command {
        ControllerCommand::Pause => {
            if !matches!(
                record.phase,
                ControllerPhase::Active | ControllerPhase::Paused
            ) {
                return Err(ControllerError::PhaseForbids);
            }
            if next.handoff.is_none() {
                next.phase = ControllerPhase::Paused;
            }
        }
        ControllerCommand::Resume => {
            record.check_input_actor(actor)?;
            if record.phase != ControllerPhase::Paused || record.pending_approval.is_some() {
                return Err(ControllerError::PhaseForbids);
            }
            next.phase = ControllerPhase::Active;
        }
        ControllerCommand::ReserveSubmission(reservation) => {
            record.check_input_actor(actor)?;
            record.validate_reservation(reservation)?;
            if record.phase != ControllerPhase::Active {
                return Err(ControllerError::PhaseForbids);
            }
            if let Some(previous) = record.last_submission {
                if previous.reservation.submission_id == reservation.submission_id {
                    if previous.reservation != reservation {
                        return Err(ControllerError::SubmissionConflict);
                    }
                    duplicate = true;
                } else if previous.state != DeliveryState::Settled
                    || record.pending_approval.is_some()
                {
                    return Err(ControllerError::Unsettled);
                }
            }
            if !duplicate {
                let sequence = record
                    .last_submission
                    .map_or(0, |value| value.reservation.sequence)
                    .checked_add(1)
                    .ok_or(ControllerError::CounterExhausted)?;
                if reservation.sequence != sequence {
                    return Err(ControllerError::SubmissionConflict);
                }
                next.last_submission = Some(SubmissionRecord {
                    reservation,
                    state: DeliveryState::Reserved,
                    outcome: None,
                });
                next.interruption = None;
            }
        }
        ControllerCommand::BeginSubmission { submission_id } => {
            record.check_input_actor(actor)?;
            if record.phase != ControllerPhase::Active || record.pending_approval.is_some() {
                return Err(ControllerError::PhaseForbids);
            }
            let submission = matching_submission(&mut next, submission_id)?;
            if submission.state != DeliveryState::Reserved {
                return Err(ControllerError::DeliveryNotReplayable);
            }
            submission.state = DeliveryState::Submitted;
        }
        ControllerCommand::AcknowledgeSubmission { submission_id } => {
            let submission = matching_submission(&mut next, submission_id)?;
            if !matches!(
                submission.state,
                DeliveryState::Submitted | DeliveryState::DeliveryUnknown
            ) {
                return Err(ControllerError::DeliveryNotReplayable);
            }
            submission.state = DeliveryState::Acknowledged;
        }
        ControllerCommand::DeliveryUnknown { submission_id } => {
            if record.pending_approval.is_some() {
                return Err(ControllerError::Unsettled);
            }
            let submission = matching_submission(&mut next, submission_id)?;
            if submission.state != DeliveryState::Submitted {
                return Err(ControllerError::DeliveryNotReplayable);
            }
            submission.state = DeliveryState::DeliveryUnknown;
            if next.handoff.is_none() {
                next.phase = ControllerPhase::Paused;
            }
        }
        ControllerCommand::SettleSubmission {
            submission_id,
            outcome,
        } => {
            let submission = matching_submission(&mut next, submission_id)?;
            let valid = match outcome {
                SubmissionOutcome::Completed => submission.state == DeliveryState::Acknowledged,
                SubmissionOutcome::NotDelivered => matches!(
                    submission.state,
                    DeliveryState::Reserved | DeliveryState::DeliveryUnknown
                ),
                SubmissionOutcome::Cancelled | SubmissionOutcome::Failed => matches!(
                    submission.state,
                    DeliveryState::Submitted
                        | DeliveryState::Acknowledged
                        | DeliveryState::DeliveryUnknown
                ),
            };
            if !valid {
                return Err(ControllerError::DeliveryNotReplayable);
            }
            if record.pending_approval.is_some() && outcome == SubmissionOutcome::Completed {
                return Err(ControllerError::Unsettled);
            }
            submission.state = DeliveryState::Settled;
            submission.outcome = Some(outcome);
            if let Some(challenge) = next.pending_approval.take() {
                next.last_approval_resolution = Some(ApprovalResolution {
                    challenge,
                    decision: None,
                    operator_id: None,
                });
            }
            if next.interruption.is_some() {
                next.interruption = Some(InterruptionState::Settled);
            }
        }
        ControllerCommand::RequestInterruption { submission_id } => {
            let submission = matching_submission(&mut next, submission_id)?;
            if matches!(
                submission.state,
                DeliveryState::Reserved | DeliveryState::Settled
            ) {
                return Err(ControllerError::PhaseForbids);
            }
            duplicate = next.interruption.is_some();
            if !duplicate {
                next.interruption = Some(InterruptionState::Requested);
            }
            if next.phase != ControllerPhase::Quiescing {
                next.phase = ControllerPhase::Paused;
            }
        }
        ControllerCommand::ObserveInterruption {
            submission_id,
            acknowledged,
        } => {
            matching_submission(&mut next, submission_id)?;
            if !matches!(
                next.interruption,
                Some(InterruptionState::Requested | InterruptionState::Unknown)
            ) {
                return Err(ControllerError::PhaseForbids);
            }
            next.interruption = Some(if acknowledged {
                InterruptionState::Acknowledged
            } else {
                InterruptionState::Unknown
            });
        }
        ControllerCommand::RaiseApproval(challenge) => {
            record.validate_challenge(challenge)?;
            if let Some(existing) = record.pending_approval {
                if existing != challenge {
                    return Err(ControllerError::ApprovalMismatch);
                }
                duplicate = true;
            } else if record
                .last_approval_resolution
                .is_some_and(|value| value.challenge.challenge_id == challenge.challenge_id)
            {
                return Err(ControllerError::ApprovalMismatch);
            } else {
                next.pending_approval = Some(challenge);
            }
        }
        ControllerCommand::ResolveApproval {
            challenge,
            decision,
        } => {
            let ControllerActor::Human { operator_id } = actor else {
                return Err(ControllerError::ActorNotAllowed);
            };
            if record.pending_approval != Some(challenge) {
                return Err(ControllerError::ApprovalMismatch);
            }
            record.validate_challenge(challenge)?;
            next.pending_approval = None;
            next.last_approval_resolution = Some(ApprovalResolution {
                challenge,
                decision: Some(decision),
                operator_id: Some(operator_id),
            });
        }
        ControllerCommand::BeginHandoff(target) => {
            if !matches!(
                record.phase,
                ControllerPhase::Active | ControllerPhase::Paused
            ) {
                return Err(ControllerError::PhaseForbids);
            }
            if matches!(record.owner, ControllerOwner::Human { .. }) {
                record.check_input_actor(actor)?;
            }
            if let ControllerOwner::Human { operator_id } = target.owner
                && actor != (ControllerActor::Human { operator_id })
            {
                return Err(ControllerError::ActorNotAllowed);
            }
            record.validate_target(target)?;
            next.handoff = Some(target);
            next.phase = ControllerPhase::Quiescing;
        }
        ControllerCommand::ObserveOldController(observation) => {
            if record.phase != ControllerPhase::Quiescing {
                return Err(ControllerError::PhaseForbids);
            }
            if observation != ControllerObservation::Terminal {
                return Err(ControllerError::OldControllerNotTerminal);
            }
            if record.has_unsettled_input() || record.pending_approval.is_some() {
                return Err(ControllerError::Unsettled);
            }
            next.phase = ControllerPhase::AwaitingAttach;
        }
        ControllerCommand::Attach {
            controller_id,
            session_id,
        } => {
            if record.phase != ControllerPhase::AwaitingAttach {
                return Err(ControllerError::OldControllerNotTerminal);
            }
            let target = record.handoff.ok_or(ControllerError::Invalid)?;
            if controller_id != target.controller_id || session_id != target.session_id {
                return Err(ControllerError::StaleFence);
            }
            next.generation = record
                .generation
                .checked_add(1)
                .ok_or(ControllerError::CounterExhausted)?;
            next.identity.controller_id = target.controller_id;
            next.identity.session_id = target.session_id;
            next.owner = target.owner;
            next.phase = ControllerPhase::Paused;
            next.handoff = None;
        }
        ControllerCommand::Close => {
            record.check_input_actor(actor)?;
            if record.has_unsettled_input()
                || record.pending_approval.is_some()
                || record.handoff.is_some()
            {
                return Err(ControllerError::Unsettled);
            }
            next.phase = ControllerPhase::Closed;
        }
    }
    if duplicate {
        return Ok(ControllerTransition {
            record: record.clone(),
            duplicate: true,
        });
    }
    next.version = record
        .version
        .checked_add(1)
        .ok_or(ControllerError::CounterExhausted)?;
    next.validate()?;
    Ok(ControllerTransition {
        record: next,
        duplicate: false,
    })
}

fn matching_submission(
    record: &mut NativeController,
    submission_id: Uuid,
) -> Result<&mut SubmissionRecord, ControllerError> {
    record
        .last_submission
        .as_mut()
        .filter(|submission| submission.reservation.submission_id == submission_id)
        .ok_or(ControllerError::SubmissionConflict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller() -> NativeController {
        let identity = ControllerIdentity {
            workflow_id: Uuid::from_u128(1),
            session_id: Uuid::from_u128(2),
            controller_id: Uuid::from_u128(3),
            contract_id: Sha256Digest::of(b"contract"),
            contract_version: 1,
        };
        let record =
            NativeController::new(identity, ControllerOwner::Structured).expect("controller");
        change(
            &record,
            ControllerActor::Automation,
            ControllerCommand::Resume,
        )
    }

    fn change(
        record: &NativeController,
        actor: ControllerActor,
        command: ControllerCommand,
    ) -> NativeController {
        apply_controller_command(record, record.version, record.fence(), actor, command)
            .expect("transition")
            .record
    }

    fn reservation(record: &NativeController) -> InputReservation {
        InputReservation {
            submission_id: Uuid::from_u128(4),
            sequence: 1,
            contract_id: record.identity.contract_id,
            prompt_digest: Sha256Digest::of(b"literal prompt"),
            prompt_bytes: 14,
        }
    }

    fn submitted() -> NativeController {
        let record = controller();
        let input = reservation(&record);
        let record = change(
            &record,
            ControllerActor::Automation,
            ControllerCommand::ReserveSubmission(input),
        );
        change(
            &record,
            ControllerActor::Automation,
            ControllerCommand::BeginSubmission {
                submission_id: input.submission_id,
            },
        )
    }

    #[test]
    fn uncertain_input_is_never_replayed_or_completed_without_acknowledgment() {
        let record = submitted();
        let id = reservation(&record).submission_id;
        let unknown = change(
            &record,
            ControllerActor::Adapter,
            ControllerCommand::DeliveryUnknown { submission_id: id },
        );
        assert_eq!(unknown.phase, ControllerPhase::Paused);
        let resumed = change(
            &unknown,
            ControllerActor::Automation,
            ControllerCommand::Resume,
        );
        assert_eq!(
            apply_controller_command(
                &resumed,
                resumed.version,
                resumed.fence(),
                ControllerActor::Automation,
                ControllerCommand::BeginSubmission { submission_id: id }
            ),
            Err(ControllerError::DeliveryNotReplayable)
        );
        assert_eq!(
            apply_controller_command(
                &resumed,
                resumed.version,
                resumed.fence(),
                ControllerActor::Adapter,
                ControllerCommand::SettleSubmission {
                    submission_id: id,
                    outcome: SubmissionOutcome::Completed
                }
            ),
            Err(ControllerError::DeliveryNotReplayable)
        );
        let duplicate = apply_controller_command(
            &resumed,
            resumed.version,
            resumed.fence(),
            ControllerActor::Automation,
            ControllerCommand::ReserveSubmission(reservation(&record)),
        )
        .expect("duplicate");
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.record.version, resumed.version);
        let mut different = reservation(&record);
        different.prompt_digest = Sha256Digest::of(b"different");
        assert_eq!(
            apply_controller_command(
                &resumed,
                resumed.version,
                resumed.fence(),
                ControllerActor::Automation,
                ControllerCommand::ReserveSubmission(different)
            ),
            Err(ControllerError::SubmissionConflict)
        );
        let ack = change(
            &resumed,
            ControllerActor::Adapter,
            ControllerCommand::AcknowledgeSubmission { submission_id: id },
        );
        let settled = change(
            &ack,
            ControllerActor::Adapter,
            ControllerCommand::SettleSubmission {
                submission_id: id,
                outcome: SubmissionOutcome::Completed,
            },
        );
        assert!(!settled.has_unsettled_input());
    }

    #[test]
    fn interruption_acknowledgment_is_not_terminal_evidence() {
        let record = submitted();
        let id = reservation(&record).submission_id;
        let record = change(
            &record,
            ControllerActor::Automation,
            ControllerCommand::RequestInterruption { submission_id: id },
        );
        let record = change(
            &record,
            ControllerActor::Adapter,
            ControllerCommand::ObserveInterruption {
                submission_id: id,
                acknowledged: true,
            },
        );
        assert!(record.has_unsettled_input());
        assert_eq!(
            apply_controller_command(
                &record,
                record.version,
                record.fence(),
                ControllerActor::Automation,
                ControllerCommand::Close
            ),
            Err(ControllerError::Unsettled)
        );
        let record = change(
            &record,
            ControllerActor::Adapter,
            ControllerCommand::SettleSubmission {
                submission_id: id,
                outcome: SubmissionOutcome::Cancelled,
            },
        );
        assert_eq!(record.interruption, Some(InterruptionState::Settled));
    }

    #[test]
    fn approval_requires_the_exact_challenge_and_a_human_actor() {
        let record = submitted();
        let challenge = ApprovalChallenge {
            challenge_id: Uuid::from_u128(5),
            requester_id: Uuid::from_u128(6),
            session_id: record.identity.session_id,
            controller: record.fence(),
            submission_id: reservation(&record).submission_id,
            operation_digest: Sha256Digest::of(b"operation"),
            scope_digest: Sha256Digest::of(b"scope"),
        };
        let record = change(
            &record,
            ControllerActor::Adapter,
            ControllerCommand::RaiseApproval(challenge),
        );
        let command = ControllerCommand::ResolveApproval {
            challenge,
            decision: ApprovalDecision::Allow,
        };
        assert_eq!(
            apply_controller_command(
                &record,
                record.version,
                record.fence(),
                ControllerActor::Automation,
                command
            ),
            Err(ControllerError::ActorNotAllowed)
        );
        let human = ControllerActor::Human {
            operator_id: Uuid::from_u128(7),
        };
        let mut altered = challenge;
        altered.scope_digest = Sha256Digest::of(b"broader");
        assert_eq!(
            apply_controller_command(
                &record,
                record.version,
                record.fence(),
                human,
                ControllerCommand::ResolveApproval {
                    challenge: altered,
                    decision: ApprovalDecision::Allow
                }
            ),
            Err(ControllerError::ApprovalMismatch)
        );
        let resolved = change(&record, human, command);
        assert!(resolved.pending_approval.is_none());
        assert_eq!(
            apply_controller_command(
                &resolved,
                resolved.version,
                resolved.fence(),
                human,
                command
            ),
            Err(ControllerError::ApprovalMismatch)
        );
    }

    #[test]
    fn handoff_requires_terminal_old_owner_and_changes_generation() {
        let record = controller();
        let target = HandoffTarget {
            controller_id: Uuid::from_u128(8),
            session_id: record.identity.session_id,
            owner: ControllerOwner::Native,
            transfer: HandoffTransfer::SameSession {
                capability_evidence: Sha256Digest::of(b"certification"),
            },
        };
        let pending = change(
            &record,
            ControllerActor::Automation,
            ControllerCommand::BeginHandoff(target),
        );
        let attach = ControllerCommand::Attach {
            controller_id: target.controller_id,
            session_id: target.session_id,
        };
        assert_eq!(
            apply_controller_command(
                &pending,
                pending.version,
                pending.fence(),
                ControllerActor::Adapter,
                attach
            ),
            Err(ControllerError::OldControllerNotTerminal)
        );
        assert_eq!(
            apply_controller_command(
                &pending,
                pending.version,
                pending.fence(),
                ControllerActor::Adapter,
                ControllerCommand::ObserveOldController(ControllerObservation::Unknown)
            ),
            Err(ControllerError::OldControllerNotTerminal)
        );
        let stopped = change(
            &pending,
            ControllerActor::Adapter,
            ControllerCommand::ObserveOldController(ControllerObservation::Terminal),
        );
        let attached = change(&stopped, ControllerActor::Adapter, attach);
        assert_eq!(attached.generation, record.generation + 1);
        assert_eq!(attached.phase, ControllerPhase::Paused);
        assert_eq!(
            apply_controller_command(
                &attached,
                attached.version,
                record.fence(),
                ControllerActor::Automation,
                ControllerCommand::Resume
            ),
            Err(ControllerError::StaleFence)
        );
    }

    #[test]
    fn context_transfer_uses_a_fresh_session_and_human_owner_excludes_automation() {
        let record = controller();
        let operator_id = Uuid::from_u128(7);
        let human = ControllerActor::Human { operator_id };
        let mut target = HandoffTarget {
            controller_id: Uuid::from_u128(8),
            session_id: record.identity.session_id,
            owner: ControllerOwner::Human { operator_id },
            transfer: HandoffTransfer::ContextTransfer {
                bundle_id: Uuid::from_u128(9),
            },
        };
        assert_eq!(
            apply_controller_command(
                &record,
                record.version,
                record.fence(),
                human,
                ControllerCommand::BeginHandoff(target)
            ),
            Err(ControllerError::InvalidContextTransfer)
        );
        target.session_id = Uuid::from_u128(10);
        let record = change(&record, human, ControllerCommand::BeginHandoff(target));
        let record = change(
            &record,
            ControllerActor::Adapter,
            ControllerCommand::ObserveOldController(ControllerObservation::Terminal),
        );
        let record = change(
            &record,
            ControllerActor::Adapter,
            ControllerCommand::Attach {
                controller_id: target.controller_id,
                session_id: target.session_id,
            },
        );
        assert_eq!(
            apply_controller_command(
                &record,
                record.version,
                record.fence(),
                ControllerActor::Automation,
                ControllerCommand::Resume
            ),
            Err(ControllerError::ActorNotAllowed)
        );
        let record = change(&record, human, ControllerCommand::Resume);
        let input = reservation(&record);
        let automation_target = HandoffTarget {
            controller_id: Uuid::from_u128(11),
            session_id: record.identity.session_id,
            owner: ControllerOwner::Structured,
            transfer: HandoffTransfer::SameSession {
                capability_evidence: Sha256Digest::of(b"resume certification"),
            },
        };
        assert_eq!(
            apply_controller_command(
                &record,
                record.version,
                record.fence(),
                ControllerActor::Automation,
                ControllerCommand::BeginHandoff(automation_target),
            ),
            Err(ControllerError::ActorNotAllowed),
        );
        assert_eq!(
            apply_controller_command(
                &record,
                record.version,
                record.fence(),
                ControllerActor::Automation,
                ControllerCommand::ReserveSubmission(input)
            ),
            Err(ControllerError::ActorNotAllowed)
        );
    }

    #[test]
    fn deserialized_invalid_records_and_stale_versions_are_refused() {
        let record = controller();
        assert_eq!(
            apply_controller_command(
                &record,
                record.version - 1,
                record.fence(),
                ControllerActor::Automation,
                ControllerCommand::Pause
            ),
            Err(ControllerError::StaleVersion)
        );
        let mut invalid = record.clone();
        invalid.generation = 0;
        let encoded = serde_json::to_vec(&invalid).expect("serialize");
        let decoded: NativeController = serde_json::from_slice(&encoded).expect("shape");
        assert_eq!(decoded.validate(), Err(ControllerError::Invalid));
        let mut exhausted = record;
        exhausted.version = u64::MAX;
        assert_eq!(
            apply_controller_command(
                &exhausted,
                exhausted.version,
                exhausted.fence(),
                ControllerActor::Automation,
                ControllerCommand::Pause
            ),
            Err(ControllerError::CounterExhausted)
        );
    }
}
