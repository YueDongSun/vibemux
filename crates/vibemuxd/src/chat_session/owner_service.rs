//! Owned workspace provisioning. Vendor execution is a separate admitted step.
use super::*;
use crate::chat_workspace::{inspect_owner_workspace, native_git};
use crate::harness_dispatch::child_path_text;
use std::{collections::BTreeSet, path::Path};
use vibemux_harness::dispatch::attempt;
use vibemux_store::{ChatExecutionOwnerOpen, ChatOwnerChange};
use vibemux_types::chat_owner::{
    ChatExecutionOwner, ChatOwnerFailure, ChatOwnerPhase, ChatWorkspaceEffect,
};
use vibemux_workspace::{WorkspaceError, WorkspaceManager};

impl ChatService {
    pub async fn open_execution_owner(
        &self,
        operation_id: Uuid,
        harness: ChatHarness,
        title: Option<String>,
    ) -> Result<ChatExecutionOwnerCommit, ChatServiceError> {
        match self
            .request(Action::OpenExecutionOwner {
                operation_id,
                harness,
                title,
            })
            .await?
        {
            Reply::ExecutionOwner(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }

    pub async fn reconcile_execution_owner(
        &self,
        input: ChatOwnerMutation,
    ) -> Result<ChatExecutionOwnerCommit, ChatServiceError> {
        match self
            .request(Action::ReconcileExecutionOwner { input })
            .await?
        {
            Reply::ExecutionOwner(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }

    pub async fn stop_execution_owner(
        &self,
        input: ChatOwnerMutation,
    ) -> Result<ChatExecutionOwnerCommit, ChatServiceError> {
        match self.request(Action::StopExecutionOwner { input }).await? {
            Reply::ExecutionOwner(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }

    pub async fn execution_owner(
        &self,
        chat_id: ChatId,
    ) -> Result<Option<ChatExecutionOwnerCommit>, ChatServiceError> {
        if !self.inner.context.has_project().await? {
            return Ok(None);
        }
        self.inner
            .context
            .writer()
            .await?
            .execution_owner(chat_id)
            .await
            .map_err(Into::into)
    }

    pub async fn execution_owner_for_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<ChatExecutionOwnerCommit>, ChatServiceError> {
        if operation_id.is_nil() {
            return Err(ChatError::InvalidInput.into());
        }
        if !self.inner.context.has_project().await? {
            return Ok(None);
        }
        self.inner
            .context
            .writer()
            .await?
            .execution_owner_for_operation(operation_id)
            .await
            .map_err(Into::into)
    }

    pub async fn execution_owners(
        &self,
        after: Option<ChatId>,
        limit: u32,
        finalized: bool,
    ) -> Result<Vec<ChatExecutionOwnerCommit>, ChatServiceError> {
        if limit == 0 || limit > vibemux_types::chat::MAX_CHAT_PAGE {
            return Err(ChatError::InvalidInput.into());
        }
        if !self.inner.context.has_project().await? {
            return Ok(Vec::new());
        }
        self.inner
            .context
            .writer()
            .await?
            .execution_owners(after, limit, finalized)
            .await
            .map_err(Into::into)
    }

    pub async fn execution_owner_title(
        &self,
        chat_id: ChatId,
    ) -> Result<Option<String>, ChatServiceError> {
        if !self.inner.context.has_project().await? {
            return Err(ChatError::NotFound.into());
        }
        self.inner
            .context
            .writer()
            .await?
            .execution_owner_title(chat_id)
            .await
            .map_err(Into::into)
    }

    pub fn workspace_provisioning_active(&self, chat_id: ChatId) -> Result<bool, ChatServiceError> {
        Ok(*self
            .inner
            .context
            .provisioning
            .lock()
            .map_err(|_| ChatServiceError::Stopped)?
            == Some(chat_id))
    }
}

impl Actor {
    pub(super) async fn open_execution_owner(
        &mut self,
        operation_id: Uuid,
        harness: ChatHarness,
        title: Option<String>,
    ) -> Result<ChatExecutionOwnerCommit, ChatServiceError> {
        if operation_id.is_nil() {
            return Err(ChatError::InvalidInput.into());
        }
        let writer = self.context.writer().await?.clone();
        if let Some(existing) = writer.execution_owner_for_operation(operation_id).await? {
            return writer
                .open_execution_owner(
                    ChatExecutionOwnerOpen {
                        operation_id,
                        harness,
                        title_ref: None,
                        profile_sha256: existing.owner.profile_sha256,
                        base_commit: existing.owner.base_commit,
                        admitted_tools: BTreeSet::new(),
                        timestamp: OffsetDateTime::now_utc(),
                    },
                    title,
                )
                .await
                .map_err(Into::into);
        }
        if self
            .context
            .provisioning
            .lock()
            .map_err(|_| ChatServiceError::Stopped)?
            .is_some()
        {
            return Err(ChatError::Busy.into());
        }
        let prepared = self
            .context
            .dispatch
            .prepare_chat(harness, new_target(harness))
            .await?;
        let root = prepared.launch.working_directory;
        let base_commit = read_head(root.clone()).await?;
        let opened = writer
            .open_execution_owner(
                ChatExecutionOwnerOpen {
                    operation_id,
                    harness,
                    title_ref: None,
                    profile_sha256: prepared.profile_sha256,
                    base_commit,
                    admitted_tools: BTreeSet::new(),
                    timestamp: OffsetDateTime::now_utc(),
                },
                title,
            )
            .await?;
        if !opened.duplicate {
            self.spawn_provision(opened.owner.chat_id, root)?;
        }
        Ok(opened)
    }

    pub(super) async fn reconcile_execution_owner(
        &mut self,
        input: ChatOwnerMutation,
    ) -> Result<ChatExecutionOwnerCommit, ChatServiceError> {
        let writer = self.context.writer().await?;
        if let Some(replayed) = writer
            .replay_execution_owner_change(input.clone(), ChatOwnerChange::Reconcile)
            .await?
        {
            return Ok(replayed);
        }
        if self
            .context
            .provisioning
            .lock()
            .map_err(|_| ChatServiceError::Stopped)?
            .is_some()
        {
            return Err(ChatError::Busy.into());
        }
        if writer
            .execution_owner(input.chat_id)
            .await?
            .is_some_and(|stored| {
                stored.owner.error_code == Some(ChatOwnerFailure::WorkspaceCleanupUnverified)
            })
        {
            return Err(WorkspaceError::ProcessCleanup.into());
        }
        let root = self
            .context
            .dispatch
            .project_root()
            .ok_or(WorkspaceError::InvalidPath)?
            .to_path_buf();
        let result = self
            .context
            .writer()
            .await?
            .change_execution_owner(input, ChatOwnerChange::Reconcile)
            .await?;
        if !result.duplicate {
            self.spawn_provision(result.owner.chat_id, root)?;
        }
        Ok(result)
    }

    pub(super) async fn stop_execution_owner(
        &mut self,
        input: ChatOwnerMutation,
    ) -> Result<ChatExecutionOwnerCommit, ChatServiceError> {
        let writer = self.context.writer().await?;
        if let Some(replayed) = writer
            .replay_execution_owner_change(input.clone(), ChatOwnerChange::Stop)
            .await?
        {
            return Ok(replayed);
        }
        if *self
            .context
            .provisioning
            .lock()
            .map_err(|_| ChatServiceError::Stopped)?
            == Some(input.chat_id)
        {
            return Err(ChatError::Busy.into());
        }
        self.context
            .writer()
            .await?
            .change_execution_owner(input, ChatOwnerChange::Stop)
            .await
            .map_err(Into::into)
    }

    fn spawn_provision(&mut self, chat_id: ChatId, root: PathBuf) -> Result<(), ChatServiceError> {
        let mut active = self
            .context
            .provisioning
            .lock()
            .map_err(|_| ChatServiceError::Stopped)?;
        if *self.context.stopping.borrow() {
            return Err(ChatServiceError::Stopped);
        }
        if active.is_some() {
            return Err(ChatError::Busy.into());
        }
        *active = Some(chat_id);
        drop(active);
        let context = Arc::clone(&self.context);
        self.provisions
            .spawn(provision_workspace(context, chat_id, root));
        Ok(())
    }

    pub(super) async fn finish_provision(
        &mut self,
        result: Option<Result<Result<(), ChatServiceError>, tokio::task::JoinError>>,
    ) {
        let id = self
            .context
            .provisioning
            .lock()
            .ok()
            .and_then(|mut active| active.take());
        // Ordinary errors already attempted one fenced failure commit. An
        // interrupted task has no such result, so retain an unknown claim.
        if matches!(result, Some(Err(_)))
            && let Some(id) = id
        {
            let _ = record_provision_failure(
                &self.context,
                id,
                false,
                ChatOwnerFailure::WorkspaceCleanupUnverified,
            )
            .await;
        }
    }
}

fn owner_mutation(owner: &ChatExecutionOwner) -> ChatOwnerMutation {
    ChatOwnerMutation {
        operation_id: Uuid::new_v4(),
        chat_id: owner.chat_id,
        expected_version: owner.version,
        timestamp: OffsetDateTime::now_utc(),
    }
}

/// The receipt and Git inventory are checked at each fresh process/input effect.
pub(super) async fn verify_execution_workspace(
    context: &Context,
    owner: &ChatExecutionOwner,
) -> Result<PathBuf, ChatServiceError> {
    if owner.phase != ChatOwnerPhase::Open
        || owner.workspace_effect != Some(ChatWorkspaceEffect::Settled)
        || owner.workspace_verified_at.is_none()
        || !owner.admitted_tools.is_empty()
    {
        return Err(ChatError::InvalidTransition.into());
    }
    let root = context
        .dispatch
        .project_root()
        .ok_or(WorkspaceError::InvalidPath)?
        .to_path_buf();
    let path = inspect_owner_workspace(&root, owner).await?;
    if read_head(path.clone()).await? != owner.base_commit {
        return Err(WorkspaceError::InvalidPath.into());
    }
    Ok(path)
}

async fn record_provision_failure(
    context: &Context,
    id: ChatId,
    cleanup_verified: bool,
    code: ChatOwnerFailure,
) -> Result<(), ChatServiceError> {
    let writer = context.writer().await?;
    let Some(stored) = writer.execution_owner(id).await? else {
        return Ok(());
    };
    if stored.owner.phase != ChatOwnerPhase::Finalized {
        writer
            .change_execution_owner(
                owner_mutation(&stored.owner),
                ChatOwnerChange::WorkspaceFailed {
                    code,
                    cleanup_verified,
                },
            )
            .await?;
    }
    Ok(())
}

async fn provision_workspace(
    context: Arc<Context>,
    id: ChatId,
    root: PathBuf,
) -> Result<(), ChatServiceError> {
    let writer = context.writer().await?.clone();
    let mut stored = writer
        .execution_owner(id)
        .await?
        .ok_or(ChatError::NotFound)?;
    let mut cleanup_verified = stored.owner.workspace_effect != Some(ChatWorkspaceEffect::Claimed);
    let result = async {
        let root_for_lookup = root.clone();
        let git = tokio::task::spawn_blocking(move || native_git(&root_for_lookup))
            .await
            .map_err(|_| ChatServiceError::Stopped)??;
        let manager = WorkspaceManager::new(git, root, stored.owner.base_commit.clone()).await?;
        if stored.owner.workspace.is_none() {
            let plan = manager.plan(stored.owner.run_id)?;
            let child_path =
                child_path_text(Path::new(&plan.path)).ok_or(WorkspaceError::InvalidPath)?;
            let resource_key = attempt::resource_key(&child_path).to_hex();
            stored = writer
                .change_execution_owner(
                    owner_mutation(&stored.owner),
                    ChatOwnerChange::Plan {
                        workspace: plan,
                        resource_key,
                    },
                )
                .await?;
        }
        let plan = stored
            .owner
            .workspace
            .clone()
            .ok_or(ChatError::ProjectionMismatch)?;
        let inspected = if stored.owner.workspace_effect == Some(ChatWorkspaceEffect::Planned) {
            false
        } else {
            match manager.inspect(&plan).await {
                Ok(()) => true,
                Err(WorkspaceError::ProcessCleanup) => {
                    cleanup_verified = false;
                    return Err(WorkspaceError::ProcessCleanup.into());
                }
                Err(_) => false,
            }
        };
        if !inspected {
            if stored.owner.workspace_effect == Some(ChatWorkspaceEffect::Claimed) {
                return Err(ChatServiceError::Workspace(WorkspaceError::InvalidPath));
            }
            stored = writer
                .change_execution_owner(
                    owner_mutation(&stored.owner),
                    ChatOwnerChange::ClaimCreation,
                )
                .await?;
            cleanup_verified = false;
            if stored.duplicate {
                return Err(ChatError::Conflict.into());
            }
            let created = manager.create_planned(stored.owner.run_id, &plan).await;
            cleanup_verified = !matches!(created, Err(WorkspaceError::ProcessCleanup));
            created?;
        } else {
            cleanup_verified = true;
        }
        let path = PathBuf::from(&plan.path);
        let resource_path = path.clone();
        let resource = tokio::task::spawn_blocking(move || {
            HarnessDispatchService::workflow_resource_key(&resource_path)
        })
        .await
        .map_err(|_| ChatServiceError::Stopped)?
        .ok_or(WorkspaceError::InvalidPath)?;
        if Some(resource.1.to_hex()) != stored.owner.resource_key
            || read_head(path).await? != stored.owner.base_commit
        {
            return Err(ChatServiceError::Workspace(WorkspaceError::InvalidPath));
        }
        writer
            .change_execution_owner(
                owner_mutation(&stored.owner),
                ChatOwnerChange::VerifyWorkspace,
            )
            .await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let code = if matches!(
            result,
            Err(ChatServiceError::Workspace(WorkspaceError::ProcessCleanup))
        ) {
            ChatOwnerFailure::WorkspaceCleanupUnverified
        } else {
            ChatOwnerFailure::WorkspaceUnverified
        };
        let cleanup_verified =
            cleanup_verified && code != ChatOwnerFailure::WorkspaceCleanupUnverified;
        let _ = record_provision_failure(&context, id, cleanup_verified, code).await;
    }
    result
}

#[cfg(test)]
mod physical_workspace_tests {
    use super::*;
    use crate::chat_workspace::tests::OwnedFixture;
    use crate::harness_dispatch::DispatchServiceSettings;
    use std::collections::HashMap;
    use tokio::sync::{OnceCell, watch};
    use vibemux_types::ToolName;

    #[tokio::test]
    async fn observation_does_not_relax_execution_grants_or_base_head_authority() {
        let fixture = OwnedFixture::new().await;
        let dispatch = HarnessDispatchService::start(
            fixture.writer.clone(),
            DispatchServiceSettings::for_project(&fixture.root, &fixture.state_dir),
        )
        .await;
        let (_stop, stopping) = watch::channel(false);
        let context = Context {
            source: fixture.writer.clone(),
            state_dir: fixture.state_dir.clone(),
            writer: OnceCell::new(),
            dispatch,
            pending: Mutex::new(HashMap::new()),
            provisioning: Mutex::new(None),
            stopping,
        };
        let path = fixture.worktree();
        let baseline = verify_execution_workspace(&context, &fixture.owner).await;
        let mut granted = fixture.owner.clone();
        granted.admitted_tools.insert(ToolName::ReadFiles);
        let grants = verify_execution_workspace(&context, &granted).await;
        let advanced = fixture.advance_head().await;
        let observed = inspect_owner_workspace(&fixture.root, &fixture.owner).await;
        let execution = verify_execution_workspace(&context, &fixture.owner).await;
        let base = fixture.base.clone();
        let original_base = fixture.owner.base_commit.clone();
        context.dispatch.shutdown().await;
        drop(context);
        fixture.finish().await;
        assert_eq!(baseline.expect("empty grants at the original HEAD"), path);
        assert!(matches!(
            grants,
            Err(ChatServiceError::Rejected(ChatError::InvalidTransition))
        ));
        assert_eq!(original_base, base);
        assert_ne!(advanced, base);
        assert_eq!(
            observed.expect("common physical observation permits the owned commit"),
            path
        );
        assert!(matches!(
            execution,
            Err(ChatServiceError::Workspace(WorkspaceError::InvalidPath))
        ));
    }
}
