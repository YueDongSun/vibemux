//! Immutable private chat bodies, accessed only inside the daemon writer.

use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use thiserror::Error;
use uuid::Uuid;
use vibemux_harness::dispatch::Sha256Digest;
use vibemux_types::{
    ProjectId,
    chat::{ContentRef, MAX_CHAT_RECORD_BYTES, MAX_PROJECT_CHAT_BYTES},
};

use crate::bounded_file::read_regular_file;

const ROOT_NAME: &str = "chat_private";
const OWNER_NAME: &str = "owner.json";
const OWNER_BYTES: u64 = 1024;
const MAX_PRIVATE_BLOBS: u64 = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum ChatPrivateError {
    #[error("private chat content reference is invalid")]
    InvalidReference,
    #[error("private chat content is unavailable or does not match its reference")]
    Unavailable,
    #[error("private chat content binding conflicts")]
    Conflict,
    #[error("private chat storage ownership or permissions could not be verified")]
    Protection,
    #[error("private chat content capacity is exhausted")]
    Capacity,
    #[error("private chat file operation {operation} failed ({kind:?}, OS {os_code:?})")]
    Io {
        operation: &'static str,
        kind: io::ErrorKind,
        os_code: Option<i32>,
    },
}

impl ChatPrivateError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidReference => "chat_content_invalid",
            Self::Unavailable => "chat_content_unavailable",
            Self::Conflict => "chat_content_conflict",
            Self::Protection => "chat_content_unprotected",
            Self::Capacity => "chat_content_capacity",
            Self::Io { .. } => "chat_content_io_failed",
        }
    }
}

