//! Control v15 reads for native worktree presentation; no state or OS effects.
use super::*;
pub use crate::workspace_access::{
    WorkspaceDirectory, WorkspaceReadMode, WorkspaceReadQuery, WorkspaceReadView,
};

pub const WORKSPACE_READ_DEADLINE: Duration = Duration::from_secs(30);

impl ControlClient {
    pub async fn workspace_read(
        &self,
        query: WorkspaceReadQuery,
    ) -> Result<WorkspaceReadView, ControlError> {
        self.require_operation(ControlOperation::WorkspaceRead)?;
        query.validate().map_err(|_| ControlError::InvalidRequest)?;
        let argument = serde_json::to_string(&query).map_err(|_| ControlError::InvalidRequest)?;
        let payload = self
            .request_with_argument(
                ControlOperation::WorkspaceRead,
                Some(argument),
                WORKSPACE_READ_DEADLINE,
            )
            .await?;
        match payload {
            ControlPayload::WorkspaceRead(view) if view.validate_for(&query).is_ok() => Ok(view),
            _ => Err(ControlError::InvalidFrame),
        }
    }
}

pub(super) async fn dispatch(
    state: Arc<ServerState>,
    argument: Option<String>,
) -> Result<ControlPayload, ControlError> {
    let argument = argument.ok_or(ControlError::InvalidRequest)?;
    if argument.len() > 2048 {
        return Err(ControlError::InvalidRequest);
    }
    let query: WorkspaceReadQuery =
        serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
    query.validate().map_err(|_| ControlError::InvalidRequest)?;
    let writer = state
        .writer
        .lock()
        .map_err(|_| ControlError::ServerTerminated)?
        .as_ref()
        .ok_or(ControlError::ServerTerminated)?
        .handle()?;
    let jobs = state.workspace_jobs.clone();
    let root = state.project_root.clone();
    let view = timeout(
        WORKSPACE_READ_DEADLINE,
        state.workspace_jobs.execute(async move {
            crate::workspace_access::read_workspace_with_jobs(writer, &root, query, Some(jobs))
                .await
        }),
    )
    .await
    .map_err(|_| ControlError::Deadline)?
    .map_err(|code| ControlError::Remote { code: code.into() })?;
    Ok(ControlPayload::WorkspaceRead(view))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_types::{ProjectId, RunId, TaskId};

    #[cfg(feature = "test_helpers")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_joins_a_directory_job_after_its_rpc_waiter_disappears() {
        use crate::chat_workspace::tests::OwnedFixture;
        let mut fixture = OwnedFixture::new().await;
        let setup = fixture.worker.take().expect("setup writer");
        tokio::task::spawn_blocking(move || setup.shutdown())
            .await
            .expect("setup join")
            .expect("writer joined");
        let database = fixture.state_dir.join("state.sqlite3");
        let server = DaemonControlServer::start(&database, &fixture.state_dir)
            .await
            .expect("server");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        let state = Arc::clone(&server.fixture_state);
        let key = fixture.owner.resource_key.as_ref().expect("resource");
        let exclusive = state
            .workspace_jobs
            .write(key)
            .await
            .expect("exclusive resource fixture");
        let query = WorkspaceReadQuery {
            project_id: fixture.owner.project_id,
            task_id: fixture.owner.task_id,
            run_id: fixture.owner.run_id,
            mode: WorkspaceReadMode::Metadata,
        };
        client
            .workspace_read(query.clone())
            .await
            .expect("metadata stays available without directory gate");
        let before = client.frontend_task(query.task_id).await.expect("before");
        let waiter = tokio::spawn(async move {
            client
                .workspace_read(WorkspaceReadQuery {
                    mode: WorkspaceReadMode::Directory,
                    ..query
                })
                .await
        });
        timeout(Duration::from_secs(3), async {
            loop {
                if state.workspace_jobs.pending_for_test() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("actual accepted directory job");
        waiter.abort();
        let _ = waiter.await;
        timeout(Duration::from_secs(10), server.shutdown())
            .await
            .expect("queued read cancels without releasing fixture write")
            .expect("server fully joined");
        assert_eq!(state.workspace_jobs.pending_for_test(), 0);
        drop(exclusive);
        let reopened = database.clone();
        let worker = tokio::task::spawn_blocking(move || WriterWorker::start(&reopened))
            .await
            .expect("restore writer task")
            .expect("writer");
        fixture.writer = worker.handle().expect("handle");
        fixture.worker = Some(worker);
        let reader = fixture.writer.clone();
        let task_id = fixture.owner.task_id;
        let after = tokio::task::spawn_blocking(move || reader.frontend_task(task_id))
            .await
            .expect("read task")
            .expect("after");
        fixture.finish().await;
        assert_eq!(
            after, before,
            "read cancellation and shutdown preserve task/run state"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn authenticated_workspace_reads_cross_control_without_mutating_task_or_run() {
        use crate::chat_workspace::tests::OwnedFixture;
        let mut fixture = OwnedFixture::new().await;
        let setup_writer = fixture.worker.take().expect("owned setup writer");
        tokio::task::spawn_blocking(move || setup_writer.shutdown())
            .await
            .expect("setup join task")
            .expect("setup writer joined");
        let database = fixture.state_dir.join("state.sqlite3");
        let server = DaemonControlServer::start(&database, &fixture.state_dir)
            .await
            .expect("owned control server");
        let client =
            ControlClient::from_descriptor(server.descriptor_path()).expect("authenticated client");
        let query = WorkspaceReadQuery {
            project_id: fixture.owner.project_id,
            task_id: fixture.owner.task_id,
            run_id: fixture.owner.run_id,
            mode: WorkspaceReadMode::Directory,
        };
        let before = client
            .frontend_task(query.task_id)
            .await
            .expect("before")
            .expect("task");
        let directory = client.workspace_read(query.clone()).await;
        let metadata = client
            .workspace_read(WorkspaceReadQuery {
                mode: WorkspaceReadMode::Metadata,
                ..query.clone()
            })
            .await;
        let old_client = ControlClient {
            descriptor: ControlDescriptor {
                version: CONTROL_PROTOCOL_V14,
                ..client.descriptor.clone()
            },
        };
        let old_task = old_client.frontend_task(query.task_id).await;
        let old_workspace = old_client.workspace_read(query.clone()).await;
        let unknown = client
            .workspace_read(WorkspaceReadQuery {
                run_id: RunId::new(),
                ..query.clone()
            })
            .await;
        let after = client
            .frontend_task(query.task_id)
            .await
            .expect("after")
            .expect("task");
        server
            .shutdown()
            .await
            .expect("control server and writer joined");
        let resumed = tokio::task::spawn_blocking(move || WriterWorker::start(&database))
            .await
            .expect("restore fixture owner task")
            .expect("fixture writer");
        fixture.writer = resumed.handle().expect("fixture handle");
        fixture.worker = Some(resumed);
        let expected_branch = fixture
            .owner
            .workspace
            .as_ref()
            .expect("workspace")
            .branch
            .clone();
        fixture.finish().await;
        assert!(
            directory
                .expect("real directory operation")
                .directory
                .is_some()
        );
        let metadata = metadata.expect("real metadata operation");
        assert_eq!(metadata.branch, Some(expected_branch));
        assert!(metadata.directory.is_none());
        assert_eq!(before.task, after.task);
        assert_eq!(before.runs, after.runs);
        assert_eq!(
            old_task
                .expect("v14 request remains supported")
                .expect("task")
                .task,
            before.task
        );
        assert_eq!(
            old_workspace.expect_err("v15 read refused locally").code(),
            "control_unsupported_version"
        );
        assert_eq!(
            unknown.expect_err("unknown canonical run").code(),
            "workspace_run_unavailable"
        );
    }

    #[tokio::test]
    async fn workspace_read_refuses_an_old_descriptor_before_transport() {
        let client = ControlClient {
            descriptor: ControlDescriptor {
                version: CONTROL_PROTOCOL_V14,
                endpoint: "not-a-live-endpoint".into(),
                token: "fixed-test-token".into(),
                process_id: 1,
            },
        };
        let error = client
            .workspace_read(WorkspaceReadQuery {
                project_id: ProjectId::new(),
                task_id: TaskId::new(),
                run_id: RunId::new(),
                mode: WorkspaceReadMode::Directory,
            })
            .await
            .expect_err("v15 capability required before connection");
        assert_eq!(error.code(), "control_unsupported_version");
        assert_eq!(
            ControlOperation::WorkspaceRead.minimum_protocol_version(),
            15
        );
        assert_eq!(
            ControlOperation::TerminalInspect.minimum_protocol_version(),
            4
        );
        assert_eq!(
            ControlOperation::ChatApproval.minimum_protocol_version(),
            14
        );
    }
}
