//! Per-user protection of the daemon-private workflow state (ADR 031 §7).
//!
//! The workflow state directory holds the opt-in content store, routed
//! message bodies, plans, and candidate bytes. Before the service uses it,
//! it is restricted to the daemon's user: on Windows to the same protected
//! three-rule ACL as the control runtime root (current user,
//! `LOCAL_SYSTEM`, built-in administrators), which every file created
//! below it inherits; on POSIX to mode `0700`. The restriction is checked
//! natively on every start, so the Windows helper runs only when the
//! directory is new or was loosened; a loosened directory is narrowed
//! again, never widened. A directory that cannot be restricted fails
//! closed. Same-user processes are not isolated by this, as ADR 031
//! states.

use std::path::Path;

use super::error::WorkflowError;

/// Creates `root` if needed and restricts it to the current user.
/// Blocking.
pub(crate) fn protect_state_root(root: &Path) -> Result<(), WorkflowError> {
    std::fs::create_dir_all(root).map_err(|_| WorkflowError::StateUnprotected)?;
    let metadata = std::fs::symlink_metadata(root).map_err(|_| WorkflowError::StateUnprotected)?;
    // A link could point the private state anywhere.
    if !metadata.is_dir() {
        return Err(WorkflowError::StateUnprotected);
    }
    restrict(root)
}

#[cfg(windows)]
fn restrict(root: &Path) -> Result<(), WorkflowError> {
    if vibemux_platform::verify_restricted_path_acl(root).is_ok() {
        return Ok(());
    }
    vibemux_platform::secure_user_directory(root).map_err(|_| WorkflowError::StateUnprotected)?;
    vibemux_platform::verify_restricted_path_acl(root)
        .map(|_| ())
        .map_err(|_| WorkflowError::StateUnprotected)
}

#[cfg(unix)]
fn restrict(root: &Path) -> Result<(), WorkflowError> {
    use std::os::unix::fs::PermissionsExt;

    const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
    // Changing the mode needs ownership, so success also proves the
    // daemon's user owns the directory.
    std::fs::set_permissions(
        root,
        std::fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE),
    )
    .map_err(|_| WorkflowError::StateUnprotected)?;
    let mode = std::fs::symlink_metadata(root)
        .map_err(|_| WorkflowError::StateUnprotected)?
        .permissions()
        .mode();
    if mode & 0o777 == PRIVATE_DIRECTORY_MODE {
        Ok(())
    } else {
        Err(WorkflowError::StateUnprotected)
    }
}

#[cfg(not(any(windows, unix)))]
fn restrict(_root: &Path) -> Result<(), WorkflowError> {
    Err(WorkflowError::StateUnprotected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn the_state_root_and_its_files_are_restricted_and_a_loosened_root_is_narrowed() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("workflow_state");
        protect_state_root(&root).expect("protect");
        let content = root.join("content");
        std::fs::create_dir_all(&content).expect("content directory");
        let blob = content.join("blob.txt");
        std::fs::write(&blob, b"private bundle text").expect("blob");
        vibemux_platform::verify_restricted_path_acls(&[&root, &content, &blob])
            .expect("the state root and everything below it are restricted");

        // Built-in Users gain full control: no longer exactly three rules.
        vibemux_platform::test_helpers::loosen_with_icacls(&root, "*S-1-5-32-545:(OI)(CI)F");
        assert!(vibemux_platform::verify_restricted_path_acl(&root).is_err());
        protect_state_root(&root).expect("narrow again");
        vibemux_platform::verify_restricted_path_acls(&[&root, &content, &blob])
            .expect("restricted again");
    }

    #[cfg(unix)]
    #[test]
    fn the_state_root_is_private_to_its_owner_even_if_it_was_widened() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("workflow_state");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).expect("widen");
        protect_state_root(&root).expect("protect");
        let mode = std::fs::metadata(&root)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_file_in_place_of_the_state_root_fails_closed() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("workflow_state");
        std::fs::write(&root, b"not a directory").expect("file");
        assert_eq!(
            protect_state_root(&root),
            Err(WorkflowError::StateUnprotected)
        );
    }
}
