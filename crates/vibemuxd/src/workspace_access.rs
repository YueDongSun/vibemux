//! Typed read-only workspace metadata and managed-directory presentation.

use crate::{
    WriterHandle,
    chat_workspace::{inspect_managed_workspace, inspect_presented_owner_workspace},
    harness_dispatch::{HarnessDispatchService, child_path_text},
};
use serde::{Deserialize, Serialize};
use std::{fmt, path::Path};
use vibemux_store::ChatExecutionOwnerCommit;
use vibemux_types::{
    ProjectId, RunId, TaskId,
    a2a::{A2aRunRecord, RunWorkspace},
    chat::ChatState,
    chat_owner::{ChatOwnerPhase, ChatWorkspaceEffect},
};

pub const MAX_WORKSPACE_PATH_BYTES: usize = 4096;
pub const MAX_WORKSPACE_BRANCH_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceReadMode {
    Metadata,
    Directory,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceReadQuery {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub mode: WorkspaceReadMode,
}
impl WorkspaceReadQuery {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.project_id.as_uuid().is_nil()
            || self.task_id.as_uuid().is_nil()
            || self.run_id.as_uuid().is_nil()
        {
            Err("workspace_invalid_request")
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDirectory {
    pub path: String,
    /// Logical resource key derived from canonical path text. Native code pins
    /// operating-system directory identity separately before an effect.
    pub resource_key: String,
}
impl fmt::Debug for WorkspaceDirectory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceDirectory")
            .field("path_bytes", &self.path.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceReadView {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub branch: Option<String>,
    pub directory: Option<WorkspaceDirectory>,
}
impl WorkspaceReadView {
    pub fn validate_for(&self, query: &WorkspaceReadQuery) -> Result<(), &'static str> {
        query.validate()?;
        validate_view(self)?;
        if self.project_id != query.project_id
            || self.task_id != query.task_id
            || self.run_id != query.run_id
        {
            return Err("workspace_scope_mismatch");
        }
        match (query.mode, &self.directory) {
            (WorkspaceReadMode::Metadata, None) => Ok(()),
            (WorkspaceReadMode::Directory, Some(directory))
                if Path::new(&directory.path).is_absolute() =>
            {
                Ok(())
            }
            _ => Err("workspace_invalid_response"),
        }
    }
}
impl fmt::Debug for WorkspaceReadView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceReadView")
            .field("project_id", &self.project_id)
            .field("task_id", &self.task_id)
            .field("run_id", &self.run_id)
            .field("has_branch", &self.branch.is_some())
            .field("has_directory", &self.directory.is_some())
            .finish()
    }
}

enum Authority {
    Owner(Box<ChatExecutionOwnerCommit>),
    A2a(Box<A2aRunRecord>),
}
impl Authority {
    fn workspace(&self) -> Option<&RunWorkspace> {
        match self {
            Self::Owner(stored) => stored.owner.workspace.as_ref(),
            Self::A2a(record) => Some(&record.workspace),
        }
    }
    fn require_scope(&self, query: &WorkspaceReadQuery) -> Result<(), &'static str> {
        let matches = match self {
            Self::Owner(stored) => {
                stored.owner.project_id == query.project_id
                    && stored.owner.task_id == query.task_id
                    && stored.owner.run_id == query.run_id
                    && stored.run.run_id() == query.run_id
            }
            Self::A2a(record) => {
                record.task.project_id() == query.project_id
                    && record.task.task_id() == query.task_id
                    && record.run.project_id() == query.project_id
                    && record.run.task_id() == query.task_id
                    && record.run.run_id() == query.run_id
                    && record.binding.project_id == query.project_id
                    && record.binding.task_id == query.task_id
                    && record.binding.run_id == query.run_id
            }
        };
        if matches {
            Ok(())
        } else {
            Err("workspace_scope_mismatch")
        }
    }
    fn require_directory(&self) -> Result<(), &'static str> {
        if let Self::Owner(stored) = self {
            let owner = &stored.owner;
            if !matches!(
                owner.phase,
                ChatOwnerPhase::Open | ChatOwnerPhase::Finalized
            ) || owner.workspace_effect != Some(ChatWorkspaceEffect::Settled)
                || owner.workspace_verified_at.is_none()
                || stored.session.state == ChatState::RecoveryRequired
            {
                return Err("workspace_directory_unavailable");
            }
        }
        if self.workspace().is_none() {
            return Err("workspace_directory_unavailable");
        }
        Ok(())
    }
    fn same_immutable_binding(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Owner(left), Self::Owner(right)) => {
                let (left, right) = (&left.owner, &right.owner);
                left.chat_id == right.chat_id
                    && left.project_id == right.project_id
                    && left.task_id == right.task_id
                    && left.run_id == right.run_id
                    && left.base_commit == right.base_commit
                    && left.workspace == right.workspace
                    && left.resource_key == right.resource_key
            }
            (Self::A2a(left), Self::A2a(right)) => {
                left.run.project_id() == right.run.project_id()
                    && left.run.task_id() == right.run.task_id()
                    && left.run.run_id() == right.run.run_id()
                    && left.run.base_commit() == right.run.base_commit()
                    && left.workspace == right.workspace
            }
            _ => false,
        }
    }
}

