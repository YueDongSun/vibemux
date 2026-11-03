//! Control v16 closed cleanup actions. The service owns accepted continuations.
use super::*;

const CLEANUP_REQUEST_DEADLINE: Duration = Duration::from_secs(30);
const MAX_CLEANUP_ARGUMENT_BYTES: usize = 2048;

impl ControlClient {
    pub async fn workspace_cleanup(
        &self,
        request: WorkspaceCleanupRequest,
    ) -> Result<Option<WorkspaceCleanupView>, ControlError> {
        self.require_operation(ControlOperation::WorkspaceCleanup)?;
        request
            .validate()
            .map_err(|_| ControlError::InvalidRequest)?;
        let scope = request.scope();
        let argument = serde_json::to_string(&request).map_err(|_| ControlError::InvalidRequest)?;
        let payload = self
            .request_with_argument(
                ControlOperation::WorkspaceCleanup,
                Some(argument),
                CLEANUP_REQUEST_DEADLINE,
            )
            .await?;
        match payload {
            ControlPayload::WorkspaceCleanup(view) => {
                if let Some(view) = &view {
                    view.validate_for(scope)
                        .map_err(|_| ControlError::InvalidFrame)?;
                    let expected = match request {
                        WorkspaceCleanupRequest::Plan { .. } => None,
                        WorkspaceCleanupRequest::Status { cleanup_id, .. } => cleanup_id,
                        WorkspaceCleanupRequest::Confirm { cleanup_id, .. }
                        | WorkspaceCleanupRequest::Reconcile { cleanup_id, .. } => Some(cleanup_id),
                    };
                    if expected.is_some_and(|id| id != view.cleanup_id) {
                        return Err(ControlError::InvalidFrame);
                    }
                } else if !matches!(request, WorkspaceCleanupRequest::Status { .. }) {
                    return Err(ControlError::InvalidFrame);
                }
                Ok(view)
            }
            _ => Err(ControlError::InvalidFrame),
        }
    }
}

pub(super) async fn dispatch(
    state: Arc<ServerState>,
    argument: Option<String>,
) -> Result<ControlPayload, ControlError> {
    let argument = argument.ok_or(ControlError::InvalidRequest)?;
    if argument.len() > MAX_CLEANUP_ARGUMENT_BYTES {
        return Err(ControlError::InvalidRequest);
    }
    let request: WorkspaceCleanupRequest =
        serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
    request
        .validate()
        .map_err(|_| ControlError::InvalidRequest)?;
    #[cfg(not(windows))]
    {
        let _ = (state, request);
        Err(ControlError::Remote {
            code: "workspace_cleanup_platform_unsupported".into(),
        })
    }
    #[cfg(windows)]
    {
        let result = timeout(CLEANUP_REQUEST_DEADLINE, async {
            match request {
                WorkspaceCleanupRequest::Plan {
                    scope,
                    operation_id,
                } => state
                    .workspace_cleanup
                    .plan(scope, operation_id)
                    .await
                    .map(Some),
                WorkspaceCleanupRequest::Confirm {
                    scope,
                    operation_id,
                    cleanup_id,
                    plan_version,
                } => state
                    .workspace_cleanup
                    .confirm(scope, operation_id, cleanup_id, plan_version)
                    .await
                    .map(Some),
                WorkspaceCleanupRequest::Status { scope, cleanup_id } => {
                    state.workspace_cleanup.status(scope, cleanup_id).await
                }
                WorkspaceCleanupRequest::Reconcile {
                    scope,
                    operation_id,
                    cleanup_id,
                    expected_version,
                } => state
                    .workspace_cleanup
                    .reconcile(scope, operation_id, cleanup_id, expected_version)
                    .await
                    .map(Some),
            }
        })
        .await
        .map_err(|_| ControlError::Deadline)?
        .map_err(|code| ControlError::Remote { code: code.into() })?;
        Ok(ControlPayload::WorkspaceCleanup(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_requires_v16_and_never_falls_back_to_an_unconfigured_terminal() {
        use crate::chat_workspace::tests::OwnedFixture;
        let mut fixture = OwnedFixture::new().await;
        fixture.change(vibemux_store::ChatOwnerChange::Stop).await;
        let setup = fixture.worker.take().expect("setup writer");
        tokio::task::spawn_blocking(move || setup.shutdown())
            .await
            .expect("join task")
            .expect("setup joined");
        let database = fixture.state_dir.join("state.sqlite3");
        let server = DaemonControlServer::start(&database, &fixture.state_dir)
            .await
            .expect("server");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        let scope = WorkspaceCleanupScope {
            project_id: fixture.owner.project_id,
            task_id: fixture.owner.task_id,
            run_id: fixture.owner.run_id,
        };
        let before = client.frontend_task(scope.task_id).await.expect("before");
        let missing = client
            .workspace_cleanup(WorkspaceCleanupRequest::Status {
                scope,
                cleanup_id: None,
            })
            .await;
        let plan = client
            .workspace_cleanup(WorkspaceCleanupRequest::Plan {
                scope,
                operation_id: Uuid::new_v4(),
            })
            .await;
        let old = ControlClient {
            descriptor: ControlDescriptor {
                version: CONTROL_PROTOCOL_V15,
                ..client.descriptor.clone()
            },
        };
        let old_cleanup = old
            .workspace_cleanup(WorkspaceCleanupRequest::Status {
                scope,
                cleanup_id: None,
            })
            .await;
        let old_metadata = old
            .workspace_read(WorkspaceReadQuery {
                project_id: scope.project_id,
                task_id: scope.task_id,
                run_id: scope.run_id,
                mode: WorkspaceReadMode::Metadata,
            })
            .await;
        let after = client.frontend_task(scope.task_id).await.expect("after");
        server.shutdown().await.expect("owned shutdown");
        let reopened = database.clone();
        let worker = tokio::task::spawn_blocking(move || WriterWorker::start(&reopened))
            .await
            .expect("restore task")
            .expect("writer");
        fixture.writer = worker.handle().expect("handle");
        fixture.worker = Some(worker);
        let present = fixture.worktree().exists();
        fixture.finish().await;
        #[cfg(windows)]
        {
            assert!(missing.expect("read-only discovery").is_none());
            assert_eq!(
                plan.expect_err("no terminal proof").code(),
                "workspace_cleanup_terminal_unavailable"
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(
                missing.expect_err("unsupported platform").code(),
                "workspace_cleanup_platform_unsupported"
            );
            assert!(plan.is_err());
        }
        assert!(matches!(old_cleanup, Err(ControlError::UnsupportedVersion)));
        assert!(old_metadata.is_ok(), "v15 retains its read operation");
        assert_eq!(before, after);
        assert!(present);
    }
}
