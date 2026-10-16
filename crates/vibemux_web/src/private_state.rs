//! File ownership and atomic persistence for web-only state, outside canonical storage.
use fs4::fs_std::FileExt;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
#[error("web_private_state_invalid")]
pub struct StateError;

pub fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, StateError> {
    reject_linked_path(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| StateError)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max_bytes as u64
    {
        return Err(StateError);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| StateError)?
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| StateError)?;
    if bytes.len() > max_bytes {
        return Err(StateError);
    }
    Ok(bytes)
}

pub fn verify_private_file(path: &Path) -> Result<(), StateError> {
    reject_linked_path(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| StateError)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(StateError);
    }
    #[cfg(windows)]
    vibemux_platform::verify_restricted_path_acl(path).map_err(|_| StateError)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0
            || metadata.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(StateError);
        }
    }
    Ok(())
}

pub fn prepare_directory(path: &Path) -> Result<(), StateError> {
    reject_linked_path(path)?;
    let existed = path.exists();
    std::fs::create_dir_all(path).map_err(|_| StateError)?;
    #[cfg(windows)]
    {
        if existed {
            vibemux_platform::verify_restricted_path_acl(path).map_err(|_| StateError)?;
        } else {
            vibemux_platform::secure_user_directory(path).map_err(|_| StateError)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if existed {
            let metadata = std::fs::metadata(path).map_err(|_| StateError)?;
            if metadata.permissions().mode() & 0o077 != 0
                || metadata.uid() != rustix::process::geteuid().as_raw()
            {
                return Err(StateError);
            }
        } else {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| StateError)?;
        }
    }
    Ok(())
}

fn reject_linked_path(path: &Path) -> Result<(), StateError> {
    // Never follow an operator-supplied link/reparse point for configuration or secrets.
    for ancestor in path.ancestors() {
        if let Ok(metadata) = std::fs::symlink_metadata(ancestor) {
            if metadata.file_type().is_symlink() {
                return Err(StateError);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(StateError);
                }
            }
        }
    }
    Ok(())
}

pub fn create_private_file(path: &Path) -> Result<File, StateError> {
    let mut options = OpenOptions::new();
    options.write(true).read(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| StateError)?;
    verify_private_file(path)?;
    Ok(file)
}

pub fn atomic_save(path: &Path, bytes: &[u8]) -> Result<(), StateError> {
    if path.exists() {
        verify_private_file(path)?;
    }
    let parent = path.parent().ok_or(StateError)?;
    let temporary = parent.join(format!(
        "pending_{}.json",
        crate::enrollment::random_token().map_err(|_| StateError)?
    ));
    let result = (|| {
        let mut file = create_private_file(&temporary)?;
        file.write_all(bytes).map_err(|_| StateError)?;
        file.sync_all().map_err(|_| StateError)?;
        drop(file);
        // std rename atomically replaces a file on supported Windows and POSIX filesystems.
        std::fs::rename(&temporary, path).map_err(|_| StateError)?;
        verify_private_file(path)
    })();
    if temporary.exists() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub struct InstanceLease {
    _file: File,
    pub state_dir: PathBuf,
}
impl InstanceLease {
    pub fn acquire(project_root: &Path) -> Result<Self, StateError> {
        let state_dir = project_root.join(".vibemux/web");
        prepare_directory(&state_dir)?;
        let path = state_dir.join("instance.lock");
        let file = if path.exists() {
            verify_private_file(&path)?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|_| StateError)?
        } else {
            create_private_file(&path)?
        };
        if !file.try_lock_exclusive().map_err(|_| StateError)? {
            return Err(StateError);
        }
        Ok(Self {
            _file: file,
            state_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn insecure_files_and_existing_directories_fail_closed_without_adoption() {
        let temp = tempfile::tempdir().unwrap();
        let lease = InstanceLease::acquire(temp.path()).unwrap();
        let path = lease.state_dir.join("private_test.json");
        atomic_save(&path, b"{}").unwrap();
        vibemux_platform::test_helpers::loosen_with_icacls(&path, "*S-1-1-0:R");
        assert!(verify_private_file(&path).is_err());
        vibemux_platform::test_helpers::restore_with_icacls(&path, "*S-1-1-0");
        verify_private_file(&path).unwrap();
        vibemux_platform::test_helpers::loosen_with_icacls(&lease.state_dir, "*S-1-1-0:R");
        assert!(prepare_directory(&lease.state_dir).is_err());
        assert!(vibemux_platform::verify_restricted_path_acl(&lease.state_dir).is_err());
        vibemux_platform::test_helpers::restore_with_icacls(&lease.state_dir, "*S-1-1-0");
        prepare_directory(&lease.state_dir).unwrap();
    }
    #[test]
    fn private_persistence_and_instance_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let lease = InstanceLease::acquire(temp.path()).unwrap();
        assert!(InstanceLease::acquire(temp.path()).is_err());
        let path = lease.state_dir.join("test.json");
        atomic_save(&path, b"first").unwrap();
        atomic_save(&path, b"second").unwrap();
        assert_eq!(read_bounded(&path, 6).unwrap(), b"second");
        assert!(read_bounded(&path, 5).is_err());
        verify_private_file(&path).unwrap();
        drop(lease);
        assert!(InstanceLease::acquire(temp.path()).is_ok());
    }
}