/// Each snapshot is one serialized authoritative read. A failed complete owner
/// load aborts the closure and cannot select a different authority domain.
async fn load_authority(
    writer: WriterHandle,
    query: &WorkspaceReadQuery,
) -> Result<Authority, &'static str> {
    let run_id = query.run_id;
    let needs_directory = query.mode == WorkspaceReadMode::Directory;
    let (project_id, owner, a2a, cleanup_blocks_directory) =
        tokio::task::spawn_blocking(move || {
            writer.with_store(move |store| {
                let project_id = store.project_id()?;
                let owner = store.chat_execution_owner_for_run(run_id)?;
                let a2a = store.a2a_run(run_id)?;
                let blocked =
                    needs_directory && store.workspace_cleanup_blocks_directory(run_id)?;
                Ok((project_id, owner, a2a, blocked))
            })
        })
        .await
        .map_err(|_| "workspace_read_failed")?
        .map_err(|_| "workspace_authority_invalid")?;
    if project_id != Some(query.project_id) {
        return Err("workspace_scope_mismatch");
    }
    let authority = match (owner, a2a) {
        (Some(_), Some(_)) => return Err("workspace_authority_conflict"),
        (Some(owner), None) => Authority::Owner(Box::new(owner)),
        (None, Some(a2a)) => Authority::A2a(Box::new(a2a)),
        (None, None) => return Err("workspace_run_unavailable"),
    };
    authority.require_scope(query)?;
    if cleanup_blocks_directory {
        return Err("workspace_directory_unavailable");
    }
    Ok(authority)
}

#[cfg(test)]
pub(crate) async fn read_workspace(
    writer: WriterHandle,
    project_root: &Path,
    query: WorkspaceReadQuery,
) -> Result<WorkspaceReadView, &'static str> {
    read_workspace_with_jobs(writer, project_root, query, None).await
}

pub(crate) async fn read_workspace_with_jobs(
    writer: WriterHandle,
    project_root: &Path,
    query: WorkspaceReadQuery,
    jobs: Option<crate::workspace_jobs::WorkspaceJobs>,
) -> Result<WorkspaceReadView, &'static str> {
    query.validate()?;
    let initial = load_authority(writer.clone(), &query).await?;
    let mut view = WorkspaceReadView {
        project_id: query.project_id,
        task_id: query.task_id,
        run_id: query.run_id,
        branch: initial
            .workspace()
            .map(|workspace| workspace.branch.clone()),
        directory: None,
    };
    validate_view(&view)?;
    if query.mode == WorkspaceReadMode::Metadata {
        view.validate_for(&query)?;
        return Ok(view);
    }
    initial.require_directory()?;
    let guard_key = if let Some(jobs) = jobs {
        let key = match &initial {
            Authority::Owner(stored) => stored
                .owner
                .resource_key
                .clone()
                .ok_or("workspace_directory_unavailable")?,
            Authority::A2a(record) => {
                // A legacy A2A path first needs a preliminary key resolution.
                // This read grants no view; full policy/physical checks follow
                // gate acquisition and repeat the key before returning data.
                let path = std::path::PathBuf::from(&record.workspace.path);
                tokio::task::spawn_blocking(move || {
                    HarnessDispatchService::workflow_resource_key(&path)
                })
                .await
                .map_err(|_| "workspace_read_failed")?
                .ok_or("workspace_directory_unavailable")?
                .1
                .to_hex()
            }
        };
        let guard = jobs.read(&key).await?;
        revalidate_after_inspection(writer.clone(), &query, &initial).await?;
        Some((guard, key))
    } else {
        None
    };
    let path = match &initial {
        Authority::Owner(stored) => {
            inspect_presented_owner_workspace(project_root, &stored.owner).await
        }
        Authority::A2a(record) => {
            inspect_managed_workspace(project_root, query.run_id, &record.workspace).await
        }
    }
    .map_err(|_| "workspace_directory_unavailable")?;
    let expected_path = child_path_text(&path).ok_or("workspace_directory_unavailable")?;
    let resource_path = path.clone();
    let resource = tokio::task::spawn_blocking(move || {
        HarnessDispatchService::workflow_resource_key(&resource_path)
    })
    .await
    .map_err(|_| "workspace_read_failed")?
    .ok_or("workspace_directory_unavailable")?;
    if resource.0 != expected_path {
        return Err("workspace_directory_unavailable");
    }
    let resource_key = resource.1.to_hex();
    if guard_key
        .as_ref()
        .is_some_and(|(_, key)| key != &resource_key)
    {
        return Err("workspace_authority_changed");
    }
    if let Authority::Owner(stored) = &initial
        && stored.owner.resource_key.as_deref() != Some(resource_key.as_str())
    {
        return Err("workspace_directory_unavailable");
    }
    view.directory = Some(WorkspaceDirectory {
        path: resource.0,
        resource_key,
    });
    validate_view(&view)?;
    // Physical work holds no store transaction. Re-read and validate current
    // owner/session policy and immutable authority before releasing the view.
    revalidate_after_inspection(writer, &query, &initial).await?;
    view.validate_for(&query)?;
    Ok(view)
}

async fn revalidate_after_inspection(
    writer: WriterHandle,
    query: &WorkspaceReadQuery,
    initial: &Authority,
) -> Result<(), &'static str> {
    let current = load_authority(writer, query).await?;
    current.require_directory()?;
    if !initial.same_immutable_binding(&current) {
        return Err("workspace_authority_changed");
    }
    Ok(())
}

fn validate_view(view: &WorkspaceReadView) -> Result<(), &'static str> {
    if let Some(branch) = &view.branch
        && (branch.is_empty()
            || branch.len() > MAX_WORKSPACE_BRANCH_BYTES
            || branch.chars().any(char::is_control))
    {
        return Err("workspace_limit");
    }
    if let Some(directory) = &view.directory
        && (directory.path.is_empty()
            || directory.path.len() > MAX_WORKSPACE_PATH_BYTES
            || directory.path.chars().any(char::is_control)
            || directory.resource_key.len() != 64
            || !directory
                .resource_key
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    {
        return Err("workspace_limit");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
