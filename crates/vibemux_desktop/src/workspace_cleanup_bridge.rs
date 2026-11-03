//! Main-only native cleanup input. The renderer supplies identifiers, never paths.
use crate::command_error::CommandError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vibemux_client::daemon_session::DaemonSession;
use vibemux_types::{RunId, TaskId, WorkspaceCleanupId};
use vibemuxd::control::{
    ControlClient, WorkspaceCleanupRequest, WorkspaceCleanupScope, WorkspaceCleanupView,
};
use vibemuxd::process::DaemonPaths;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupTarget {
    pub registration_key: String,
    pub task_id: TaskId,
    pub run_id: RunId,
}
impl CleanupTarget {
    pub(crate) fn validate(&self) -> Result<(), CommandError> {
        if self.registration_key.is_empty()
            || self.registration_key.len() > 256
            || self.registration_key.chars().any(char::is_control)
            || self.task_id.as_uuid().is_nil()
            || self.run_id.as_uuid().is_nil()
        {
            Err(failure("invalid_arguments"))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeCleanupRequest {
    Plan {
        registration_key: String,
        task_id: TaskId,
        run_id: RunId,
        operation_id: Uuid,
    },
    Confirm {
        registration_key: String,
        task_id: TaskId,
        run_id: RunId,
        operation_id: Uuid,
        cleanup_id: WorkspaceCleanupId,
        plan_version: u64,
    },
    Status {
        registration_key: String,
        task_id: TaskId,
        run_id: RunId,
        cleanup_id: Option<WorkspaceCleanupId>,
    },
    Reconcile {
        registration_key: String,
        task_id: TaskId,
        run_id: RunId,
        operation_id: Uuid,
        cleanup_id: WorkspaceCleanupId,
        expected_version: u64,
    },
}
impl NativeCleanupRequest {
    pub fn target(&self) -> CleanupTarget {
        let (registration_key, task_id, run_id) = match self {
            Self::Plan {
                registration_key,
                task_id,
                run_id,
                ..
            }
            | Self::Confirm {
                registration_key,
                task_id,
                run_id,
                ..
            }
            | Self::Status {
                registration_key,
                task_id,
                run_id,
                ..
            }
            | Self::Reconcile {
                registration_key,
                task_id,
                run_id,
                ..
            } => (registration_key, task_id, run_id),
        };
        CleanupTarget {
            registration_key: registration_key.clone(),
            task_id: *task_id,
            run_id: *run_id,
        }
    }
    pub(crate) fn core(
        self,
        scope: WorkspaceCleanupScope,
    ) -> Result<WorkspaceCleanupRequest, CommandError> {
        let target = self.target();
        target.validate()?;
        if scope.task_id != target.task_id || scope.run_id != target.run_id {
            return Err(failure("invalid_arguments"));
        }
        let request = match self {
            Self::Plan { operation_id, .. } => WorkspaceCleanupRequest::Plan {
                scope,
                operation_id,
            },
            Self::Confirm {
                operation_id,
                cleanup_id,
                plan_version,
                ..
            } => WorkspaceCleanupRequest::Confirm {
                scope,
                operation_id,
                cleanup_id,
                plan_version,
            },
            Self::Status { cleanup_id, .. } => {
                WorkspaceCleanupRequest::Status { scope, cleanup_id }
            }
            Self::Reconcile {
                operation_id,
                cleanup_id,
                expected_version,
                ..
            } => WorkspaceCleanupRequest::Reconcile {
                scope,
                operation_id,
                cleanup_id,
                expected_version,
            },
        };
        request.validate().map_err(failure)?;
        Ok(request)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct NativeCleanupReply {
    pub target: CleanupTarget,
    pub view: Option<WorkspaceCleanupView>,
}

pub(crate) trait CleanupPort {
    fn request(
        &mut self,
        request: WorkspaceCleanupRequest,
    ) -> Result<Option<WorkspaceCleanupView>, CommandError>;
}
pub(crate) struct ControlCleanupPort {
    client: ControlClient,
    runtime: tokio::runtime::Runtime,
}
impl ControlCleanupPort {
    pub(crate) fn connect(paths: &DaemonPaths) -> Result<Self, CommandError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| failure("workspace_cleanup_unavailable"))?;
        let (client, _) = DaemonSession::default()
            .connect(paths)
            .map_err(|code| failure(&code))?;
        Ok(Self { client, runtime })
    }
}
impl CleanupPort for ControlCleanupPort {
    fn request(
        &mut self,
        request: WorkspaceCleanupRequest,
    ) -> Result<Option<WorkspaceCleanupView>, CommandError> {
        self.runtime
            .block_on(self.client.workspace_cleanup(request))
            .map_err(|error| failure(error.code()))
    }
}

pub(crate) fn perform(
    request: NativeCleanupRequest,
    scope: WorkspaceCleanupScope,
    port: &mut impl CleanupPort,
    current: impl Fn() -> bool,
) -> Result<NativeCleanupReply, CommandError> {
    if !current() {
        return Err(failure("workspace_cleanup_caller_unavailable"));
    }
    let target = request.target();
    let request = request.core(scope)?;
    let expected = match &request {
        WorkspaceCleanupRequest::Plan { .. } => None,
        WorkspaceCleanupRequest::Status { cleanup_id, .. } => *cleanup_id,
        WorkspaceCleanupRequest::Confirm { cleanup_id, .. }
        | WorkspaceCleanupRequest::Reconcile { cleanup_id, .. } => Some(*cleanup_id),
    };
    let allow_empty = matches!(&request, WorkspaceCleanupRequest::Status { .. });
    let view = port.request(request)?;
    if !current() {
        return Err(failure("workspace_cleanup_caller_unavailable"));
    }
    if let Some(view) = &view {
        view.validate_for(scope).map_err(failure)?;
        if expected.is_some_and(|id| id != view.cleanup_id) {
            return Err(failure("workspace_cleanup_invalid_response"));
        }
    } else if !allow_empty {
        return Err(failure("workspace_cleanup_invalid_response"));
    }
    Ok(NativeCleanupReply { target, view })
}
pub(crate) fn failure(code: &str) -> CommandError {
    let code = if !code.is_empty()
        && code.len() <= 96
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        code
    } else {
        "workspace_cleanup_unconfirmed"
    };
    CommandError::new(
        code,
        "The worktree removal outcome could not be confirmed. Refresh its status before another action.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    struct Port {
        calls: usize,
    }
    impl CleanupPort for Port {
        fn request(
            &mut self,
            _: WorkspaceCleanupRequest,
        ) -> Result<Option<WorkspaceCleanupView>, CommandError> {
            self.calls += 1;
            Ok(None)
        }
    }
    #[test]
    fn invalid_target_and_closed_caller_never_forward_cleanup() {
        let scope = WorkspaceCleanupScope {
            project_id: vibemux_types::ProjectId::new(),
            task_id: TaskId::new(),
            run_id: RunId::new(),
        };
        let make = || NativeCleanupRequest::Status {
            registration_key: "registered".into(),
            task_id: scope.task_id,
            run_id: scope.run_id,
            cleanup_id: None,
        };
        let mut port = Port { calls: 0 };
        assert_eq!(
            perform(make(), scope, &mut port, || false)
                .expect_err("caller closed")
                .code,
            "workspace_cleanup_caller_unavailable"
        );
        assert_eq!(port.calls, 0);
        let different = WorkspaceCleanupScope {
            run_id: RunId::new(),
            ..scope
        };
        assert!(perform(make(), different, &mut port, || true).is_err());
        assert_eq!(port.calls, 0);
        let calls = Cell::new(0);
        assert_eq!(
            perform(make(), scope, &mut port, || {
                let before = calls.get();
                calls.set(before + 1);
                before == 0
            })
            .expect_err("lost after accepted read")
            .code,
            "workspace_cleanup_caller_unavailable"
        );
        assert_eq!(port.calls, 1);
    }
}
