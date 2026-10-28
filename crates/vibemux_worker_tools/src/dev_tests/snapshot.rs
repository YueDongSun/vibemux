use super::DevTestRequest;
use crate::{contract::ToolError, workspace::verify_private_parent};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use vibemux_workflow::Sha256Digest;

/// The parent/root and owner marker are outside the candidate's writable tree.
/// No Drop removal: loss of the execution future must retain recovery evidence.
pub(super) struct Snapshot {
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub scratch: PathBuf,
    pub identity: Sha256Digest,
    owner: Vec<u8>,
}

impl Snapshot {
    pub fn create(parent: &Path, request: &DevTestRequest) -> Result<Self, ToolError> {
        request.validate()?;
        let parent = verify_private_parent(parent)?;
        let directory = tempfile::Builder::new()
            .prefix("vibemux_dev_test_")
            .tempdir_in(&parent)
            .map_err(|_| ToolError::Io)?;
        let root = directory.path().canonicalize().map_err(|_| ToolError::Io)?;
        #[cfg(windows)]
        {
            vibemux_platform::secure_user_directory(&root).map_err(|_| ToolError::Io)?;
            vibemux_platform::verify_restricted_path_acl(&root).map_err(|_| ToolError::Io)?;
        }
        let workspace = root.join("workspace");
        let scratch = workspace.join(".vibemux_test_scratch");
        fs::create_dir(&workspace)
            .and_then(|()| fs::create_dir(&scratch))
            .map_err(|_| ToolError::Io)?;
        let mut files = request.files.clone();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let identity = super::input_identity(&files)?;
        let owner = serde_json::to_vec(&serde_json::json!({
            "schema_version":super::DEV_TEST_SCHEMA,
            "attempt_id":request.attempt_id, "operation_id":request.operation_id,
            "suite_id":request.suite_id, "input_snapshot":identity,
        }))
        .map_err(|_| ToolError::InvalidBootstrap)?;
        let mut owner_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("owner.json"))
            .map_err(|_| ToolError::Io)?;
        owner_file
            .write_all(&owner)
            .and_then(|()| owner_file.sync_all())
            .map_err(|_| ToolError::Io)?;
        drop(owner_file);
        for input in files {
            let path = workspace.join(&input.path);
            fs::create_dir_all(path.parent().ok_or(ToolError::InvalidPath)?)
                .map_err(|_| ToolError::Io)?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|_| ToolError::Io)?;
            file.write_all(input.content.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|_| ToolError::Io)?;
        }
        let _ = directory.keep();
        Ok(Self {
            root,
            workspace,
            scratch,
            identity,
            owner,
        })
    }

    /// Only called after independent process/Job quiescence (or before spawn).
    /// Rust remove_dir_all does not traverse candidate-created reparse links.
    pub fn remove(&self) -> std::io::Result<()> {
        let ordinary_directory = |path: &Path| {
            fs::symlink_metadata(path).is_ok_and(|metadata| {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    metadata.is_dir() && metadata.file_attributes() & 0x400 == 0
                }
                #[cfg(not(windows))]
                {
                    metadata.is_dir() && !metadata.file_type().is_symlink()
                }
            })
        };
        if !ordinary_directory(&self.root)
            || self
                .root
                .parent()
                .is_none_or(|parent| !ordinary_directory(parent))
            || fs::read(self.root.join("owner.json")).ok().as_deref() != Some(&self.owner)
        {
            return Err(std::io::Error::other("snapshot owner binding changed"));
        }
        // Remove the candidate-controlled subtree before its immutable owner
        // record. A sharing/permission failure there must retain attribution.
        fs::remove_dir_all(&self.workspace)?;
        fs::remove_file(self.root.join("owner.json"))?;
        fs::remove_dir(&self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn failed_workspace_cleanup_preserves_owner_marker() {
        let root = tempfile::TempDir::new().unwrap();
        vibemux_platform::secure_user_directory(root.path()).unwrap();
        let request: DevTestRequest = serde_json::from_str(include_str!(
            "../../../../examples/worker_tools/development_test.json"
        ))
        .unwrap();
        let snapshot = Snapshot::create(root.path(), &request).unwrap();
        let marker = snapshot.root.join("owner.json");
        let original = fs::read(&marker).unwrap();
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(snapshot.workspace.join("src/value.cjs"))
            .unwrap();
        let result = snapshot.remove();
        let retained_marker = fs::read(&marker);
        drop(locked);
        assert!(result.is_err());
        assert_eq!(retained_marker.unwrap(), original);
        snapshot.remove().unwrap();
    }
}
