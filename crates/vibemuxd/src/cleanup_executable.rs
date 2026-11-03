//! Retained Windows object handles for a cleanup executable outside its project.

use std::{
    ffi::OsStr,
    fmt,
    fs::{self, File, Metadata, OpenOptions},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf, Prefix},
};

use vibemux_platform::{WindowsFileIdentity, query_windows_file_identity};
use vibemux_types::workspace_cleanup::CleanupObjectIdentity;

const FILE_SHARE_READ: u32 = 1;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const MAX_DIRECTORY_PINS: usize = 256;
const INVALID_PATH: &str = "cleanup_executable_invalid_path";
const UNAVAILABLE: &str = "cleanup_executable_unavailable";
const INVALID_OBJECT: &str = "cleanup_executable_invalid_object";
const IDENTITY_UNAVAILABLE: &str = "cleanup_executable_identity_unavailable";
const CAPACITY: &str = "cleanup_executable_capacity";
const PROJECT_CONTAINED: &str = "cleanup_executable_project_contained";
const CHANGED: &str = "cleanup_executable_changed";

struct DirectoryPin {
    path: PathBuf,
    file: File,
    identity: WindowsFileIdentity,
}

impl DirectoryPin {
    fn acquire(path: PathBuf) -> Result<Self, &'static str> {
        let file = open_directory(&path)?;
        let identity = directory_identity(&file)?;
        Ok(Self {
            path,
            file,
            identity,
        })
    }

    fn verify(&self) -> Result<(), &'static str> {
        if directory_identity(&self.file)? != self.identity
            || directory_identity(&open_directory(&self.path)?)? != self.identity
        {
            return Err(CHANGED);
        }
        Ok(())
    }
}

/// The handles must remain owned until the contained child has been joined.
/// Object identity is private audit data, not process or filesystem authority.
pub(crate) struct CleanupExecutablePin {
    path: PathBuf,
    file: File,
    identity: WindowsFileIdentity,
    length: u64,
    ancestry: Vec<DirectoryPin>,
    project_ancestry: Vec<DirectoryPin>,
}

impl CleanupExecutablePin {
    pub(crate) fn acquire(path: &Path, project_root: &Path) -> Result<Self, &'static str> {
        validate_local_path(path)?;
        let ancestor_paths = directory_paths(path.parent().ok_or(INVALID_PATH)?)?;
        let project_paths = directory_paths(project_root)?;
        if ancestor_paths.len() + project_paths.len() > MAX_DIRECTORY_PINS {
            return Err(CAPACITY);
        }
        // Root-first acquisition retains every component before opening its
        // descendant. OPEN_REPARSE_POINT observes raw links instead of following
        // them. Denying WRITE and DELETE excludes writable reparse mutation
        // handles as well as replacement of the pinned chain.
        let ancestry = ancestor_paths
            .into_iter()
            .map(DirectoryPin::acquire)
            .collect::<Result<Vec<_>, _>>()?;
        let project_ancestry = project_paths
            .into_iter()
            .map(DirectoryPin::acquire)
            .collect::<Result<Vec<_>, _>>()?;
        let project_identity = project_ancestry.last().ok_or(INVALID_PATH)?.identity;
        if ancestry
            .iter()
            .any(|ancestor| ancestor.identity == project_identity)
        {
            return Err(PROJECT_CONTAINED);
        }
        let canonical_project = fs::canonicalize(project_root).map_err(|_| UNAVAILABLE)?;
        validate_local_path(&canonical_project)?;
        if directory_identity(&open_directory(&canonical_project)?)? != project_identity {
            return Err(CHANGED);
        }
        let file = open_executable(path)?;
        let (identity, length) = executable_identity(&file)?;
        let canonical_path = fs::canonicalize(path).map_err(|_| UNAVAILABLE)?;
        validate_local_path(&canonical_path)?;
        let pin = Self {
            path: canonical_path,
            file,
            identity,
            length,
            ancestry,
            project_ancestry,
        };
        // Check both the held objects and their current path bindings after the
        // complete chain is pinned. The consumer repeats this immediately
        // before consuming execution authority.
        pin.verify()?;
        Ok(pin)
    }

    pub(crate) fn verify(&self) -> Result<(), &'static str> {
        for directory in self.ancestry.iter().chain(&self.project_ancestry) {
            directory.verify()?;
        }
        let held = executable_identity(&self.file)?;
        let current = executable_identity(&open_executable(&self.path)?)?;
        if held != (self.identity, self.length) || current != held {
            return Err(CHANGED);
        }
        Ok(())
    }

    pub(crate) fn canonical_path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn identity(&self) -> CleanupObjectIdentity {
        CleanupObjectIdentity::WindowsFileId128V1 {
            volume_serial_number: self.identity.volume_serial_number,
            file_id: self.identity.file_id,
        }
    }
}

impl fmt::Debug for CleanupExecutablePin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CleanupExecutablePin(<redacted>)")
    }
}

fn open_directory(path: &Path) -> Result<File, &'static str> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| UNAVAILABLE)
}

fn open_executable(path: &Path) -> Result<File, &'static str> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| UNAVAILABLE)
}

fn directory_identity(file: &File) -> Result<WindowsFileIdentity, &'static str> {
    let metadata = file.metadata().map_err(|_| UNAVAILABLE)?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(INVALID_OBJECT);
    }
    query_windows_file_identity(file).map_err(|_| IDENTITY_UNAVAILABLE)
}

fn executable_identity(file: &File) -> Result<(WindowsFileIdentity, u64), &'static str> {
    let metadata = file.metadata().map_err(|_| UNAVAILABLE)?;
    if !metadata.is_file() || is_reparse(&metadata) || metadata.len() == 0 {
        return Err(INVALID_OBJECT);
    }
    Ok((
        query_windows_file_identity(file).map_err(|_| IDENTITY_UNAVAILABLE)?,
        metadata.len(),
    ))
}

fn is_reparse(metadata: &Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn validate_local_path(path: &Path) -> Result<(), &'static str> {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        || components.next() != Some(Component::RootDir)
    {
        return Err(INVALID_PATH);
    }
    for component in components {
        match component {
            Component::Normal(name)
                if name != OsStr::new(".") && name != OsStr::new("..")
                // Alternate streams do not have independent FileIdInfo identity.
                    && !name.encode_wide().any(|unit| unit == 0 || unit == u16::from(b':')) => {}
            _ => return Err(INVALID_PATH),
        }
    }
    Ok(())
}

fn directory_paths(path: &Path) -> Result<Vec<PathBuf>, &'static str> {
    validate_local_path(path)?;
    let mut paths = Vec::new();
    for ancestor in path.ancestors() {
        if paths.len() == MAX_DIRECTORY_PINS {
            return Err(CAPACITY);
        }
        paths.push(ancestor.to_path_buf());
    }
    paths.reverse();
    Ok(paths)
}

#[cfg(test)]
mod tests;
