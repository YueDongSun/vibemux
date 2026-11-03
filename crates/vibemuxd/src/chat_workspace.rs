//! Physical ownership checks for execution, terminal, and directory presentation.
use crate::harness_dispatch::HarnessDispatchService;
use std::path::{Path, PathBuf};
use vibemux_types::chat_owner::{ChatExecutionOwner, ChatOwnerPhase, ChatWorkspaceEffect};
use vibemux_types::{RunId, a2a::RunWorkspace};
use vibemux_workspace::{WorkspaceError, WorkspaceManager};

/// Observation does not admit tools or require an unchanged branch HEAD.
pub(crate) async fn inspect_owner_workspace(
    project_root: &Path,
    owner: &ChatExecutionOwner,
) -> Result<PathBuf, WorkspaceError> {
    owner.validate().map_err(|_| WorkspaceError::InvalidPath)?;
    if owner.phase != ChatOwnerPhase::Open
        || owner.workspace_effect != Some(ChatWorkspaceEffect::Settled)
        || owner.workspace_verified_at.is_none()
    {
        return Err(WorkspaceError::InvalidPath);
    }
    inspect_owner_physical(project_root, owner).await
}

/// Retained verified workspaces remain presentable after owner finalization.
pub(crate) async fn inspect_presented_owner_workspace(
    project_root: &Path,
    owner: &ChatExecutionOwner,
) -> Result<PathBuf, WorkspaceError> {
    owner.validate().map_err(|_| WorkspaceError::InvalidPath)?;
    if !matches!(
        owner.phase,
        ChatOwnerPhase::Open | ChatOwnerPhase::Finalized
    ) || owner.workspace_effect != Some(ChatWorkspaceEffect::Settled)
        || owner.workspace_verified_at.is_none()
    {
        return Err(WorkspaceError::InvalidPath);
    }
    inspect_owner_physical(project_root, owner).await
}

async fn inspect_owner_physical(
    project_root: &Path,
    owner: &ChatExecutionOwner,
) -> Result<PathBuf, WorkspaceError> {
    let workspace = owner
        .workspace
        .as_ref()
        .ok_or(WorkspaceError::InvalidPath)?;
    let path = inspect_managed_workspace(project_root, owner.run_id, workspace).await?;
    let resource_path = path.clone();
    let resource = tokio::task::spawn_blocking(move || {
        HarnessDispatchService::workflow_resource_key(&resource_path)
    })
    .await
    .map_err(|_| WorkspaceError::InvalidPath)?
    .ok_or(WorkspaceError::InvalidPath)?;
    if Some(resource.1.to_hex()) != owner.resource_key {
        return Err(WorkspaceError::InvalidPath);
    }
    Ok(path)
}

/// Inspects the exact managed Run slot without provisioning missing namespaces.
pub(crate) async fn inspect_managed_workspace(
    project_root: &Path,
    run_id: RunId,
    workspace: &RunWorkspace,
) -> Result<PathBuf, WorkspaceError> {
    let path = PathBuf::from(&workspace.path);
    let slot = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.parse::<RunId>().ok());
    if slot != Some(run_id) {
        return Err(WorkspaceError::InvalidPath);
    }
    let root = project_root.to_path_buf();
    let root_for_git = root.clone();
    let git = tokio::task::spawn_blocking(move || native_git(&root_for_git))
        .await
        .map_err(|_| WorkspaceError::Git)??;
    let manager = WorkspaceManager::open_existing(git, root, workspace.base_commit.clone()).await?;
    manager.inspect(workspace).await?;
    Ok(path)
}

pub(crate) fn native_git(root: &Path) -> Result<PathBuf, WorkspaceError> {
    let root = std::fs::canonicalize(root).map_err(|_| WorkspaceError::InvalidPath)?;
    let path = std::env::var_os("PATH").ok_or(WorkspaceError::Git)?;
    let name = if cfg!(windows) { "git.exe" } else { "git" };
    std::env::split_paths(&path)
        .filter(|entry| entry.is_absolute())
        .filter_map(|entry| std::fs::canonicalize(entry.join(name)).ok())
        .find(|candidate| {
            let Ok(metadata) = std::fs::metadata(candidate) else {
                return false;
            };
            if !metadata.is_file() || candidate.starts_with(&root) {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                candidate
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
            }
        })
        .ok_or(WorkspaceError::Git)
}

#[cfg(test)]
pub(crate) mod tests;
