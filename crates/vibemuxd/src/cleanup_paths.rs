//! Live directory identity for a cleanup scope. No deletion or process authority.
use std::{
    fs::{File, OpenOptions},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use vibemux_platform::{WindowsFileIdentity, query_windows_file_identity};
use vibemux_types::{
    a2a::RunWorkspace,
    workspace_cleanup::{CleanupObjectIdentity, WorkspaceCleanupBinding},
};
use vibemux_workspace::CleanupObservation;

const NAMESPACE_SHARING: u32 = 3;
const DIRECTORY_FLAGS: u32 = 0x0200_0000 | 0x0020_0000;
const REPARSE_ATTRIBUTE: u32 = 0x400;
const MAX_SCOPE_ANCESTORS: usize = 256;

struct DirectoryPin {
    path: PathBuf,
    file: File,
    identity: WindowsFileIdentity,
}
impl DirectoryPin {
    fn open(path: &Path) -> Result<Self, &'static str> {
        let file = OpenOptions::new()
            .read(true)
            .share_mode(NAMESPACE_SHARING)
            .custom_flags(DIRECTORY_FLAGS)
            .open(path)
            .map_err(|_| "workspace_cleanup_directory_unavailable")?;
        let identity = directory_identity(&file)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            identity,
        })
    }
    fn verify(&self) -> Result<(), &'static str> {
        let current = Self::open(&self.path)?;
        if current.identity != self.identity || directory_identity(&self.file)? != self.identity {
            return Err("workspace_cleanup_changed");
        }
        Ok(())
    }
}

/// Leaf sharing protects observation. It must be released immediately before
/// Go so Git can remove that leaf; repository/common/parent pins stay owned
/// until joined completion. This is the trusted-namespace contract of ADR066.
pub(crate) struct CleanupPaths {
    repository: DirectoryPin,
    common: DirectoryPin,
    parents: Vec<DirectoryPin>,
    target: Option<DirectoryPin>,
    target_identity: Option<WindowsFileIdentity>,
}
impl CleanupPaths {
    pub(crate) fn capture(
        observation: &CleanupObservation,
        workspace: &RunWorkspace,
    ) -> Result<Self, &'static str> {
        let mut pins = Self::base(
            &observation.repository_root,
            &observation.common_git_dir,
            Path::new(&workspace.path),
        )?;
        let target = DirectoryPin::open(Path::new(&workspace.path))?;
        pins.target_identity = Some(target.identity);
        pins.target = Some(target);
        pins.verify_before_release()?;
        Ok(pins)
    }
    pub(crate) fn remaining(binding: &WorkspaceCleanupBinding) -> Result<Self, &'static str> {
        let pins = Self::base(
            Path::new(&binding.repository_root),
            Path::new(&binding.common_git_dir),
            Path::new(&binding.workspace.path),
        )?;
        if object_identity(pins.repository.identity) != binding.repository_identity
            || object_identity(pins.common.identity) != binding.common_directory_identity
        {
            return Err("workspace_cleanup_changed");
        }
        pins.verify_roots()?;
        Ok(pins)
    }
    fn base(
        repository_root: &Path,
        common_git_dir: &Path,
        path: &Path,
    ) -> Result<Self, &'static str> {
        let repository = DirectoryPin::open(repository_root)?;
        let common = DirectoryPin::open(common_git_dir)?;
        let mut parents = Vec::new();
        let mut reached_root = false;
        for path in path
            .parent()
            .ok_or("workspace_cleanup_directory_unavailable")?
            .ancestors()
            .take(MAX_SCOPE_ANCESTORS)
        {
            let pin = DirectoryPin::open(path)?;
            if pin.identity == repository.identity {
                reached_root = true;
                break;
            }
            parents.push(pin);
        }
        if !reached_root || parents.is_empty() {
            return Err("workspace_cleanup_directory_unavailable");
        }
        Ok(Self {
            repository,
            common,
            parents,
            target: None,
            target_identity: None,
        })
    }
    pub(crate) fn identities(
        &self,
    ) -> Result<
        (
            CleanupObjectIdentity,
            CleanupObjectIdentity,
            CleanupObjectIdentity,
        ),
        &'static str,
    > {
        Ok((
            object_identity(self.repository.identity),
            object_identity(self.common.identity),
            object_identity(
                self.target_identity
                    .ok_or("workspace_cleanup_directory_unavailable")?,
            ),
        ))
    }
    pub(crate) fn matches(&self, binding: &WorkspaceCleanupBinding) -> bool {
        self.identities().is_ok_and(|ids| {
            ids == (
                binding.repository_identity.clone(),
                binding.common_directory_identity.clone(),
                binding.worktree_identity.clone(),
            )
        })
    }
    pub(crate) fn verify_roots(&self) -> Result<(), &'static str> {
        self.repository.verify()?;
        self.common.verify()?;
        for parent in &self.parents {
            parent.verify()?;
        }
        Ok(())
    }
    pub(crate) fn verify_before_release(&self) -> Result<(), &'static str> {
        self.verify_roots()?;
        self.target
            .as_ref()
            .ok_or("workspace_cleanup_directory_unavailable")?
            .verify()
    }
    pub(crate) fn release_target(&mut self) {
        self.target = None;
    }
}

fn directory_identity(file: &File) -> Result<WindowsFileIdentity, &'static str> {
    let metadata = file
        .metadata()
        .map_err(|_| "workspace_cleanup_directory_unavailable")?;
    if !metadata.is_dir() || metadata.file_attributes() & REPARSE_ATTRIBUTE != 0 {
        return Err("workspace_cleanup_directory_unavailable");
    }
    query_windows_file_identity(file).map_err(|_| "workspace_cleanup_identity_unavailable")
}
fn object_identity(identity: WindowsFileIdentity) -> CleanupObjectIdentity {
    CleanupObjectIdentity::WindowsFileId128V1 {
        volume_serial_number: identity.volume_serial_number,
        file_id: identity.file_id,
    }
}