fn io_failure(operation: &'static str, error: io::Error) -> ChatPrivateError {
    ChatPrivateError::Io {
        operation,
        kind: error.kind(),
        os_code: error.raw_os_error(),
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnerMarker {
    schema_version: u32,
    project_id: ProjectId,
}

/// This object has no independent thread or database connection. The chat
/// writer adapter executes every body operation in WriterWorker's queue.
#[derive(Clone)]
pub(crate) struct PrivateChatFiles {
    state_dir: PathBuf,
    root: PathBuf,
    project_id: ProjectId,
    state_identity: Arc<same_file::Handle>,
    root_identity: Arc<OnceLock<same_file::Handle>>,
}

pub(crate) struct BlobWrite {
    pub reference: ContentRef,
    pub created: bool,
}

impl fmt::Debug for BlobWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlobWrite")
            .field("created", &self.created)
            .field("byte_count", &self.reference.byte_count)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PrivateChatFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrivateChatFiles")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl PrivateChatFiles {
    pub fn new(state_dir: &Path, project_id: ProjectId) -> Result<Self, ChatPrivateError> {
        safe_directory(state_dir)?;
        let state_dir =
            fs::canonicalize(state_dir).map_err(|error| io_failure("resolve_state", error))?;
        let state_identity = same_file::Handle::from_path(&state_dir)
            .map_err(|error| io_failure("open_state", error))?;
        Ok(Self {
            root: state_dir.join(ROOT_NAME),
            state_dir,
            project_id,
            state_identity: Arc::new(state_identity),
            root_identity: Arc::new(OnceLock::new()),
        })
    }

    fn verify_state(&self) -> Result<(), ChatPrivateError> {
        safe_directory(&self.state_dir)?;
        let current = same_file::Handle::from_path(&self.state_dir)
            .map_err(|_| ChatPrivateError::Protection)?;
        if current != *self.state_identity {
            return Err(ChatPrivateError::Protection);
        }
        Ok(())
    }

    /// Called only after the explicit private-transcript consent. Reads do not
    /// create directories or repair permissions of an existing resource.
    pub fn initialize(&self) -> Result<(), ChatPrivateError> {
        self.verify_state()?;
        let created = match fs::create_dir(&self.root) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(io_failure("create_root", error)),
        };
        safe_directory(&self.root)?;
        if created {
            protect_new_root(&self.root)?;
        }
        verify_private(&self.root)?;
        let marker_path = self.root.join(OWNER_NAME);
        if !marker_path.exists() {
            // An empty protected directory is also the recoverable result of a
            // crash between creating the root and publishing its owner marker.
            if fs::read_dir(&self.root)
                .map_err(|error| io_failure("inspect_root", error))?
                .next()
                .is_some()
            {
                return Err(ChatPrivateError::Protection);
            }
            let marker = serde_json::to_vec(&OwnerMarker {
                schema_version: 1,
                project_id: self.project_id,
            })
            .map_err(|_| ChatPrivateError::Protection)?;
            let mut file = create_new_private(&marker_path)?;
            file.write_all(&marker)
                .map_err(|error| io_failure("write_owner", error))?;
            file.sync_all()
                .map_err(|error| io_failure("sync_owner", error))?;
        }
        self.verify_root()
    }

    fn verify_root(&self) -> Result<(), ChatPrivateError> {
        self.verify_state()?;
        safe_directory(&self.root)?;
        verify_private(&self.root)?;
        let current =
            same_file::Handle::from_path(&self.root).map_err(|_| ChatPrivateError::Protection)?;
        if let Some(known) = self.root_identity.get() {
            if known != &current {
                return Err(ChatPrivateError::Protection);
            }
        } else {
            let _ = self.root_identity.set(current);
        }
        let marker_path = self.root.join(OWNER_NAME);
        safe_file(&marker_path)?;
        let bytes = read_regular_file(&marker_path, OWNER_BYTES)
            .map_err(|_| ChatPrivateError::Protection)?;
        let marker: OwnerMarker =
            serde_json::from_slice(&bytes).map_err(|_| ChatPrivateError::Protection)?;
        if marker.schema_version != 1 || marker.project_id != self.project_id {
            return Err(ChatPrivateError::Protection);
        }
        Ok(())
    }

    fn blob_path(&self, blob_id: Uuid) -> PathBuf {
        self.root.join(format!("{}.blob", blob_id.simple()))
    }

    pub fn reference(blob_id: Uuid, bytes: &[u8]) -> Result<ContentRef, ChatPrivateError> {
        if blob_id.is_nil() || bytes.is_empty() || bytes.len() as u64 > MAX_CHAT_RECORD_BYTES {
            return Err(ChatPrivateError::InvalidReference);
        }
        Ok(ContentRef {
            blob_id,
            sha256: digest(bytes),
            byte_count: bytes.len() as u64,
        })
    }

    pub fn write_blob(&self, blob_id: Uuid, bytes: &[u8]) -> Result<BlobWrite, ChatPrivateError> {
        let reference = Self::reference(blob_id, bytes)?;
        self.verify_root()?;
        let destination = self.blob_path(blob_id);
        if destination
            .try_exists()
            .map_err(|error| io_failure("inspect_blob", error))?
        {
            return if self.read_blob(&reference).is_ok() {
                Ok(BlobWrite {
                    reference,
                    created: false,
                })
            } else {
                Err(ChatPrivateError::Conflict)
            };
        }
        self.check_capacity(reference.byte_count)?;
        let staging = self.root.join(format!("{}.tmp", Uuid::new_v4().simple()));
        let mut file = create_new_private(&staging)?;
        let identity = same_file::Handle::from_file(
            file.try_clone()
                .map_err(|error| io_failure("retain_staging", error))?,
        )
        .map_err(|error| io_failure("identify_staging", error))?;
        let result = (|| {
            file.write_all(bytes)
                .map_err(|error| io_failure("write_blob", error))?;
            file.sync_all()
                .map_err(|error| io_failure("sync_blob", error))?;
            self.verify_root()?;
            // Hard-link publication is create-only on Windows and POSIX. It
            // cannot replace a raced destination, unlike rename on POSIX.
            fs::hard_link(&staging, &destination).map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    ChatPrivateError::Conflict
                } else {
                    io_failure("publish_blob", error)
                }
            })?;
            Ok(BlobWrite {
                reference,
                created: true,
            })
        })();
        drop(file);
        let current =
            same_file::Handle::from_path(&staging).map_err(|_| ChatPrivateError::Protection)?;
        if current != identity {
            return Err(ChatPrivateError::Protection);
        }
        fs::remove_file(&staging).map_err(|error| io_failure("remove_staging", error))?;
        result
    }

    pub fn read_blob(&self, reference: &ContentRef) -> Result<Vec<u8>, ChatPrivateError> {
        reference
            .validate(MAX_CHAT_RECORD_BYTES)
            .map_err(|_| ChatPrivateError::InvalidReference)?;
        self.verify_root()?;
        let path = self.blob_path(reference.blob_id);
        safe_file(&path)?;
        let bytes = read_regular_file(&path, reference.byte_count)
            .map_err(|_| ChatPrivateError::Unavailable)?;
        if bytes.len() as u64 != reference.byte_count || digest(&bytes) != reference.sha256 {
            return Err(ChatPrivateError::Unavailable);
        }
        Ok(bytes)
    }

    /// The writer must first prove this reference is no longer present in the
    /// canonical content index. Changed/mismatched resources are never deleted.
    pub fn remove_blob(&self, reference: &ContentRef) -> Result<bool, ChatPrivateError> {
        self.verify_root()?;
        let path = self.blob_path(reference.blob_id);
        if !path
            .try_exists()
            .map_err(|error| io_failure("inspect_blob", error))?
        {
            return Ok(false);
        }
        self.read_blob(reference)?;
        fs::remove_file(path).map_err(|error| io_failure("remove_blob", error))?;
        Ok(true)
    }

    fn check_capacity(&self, incoming: u64) -> Result<(), ChatPrivateError> {
        let mut bytes = incoming;
        let mut files = 0_u64;
        for entry in
            fs::read_dir(&self.root).map_err(|error| io_failure("enumerate_content", error))?
        {
            let entry = entry.map_err(|error| io_failure("inspect_content", error))?;
            if entry.file_name() == OWNER_NAME {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| io_failure("inspect_content", error))?;
            if !metadata.is_file() || is_link(&metadata) {
                return Err(ChatPrivateError::Protection);
            }
            files = files.checked_add(1).ok_or(ChatPrivateError::Capacity)?;
            bytes = bytes
                .checked_add(metadata.len())
                .ok_or(ChatPrivateError::Capacity)?;
            if files >= MAX_PRIVATE_BLOBS || bytes > MAX_PROJECT_CHAT_BYTES {
                return Err(ChatPrivateError::Capacity);
            }
        }
        Ok(())
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256Digest::of(bytes).to_hex()
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn safe_directory(path: &Path) -> Result<(), ChatPrivateError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ChatPrivateError::Protection)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(ChatPrivateError::Protection);
    }
    Ok(())
}

