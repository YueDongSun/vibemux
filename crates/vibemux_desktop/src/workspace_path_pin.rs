//! Native directory handles retained across a fresh Core check and OS handoff.
use crate::command_error::CommandError;
use same_file::Handle;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

const MAX_DIRECTORY_ANCESTORS: usize = 256;

pub(crate) struct WorkspacePathPin {
    canonical_root: PathBuf,
    canonical_directory: PathBuf,
    handles: Vec<(PathBuf, Handle)>,
}

impl WorkspacePathPin {
    pub(crate) fn acquire(root: &Path, directory: &Path) -> Result<Self, CommandError> {
        if !root.is_absolute() || !directory.is_absolute() {
            return Err(changed());
        }
        let canonical_root = fs::canonicalize(root).map_err(|_| changed())?;
        let canonical_directory = fs::canonicalize(directory).map_err(|_| changed())?;
        if canonical_root == canonical_directory
            || !canonical_directory.starts_with(&canonical_root)
        {
            return Err(changed());
        }
        let root_identity = Handle::from_path(&canonical_root).map_err(|_| changed())?;
        let mut handles = Vec::new();
        let mut reached_root = false;
        for path in directory.ancestors().take(MAX_DIRECTORY_ANCESTORS) {
            let file = ordinary_directory(path)?;
            let identity = Handle::from_file(file).map_err(|_| changed())?;
            let is_root = identity == root_identity;
            handles.push((path.to_path_buf(), identity));
            if is_root {
                reached_root = true;
                break;
            }
        }
        // Canonical containment is preliminary: the pinned ancestry must reach
        // the actual registered-root file object too.
        if !reached_root || handles.len() < 2 {
            return Err(changed());
        }
        let pin = Self {
            canonical_root,
            canonical_directory,
            handles,
        };
        pin.recheck()?;
        Ok(pin)
    }

    pub(crate) fn recheck(&self) -> Result<(), CommandError> {
        for (path, identity) in &self.handles {
            require_ordinary_directory(path)?;
            let current = Handle::from_path(path).map_err(|_| changed())?;
            if &current != identity {
                return Err(changed());
            }
        }
        let directory = self.handles.first().ok_or_else(changed)?;
        let root = self.handles.last().ok_or_else(changed)?;
        if fs::canonicalize(&directory.0).map_err(|_| changed())? != self.canonical_directory
            || fs::canonicalize(&root.0).map_err(|_| changed())? != self.canonical_root
        {
            return Err(changed());
        }
        Ok(())
    }
}

fn ordinary_directory(path: &Path) -> Result<File, CommandError> {
    require_ordinary_directory(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Retain real read access, allow compatible reads/writes, deny delete
        // sharing. Attribute-only handles do not exclude namespace rename.
        // OPEN_REPARSE_POINT ensures the final entry is inspected, not followed.
        options
            .share_mode(3)
            .custom_flags(0x0200_0000 | 0x0020_0000);
    }
    let file = options.open(path).map_err(|_| changed())?;
    let metadata = file.metadata().map_err(|_| changed())?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(changed());
    }
    Ok(file)
}

fn require_ordinary_directory(path: &Path) -> Result<(), CommandError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| changed())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse(&metadata) {
        return Err(changed());
    }
    Ok(())
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

fn changed() -> CommandError {
    CommandError::new(
        "workspace_directory_changed",
        "The worktree directory identity changed. Refresh before another action.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_accept_only_existing_descendants_and_preserve_ordinary_writes() {
        let temp = tempfile::tempdir().expect("owned root");
        let root = fs::canonicalize(temp.path()).expect("root");
        let directory = root.join("managed").join("run");
        fs::create_dir_all(&directory).expect("owned directories");
        let pin = WorkspacePathPin::acquire(&root, &directory).expect("pins");
        fs::write(directory.join("ordinary.txt"), "fixture").expect("compatible writes");
        pin.recheck().expect("same identity");
        assert!(WorkspacePathPin::acquire(&root, &root).is_err());
        assert!(WorkspacePathPin::acquire(&root, &root.join("missing")).is_err());
        let foreign = tempfile::tempdir().expect("foreign owned fixture");
        assert!(WorkspacePathPin::acquire(&root, foreign.path()).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_directory_pins_use_file_identity_across_path_case() {
        let temp = tempfile::tempdir().expect("owned root");
        let root = fs::canonicalize(temp.path()).expect("root");
        let directory = root.join("MixedCase").join("Run");
        fs::create_dir_all(&directory).expect("owned directories");
        let root_case = PathBuf::from(root.to_string_lossy().to_ascii_uppercase());
        let pin = WorkspacePathPin::acquire(&root_case, &directory).expect("same root identity");
        pin.recheck().expect("case-insensitive path authority");
    }

    #[cfg(windows)]
    #[test]
    fn windows_pins_deny_namespace_rename_until_release() {
        let temp = tempfile::tempdir().expect("owned root");
        let root = fs::canonicalize(temp.path()).expect("root");
        let directory = root.join("run");
        fs::create_dir(&directory).expect("owned directory");
        let pin = WorkspacePathPin::acquire(&root, &directory).expect("pins");
        let moved = root.join("moved");
        assert!(fs::rename(&directory, &moved).is_err());
        pin.recheck().expect("identity retained");
        drop(pin);
        fs::rename(&directory, &moved).expect("released owned directory");
    }

    #[test]
    fn reparse_ancestry_is_refused_even_when_its_target_is_inside_the_root() {
        let temp = tempfile::tempdir().expect("owned root");
        // Windows PowerShell 5.1's junction builder rejects verbatim paths.
        let root = temp.path().to_path_buf();
        let target = root.join("ordinary");
        let directory = target.join("run");
        fs::create_dir_all(&directory).expect("ordinary directories");
        fs::write(directory.join("sentinel"), "untouched").expect("sentinel");
        let link = root.join("linked");
        #[cfg(windows)]
        {
            use std::{os::windows::process::CommandExt, process::Command};
            let powershell = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            let status = Command::new(powershell)
                .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:VIBEMUX_TEST_LINK -Target $env:VIBEMUX_TEST_TARGET | Out-Null"])
                .env("VIBEMUX_TEST_LINK", &link).env("VIBEMUX_TEST_TARGET", &target)
                .creation_flags(0x0800_0000).status().expect("junction fixture");
            assert!(status.success(), "the reparse fixture must exist");
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("symlink fixture");
        assert!(WorkspacePathPin::acquire(&root, &link.join("run")).is_err());
        assert_eq!(
            fs::read_to_string(directory.join("sentinel")).expect("sentinel"),
            "untouched"
        );
        #[cfg(windows)]
        fs::remove_dir(link).expect("remove fixture junction only");
        #[cfg(unix)]
        fs::remove_file(link).expect("remove fixture symlink only");
    }

    #[cfg(unix)]
    #[test]
    fn posix_recheck_refuses_an_owned_directory_replaced_after_pinning() {
        let temp = tempfile::tempdir().expect("owned root");
        let root = fs::canonicalize(temp.path()).expect("root");
        let directory = root.join("run");
        fs::create_dir(&directory).expect("owned directory");
        let pin = WorkspacePathPin::acquire(&root, &directory).expect("pins");
        fs::rename(&directory, root.join("moved")).expect("rename owned fixture");
        fs::create_dir(&directory).expect("new directory");
        assert!(pin.recheck().is_err());
    }
}
