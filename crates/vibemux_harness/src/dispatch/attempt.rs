//! Durable attempt phases and their canonical Task/Run mapping (ADR 029 §3).
//!
//! The store applies [`apply`] inside the same transaction that appends the
//! matching event, and rejects every edge this table does not list. The
//! table also fixes the claim-fence and reservation effects, so the store
//! never has to infer them.

use serde::{Deserialize, Serialize};
use vibemux_types::{RunCompletionAuthority, RunStatus, TaskStatus};

use super::{AttemptOutcome, DispatchError, Sha256Digest, digest::hash_fields};

const RESOURCE_KEY_DOMAIN: &str = "vibemux.harness_dispatch.resource.v1";

/// Canonical Task/Run statuses written at admission.
pub const ADMISSION_TASK_STATUS: TaskStatus = TaskStatus::InProgress;
pub const ADMISSION_RUN_STATUS: RunStatus = RunStatus::Preparing;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchPhase {
    /// Durably admitted; no process launched yet.
    Admitted,
    /// Claimed with a fence; a process may be running.
    Running,
    /// Cancellation committed while running; awaiting the fenced finish.
    CancelRequested,
    /// The daemon restarted while a process may have run; needs an operator.
    RecoveryPending,
    Completed,
    Failed,
    Unverified,
    Cancelled,
}