fn safe_file(path: &Path) -> Result<(), ChatPrivateError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ChatPrivateError::Unavailable)?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(ChatPrivateError::Unavailable);
    }
    verify_private(path)
}

fn create_new_private(path: &Path) -> Result<fs::File, ChatPrivateError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| io_failure("create_file", error))
}

#[cfg(windows)]
fn protect_new_root(path: &Path) -> Result<(), ChatPrivateError> {
    vibemux_platform::secure_user_directory(path)
        .map(|_| ())
        .map_err(|_| ChatPrivateError::Protection)
}
#[cfg(unix)]
fn protect_new_root(path: &Path) -> Result<(), ChatPrivateError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_failure("protect_root", error))
}
#[cfg(windows)]
fn verify_private(path: &Path) -> Result<(), ChatPrivateError> {
    vibemux_platform::verify_restricted_path_acl(path)
        .map(|_| ())
        .map_err(|_| ChatPrivateError::Protection)
}
#[cfg(unix)]
fn verify_private(path: &Path) -> Result<(), ChatPrivateError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::symlink_metadata(path)
        .map_err(|_| ChatPrivateError::Protection)?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(ChatPrivateError::Protection);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use vibemux_types::ProjectId;

    #[test]
    fn private_bodies_round_trip_and_a_changed_operation_cannot_replace_them() {
        let temp = tempfile::tempdir().expect("private state");
        let project = ProjectId::from_uuid(Uuid::new_v4()).expect("project");
        let files = PrivateChatFiles::new(temp.path(), project).expect("paths");
        files.initialize().expect("protected root");
        let id = Uuid::new_v4();
        let written = files
            .write_blob(id, "private 中文 text".as_bytes())
            .expect("body");
        assert!(written.created);
        let reference = written.reference;
        assert_eq!(
            files.read_blob(&reference).expect("read"),
            "private 中文 text".as_bytes()
        );
        let duplicate = files
            .write_blob(id, "private 中文 text".as_bytes())
            .expect("duplicate");
        assert!(!duplicate.created);
        assert_eq!(duplicate.reference, reference);
        assert_eq!(
            files
                .write_blob(id, b"different")
                .expect_err("changed reuse")
                .code(),
            "chat_content_conflict"
        );
        assert_eq!(
            files.read_blob(&reference).expect("original retained"),
            "private 中文 text".as_bytes()
        );
    }

    #[test]
    fn private_reads_do_not_create_storage_and_bounds_precede_writes() {
        let temp = tempfile::tempdir().expect("state");
        let files = PrivateChatFiles::new(temp.path(), ProjectId::new()).expect("paths");
        let reference = ContentRef {
            blob_id: Uuid::new_v4(),
            sha256: "a".repeat(64),
            byte_count: 1,
        };
        assert!(files.read_blob(&reference).is_err());
        assert!(!temp.path().join(ROOT_NAME).exists());
        assert_eq!(
            files.write_blob(Uuid::nil(), b"x").expect_err("nil").code(),
            "chat_content_invalid"
        );
        assert_eq!(
            files
                .write_blob(
                    Uuid::new_v4(),
                    &vec![b'x'; MAX_CHAT_RECORD_BYTES as usize + 1]
                )
                .expect_err("oversized")
                .code(),
            "chat_content_invalid"
        );
        assert!(!temp.path().join(ROOT_NAME).exists());
    }

    #[test]
    fn wrong_project_and_changed_private_content_fail_closed() {
        let temp = tempfile::tempdir().expect("state");
        let files = PrivateChatFiles::new(temp.path(), ProjectId::new()).expect("paths");
        files.initialize().expect("root");
        let reference = files
            .write_blob(Uuid::new_v4(), b"private value")
            .expect("body")
            .reference;
        let other = PrivateChatFiles::new(temp.path(), ProjectId::new()).expect("other identity");
        assert_eq!(
            other.initialize().expect_err("wrong owner").code(),
            "chat_content_unprotected"
        );
        std::fs::write(files.blob_path(reference.blob_id), b"changed value")
            .expect("corruption fixture");
        assert_eq!(
            files.read_blob(&reference).expect_err("integrity").code(),
            "chat_content_unavailable"
        );
        assert!(files.remove_blob(&reference).is_err());
        assert!(
            files.blob_path(reference.blob_id).exists(),
            "changed resources are preserved"
        );
    }

    #[test]
    fn explicit_removal_is_idempotent_and_leaves_other_private_bodies() {
        let temp = tempfile::tempdir().expect("state");
        let files = PrivateChatFiles::new(temp.path(), ProjectId::new()).expect("paths");
        files.initialize().expect("root");
        let first = files
            .write_blob(Uuid::new_v4(), b"first")
            .expect("first")
            .reference;
        let second = files
            .write_blob(Uuid::new_v4(), b"second")
            .expect("second")
            .reference;
        assert!(files.remove_blob(&first).expect("removed"));
        assert!(!files.remove_blob(&first).expect("already absent"));
        assert_eq!(files.read_blob(&second).expect("other retained"), b"second");
    }

    #[test]
    fn an_existing_unknown_directory_is_not_adopted() {
        let temp = tempfile::tempdir().expect("state");
        let root = temp.path().join(ROOT_NAME);
        std::fs::create_dir(&root).expect("unknown directory");
        std::fs::write(root.join("unrelated.txt"), b"preserve").expect("unrelated file");
        let files = PrivateChatFiles::new(temp.path(), ProjectId::new()).expect("paths");
        assert!(files.initialize().is_err());
        assert_eq!(
            std::fs::read(root.join("unrelated.txt")).expect("preserved"),
            b"preserve"
        );
    }
}
