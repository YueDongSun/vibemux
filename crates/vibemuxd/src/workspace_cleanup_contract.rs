//! Closed cleanup requests and bounded public projections. No paths or OS IDs.
use serde::{Deserialize, Serialize};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_types::{
    ProjectId, RunId, TaskId, WorkspaceCleanupId,
    workspace_cleanup::{WorkspaceCleanupOutcome, WorkspaceCleanupPhase},
};

pub const WORKSPACE_CLEANUP_VIEW_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanupScope {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
}
impl WorkspaceCleanupScope {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.project_id.as_uuid().is_nil()
            || self.task_id.as_uuid().is_nil()
            || self.run_id.as_uuid().is_nil()
        {
            Err("workspace_cleanup_invalid_request")
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceCleanupRequest {
    Plan {
        scope: WorkspaceCleanupScope,
        operation_id: Uuid,
    },
    Confirm {
        scope: WorkspaceCleanupScope,
        operation_id: Uuid,
        cleanup_id: WorkspaceCleanupId,
        plan_version: u64,
    },
    Status {
        scope: WorkspaceCleanupScope,
        cleanup_id: Option<WorkspaceCleanupId>,
    },
    Reconcile {
        scope: WorkspaceCleanupScope,
        operation_id: Uuid,
        cleanup_id: WorkspaceCleanupId,
        expected_version: u64,
    },
}
impl WorkspaceCleanupRequest {
    pub fn scope(&self) -> WorkspaceCleanupScope {
        match self {
            Self::Plan { scope, .. }
            | Self::Confirm { scope, .. }
            | Self::Status { scope, .. }
            | Self::Reconcile { scope, .. } => *scope,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        self.scope().validate()?;
        let valid = match self {
            Self::Plan { operation_id, .. } => !operation_id.is_nil(),
            Self::Confirm {
                operation_id,
                cleanup_id,
                plan_version,
                ..
            } => !operation_id.is_nil() && !cleanup_id.as_uuid().is_nil() && *plan_version == 1,
            Self::Status { cleanup_id, .. } => cleanup_id.is_none_or(|id| !id.as_uuid().is_nil()),
            Self::Reconcile {
                operation_id,
                cleanup_id,
                expected_version,
                ..
            } => {
                !operation_id.is_nil()
                    && !cleanup_id.as_uuid().is_nil()
                    && (1..=i64::MAX as u64).contains(expected_version)
            }
        };
        if valid {
            Ok(())
        } else {
            Err("workspace_cleanup_invalid_request")
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanupView {
    pub schema_version: u32,
    pub scope: WorkspaceCleanupScope,
    pub cleanup_id: WorkspaceCleanupId,
    pub phase: WorkspaceCleanupPhase,
    pub version: u64,
    pub plan_version: u64,
    pub branch: String,
    pub can_confirm: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub outcome: Option<WorkspaceCleanupOutcome>,
    pub error_code: Option<String>,
}
impl fmt::Debug for WorkspaceCleanupView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceCleanupView")
            .field("cleanup_id", &self.cleanup_id)
            .field("phase", &self.phase)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}
impl WorkspaceCleanupView {
    pub fn validate_for(&self, scope: WorkspaceCleanupScope) -> Result<(), &'static str> {
        scope.validate()?;
        if self.schema_version != WORKSPACE_CLEANUP_VIEW_VERSION
            || self.scope != scope
            || self.cleanup_id.as_uuid().is_nil()
            || !(1..=i64::MAX as u64).contains(&self.version)
            || self.plan_version != 1
            || self.branch != format!("codex/run_{}", scope.run_id.as_uuid().simple())
            || self.branch.len() > 1024
            || self.branch.chars().any(char::is_control)
            || (self.can_confirm
                && (self.phase != WorkspaceCleanupPhase::Planned || self.version != 1))
            || self.error_code.as_ref().is_some_and(|code| {
                code.is_empty()
                    || code.len() > 96
                    || !code
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
        {
            return Err("workspace_cleanup_invalid_response");
        }
        let phase_valid = match self.phase {
            WorkspaceCleanupPhase::Removed => {
                self.error_code.is_none()
                    && matches!(
                        self.outcome,
                        Some(
                            WorkspaceCleanupOutcome::RemovedExecuted
                                | WorkspaceCleanupOutcome::RemovedObserved
                        )
                    )
            }
            WorkspaceCleanupPhase::Refused => {
                self.error_code.is_some() && self.outcome == Some(WorkspaceCleanupOutcome::Refused)
            }
            WorkspaceCleanupPhase::Unknown => self.error_code.is_some() && self.outcome.is_none(),
            _ => self.error_code.is_none() && self.outcome.is_none(),
        };
        if !phase_valid {
            return Err("workspace_cleanup_invalid_response");
        }
        Ok(())
    }
}
