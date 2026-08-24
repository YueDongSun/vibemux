//! Bounded, redacted snapshots of the daemon writer lock for explicit recovery.

use std::{
    fmt,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_WRITER_LOCK_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriterLockInfo {
    pub process_id: u32,
}

pub struct WriterLockSnapshot {
    path: PathBuf,
    encoded: Vec<u8>,
    info: WriterLockInfo,
}

impl fmt::Debug for WriterLockSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WriterLockSnapshot")
            .field("info", &self.info)
            .field("contents", &"[redacted]")
            .finish()
    }
}

impl WriterLockSnapshot {
    #[must_use]
    pub const fn info(&self) -> WriterLockInfo {
        self.info
    }

    #[must_use]
    pub fn binding_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"vibemux-writer-lock-v1\0");
        hasher.update(&self.encoded);
        hasher.finalize().into()
    }

    pub fn remove_if_unchanged(&self) -> Result<(), RuntimeArtifactError> {
        let current = read_regular_bounded(&self.path, MAX_WRITER_LOCK_BYTES)?;
        if current != self.encoded {
            return Err(RuntimeArtifactError::Changed);
        }
        std::fs::remove_file(&self.path).map_err(|_| RuntimeArtifactError::CleanupFailed)
    }
}

pub fn inspect_writer_lock(
    path: &Path,
) -> Result<Option<WriterLockSnapshot>, RuntimeArtifactError> {
    let encoded = match read_optional_regular_bounded(path, MAX_WRITER_LOCK_BYTES)? {
        Some(encoded) => encoded,
        None => return Ok(None),
    };
    let encoded_contents =
        std::str::from_utf8(&encoded).map_err(|_| RuntimeArtifactError::Invalid)?;
    let contents = encoded_contents
        .strip_suffix("\r\n")
        .or_else(|| encoded_contents.strip_suffix('\n'))
        .unwrap_or(encoded_contents);
    if contents.trim() != contents || contents.contains('\r') || contents.contains('\n') {
        return Err(RuntimeArtifactError::Invalid);
    }
    let mut parts = contents.split('-');
    let process_id = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|process_id| *process_id != 0)
        .ok_or(RuntimeArtifactError::Invalid)?;
    parts
        .next()
        .and_then(|value| value.parse::<u128>().ok())
        .ok_or(RuntimeArtifactError::Invalid)?;
    parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(RuntimeArtifactError::Invalid)?;
    if parts.next().is_some() {
        return Err(RuntimeArtifactError::Invalid);
    }
    Ok(Some(WriterLockSnapshot {
        path: path.to_path_buf(),
        encoded,
        info: WriterLockInfo { process_id },
    }))
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RuntimeArtifactError {
    #[error("runtime artifact is invalid")]
    Invalid,
    #[error("runtime artifact path is unsafe")]
    UnsafePath,
    #[error("runtime artifact changed after inspection")]
    Changed,
    #[error("runtime artifact cleanup failed")]
    CleanupFailed,
    #[error("runtime artifact is inaccessible")]
    Inaccessible,
}

impl RuntimeArtifactError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Invalid => "recovery_artifact_invalid",
            Self::UnsafePath => "recovery_artifact_unsafe_path",
            Self::Changed => "recovery_artifact_changed",
            Self::CleanupFailed => "recovery_artifact_cleanup_failed",
            Self::Inaccessible => "recovery_artifact_inaccessible",
        }
    }
}

fn read_regular_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, RuntimeArtifactError> {
    read_optional_regular_bounded(path, maximum)?.ok_or(RuntimeArtifactError::Changed)
}

fn read_optional_regular_bounded(
    path: &Path,
    maximum: usize,
) -> Result<Option<Vec<u8>>, RuntimeArtifactError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(RuntimeArtifactError::Inaccessible),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(RuntimeArtifactError::UnsafePath);
    }
    if metadata.len() == 0 || metadata.len() > maximum as u64 {
        return Err(RuntimeArtifactError::Invalid);
    }
    let file = File::open(path).map_err(|_| RuntimeArtifactError::Inaccessible)?;
    let mut encoded = Vec::new();
    file.take((maximum + 1) as u64)
        .read_to_end(&mut encoded)
        .map_err(|_| RuntimeArtifactError::Inaccessible)?;
    if encoded.is_empty() || encoded.len() > maximum {
        return Err(RuntimeArtifactError::Invalid);
    }
    Ok(Some(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_lock_is_redacted_and_removed_only_when_unchanged() {
        let temp = tempfile::tempdir().expect("temp directory");
        let path = temp.path().join("state.writer.lock");
        std::fs::write(&path, b"42-123456789-7\n").expect("writer lock");
        let snapshot = inspect_writer_lock(&path)
            .expect("inspect lock")
            .expect("lock snapshot");
        assert_eq!(snapshot.info().process_id, 42);
        assert!(!format!("{snapshot:?}").contains("123456789"));
        snapshot
            .remove_if_unchanged()
            .expect("remove unchanged lock");
        assert!(!path.exists());
    }

    #[test]
    fn changed_or_malformed_lock_fails_closed() {
        let temp = tempfile::tempdir().expect("temp directory");
        let path = temp.path().join("state.writer.lock");
        std::fs::write(&path, b"42-123456789-7").expect("writer lock");
        let snapshot = inspect_writer_lock(&path)
            .expect("inspect lock")
            .expect("lock snapshot");
        std::fs::write(&path, b"43-123456790-8").expect("replacement lock");
        assert_eq!(
            snapshot
                .remove_if_unchanged()
                .expect_err("changed lock must fail"),
            RuntimeArtifactError::Changed
        );
        assert!(path.exists());

        std::fs::write(&path, b"not-a-valid-lock").expect("malformed lock");
        assert_eq!(
            inspect_writer_lock(&path).expect_err("malformed lock must fail"),
            RuntimeArtifactError::Invalid
        );
    }
}