impl DispatchPhase {
    pub const ALL: [Self; 8] = [
        Self::Admitted,
        Self::Running,
        Self::CancelRequested,
        Self::RecoveryPending,
        Self::Completed,
        Self::Failed,
        Self::Unverified,
        Self::Cancelled,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Running => "running",
            Self::CancelRequested => "cancel_requested",
            Self::RecoveryPending => "recovery_pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unverified => "unverified",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|phase| phase.as_str() == name)
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Unverified | Self::Cancelled
        )
    }

    /// Whether this phase holds the working-directory reservation.
    #[must_use]
    pub const fn holds_reservation(self) -> bool {
        !self.is_terminal()
    }

    /// Task/Run statuses of a non-terminal phase. Terminal phases can be
    /// reached along different edges (for example, `Cancelled` with Run
    /// `failed` or `stopped`), so they have no single canonical pair.
    #[must_use]
    pub const fn canonical_statuses(self) -> Option<(TaskStatus, RunStatus)> {
        match self {
            Self::Admitted => Some((TaskStatus::InProgress, RunStatus::Preparing)),
            Self::Running | Self::CancelRequested => {
                Some((TaskStatus::InProgress, RunStatus::Running))
            }
            Self::RecoveryPending => Some((TaskStatus::Blocked, RunStatus::Stale)),
            Self::Completed | Self::Failed | Self::Unverified | Self::Cancelled => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchTrigger {
    /// Single-use claim before spawning the process.
    Claim,
    /// Fenced finish with the decided outcome.
    Finish(AttemptOutcome),
    /// Operator cancellation (also the acknowledgement for recovery).
    Cancel,
    /// Daemon startup recovery of an attempt left non-terminal.
    Recover,
}

/// What the store must do with the claim fence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FenceEffect {
    /// Leave the fence as it is.
    Keep,
    /// Mint a fresh fence and return it to the claiming executor.
    Mint,
    /// The caller must present the current fence.
    Require,
    /// Invalidate the fence so a stale executor can never finish.
    Clear,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhaseTransition {
    pub from: DispatchPhase,
    pub to: DispatchPhase,
    /// New Task status, when it changes.
    pub task_status: Option<TaskStatus>,
    /// New Run status, when it changes.
    pub run_status: Option<RunStatus>,
    /// Required exactly when `run_status` is `Succeeded`.
    pub completion_authority: Option<RunCompletionAuthority>,
    pub fence: FenceEffect,
    pub releases_reservation: bool,
    /// Fixed code recorded with the transition (for example, `Interrupted`).
    pub error_code: Option<DispatchError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionResult {
    Changed(PhaseTransition),
    /// Idempotent repeat; nothing to write.
    Unchanged,
}

/// Applies `trigger` to an attempt in `from`.
pub fn apply(
    from: DispatchPhase,
    trigger: DispatchTrigger,
) -> Result<TransitionResult, DispatchError> {
    use DispatchPhase as P;
    use DispatchTrigger as T;

    let changed = |to: DispatchPhase,
                   task_status: Option<TaskStatus>,
                   run_status: Option<RunStatus>,
                   fence: FenceEffect,
                   error_code: Option<DispatchError>| {
        TransitionResult::Changed(PhaseTransition {
            from,
            to,
            task_status,
            run_status,
            completion_authority: matches!(run_status, Some(RunStatus::Succeeded))
                .then_some(RunCompletionAuthority::StructuredAdapter),
            fence,
            releases_reservation: from.holds_reservation() && !to.holds_reservation(),
            error_code,
        })
    };

    let result = match (from, trigger) {
        (_, T::Finish(AttemptOutcome::Probed)) => return Err(DispatchError::InvalidTransition),

        (P::Admitted, T::Claim) => changed(
            P::Running,
            None,
            Some(RunStatus::Running),
            FenceEffect::Mint,
            None,
        ),
        // There is no preparing -> stopped edge; nothing ran, so the Run failed.
        (P::Admitted, T::Cancel) => changed(
            P::Cancelled,
            Some(TaskStatus::Cancelled),
            Some(RunStatus::Failed),
            FenceEffect::Keep,
            None,
        ),
        (P::Admitted, T::Recover) => changed(
            P::Failed,
            Some(TaskStatus::Blocked),
            Some(RunStatus::Failed),
            FenceEffect::Keep,
            Some(DispatchError::Interrupted),
        ),
        (P::Admitted, T::Finish(_)) => return Err(DispatchError::InvalidTransition),

        (P::Running, T::Finish(outcome)) => {
            let (to, task, run) = finished_statuses(outcome);
            changed(to, Some(task), Some(run), FenceEffect::Require, None)
        }
        (P::Running, T::Cancel) => changed(P::CancelRequested, None, None, FenceEffect::Keep, None),
        // Cancellation overrides whatever the late finish reports.
        (P::CancelRequested, T::Finish(_)) => changed(
            P::Cancelled,
            Some(TaskStatus::Cancelled),
            Some(RunStatus::Stopped),
            FenceEffect::Require,
            None,
        ),
        (P::CancelRequested, T::Cancel) => TransitionResult::Unchanged,
        (P::Running | P::CancelRequested, T::Recover) => changed(
            P::RecoveryPending,
            Some(TaskStatus::Blocked),
            Some(RunStatus::Stale),
            FenceEffect::Clear,
            Some(DispatchError::Interrupted),
        ),
        (P::Running | P::CancelRequested | P::RecoveryPending, T::Claim) => {
            return Err(DispatchError::InvalidTransition);
        }

        (P::RecoveryPending, T::Cancel) => changed(
            P::Cancelled,
            Some(TaskStatus::Cancelled),
            Some(RunStatus::Stopped),
            FenceEffect::Keep,
            None,
        ),
        (P::RecoveryPending, T::Recover) => TransitionResult::Unchanged,
        (P::RecoveryPending, T::Finish(_)) => return Err(DispatchError::StaleFence),

        (P::Cancelled, T::Cancel) => TransitionResult::Unchanged,
        (P::Completed | P::Failed | P::Unverified, T::Cancel) => {
            return Err(DispatchError::Terminal);
        }
        (P::Completed | P::Failed | P::Unverified | P::Cancelled, T::Recover) => {
            TransitionResult::Unchanged
        }
        (P::Completed | P::Failed | P::Unverified | P::Cancelled, T::Finish(_)) => {
            return Err(DispatchError::StaleFence);
        }
        (P::Completed | P::Failed | P::Unverified | P::Cancelled, T::Claim) => {
            return Err(DispatchError::Terminal);
        }
    };
    Ok(result)
}

fn finished_statuses(outcome: AttemptOutcome) -> (DispatchPhase, TaskStatus, RunStatus) {
    match outcome {
        AttemptOutcome::Completed => (
            DispatchPhase::Completed,
            TaskStatus::Done,
            RunStatus::Succeeded,
        ),
        AttemptOutcome::Failed | AttemptOutcome::Probed => (
            DispatchPhase::Failed,
            TaskStatus::Blocked,
            RunStatus::Failed,
        ),
        AttemptOutcome::Unverified => (
            DispatchPhase::Unverified,
            TaskStatus::Blocked,
            RunStatus::Failed,
        ),
        AttemptOutcome::Cancelled => (
            DispatchPhase::Cancelled,
            TaskStatus::Cancelled,
            RunStatus::Stopped,
        ),
    }
}

/// Reservation key for a canonical working directory. The store keeps only
/// this hash, never the path.
#[must_use]
pub fn resource_key(canonical_working_directory: &str) -> Sha256Digest {
    hash_fields(
        RESOURCE_KEY_DOMAIN,
        &[canonical_working_directory.as_bytes()],
    )
}
