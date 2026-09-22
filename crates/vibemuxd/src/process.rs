//! Canonical project paths for the standalone Rust daemon process.

use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::{fs::OpenOptions, io::Write};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{control::DESCRIPTOR_FILE_NAME, writer_lock_path_for_database};

pub const RUNTIME_DIR_NAME: &str = ".vibemux";
pub const RUST_DATABASE_FILE_NAME: &str = "vibemux_rust.sqlite3";
pub const PYTHON_DATABASE_FILE_NAME: &str = "vibemux.sqlite3";
pub const CONTROL_WRITER_LOCK_FILE_NAME: &str = "writer.lock";
#[cfg(windows)]
const CONTROL_ACL_MARKER_FILE_NAME: &str = ".acl_v1";
#[cfg(windows)]
const CONTROL_ACL_MARKER_CONTENTS: &[u8] = b"vibemux_control_acl_v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonPaths {
    project_root: PathBuf,
    state_dir: PathBuf,
    control_runtime_root: PathBuf,
    runtime_dir: PathBuf,
    runtime_key: String,
    database_path: PathBuf,
    descriptor_path: PathBuf,
    writer_lock_path: PathBuf,
    legacy_descriptor_path: PathBuf,
    legacy_writer_lock_path: PathBuf,
}

/// Canonical probe-cache location for a state directory. This is the SINGLE
/// owner of `<state_dir>/probe_cache.json`: `DaemonPaths::probe_cache_path`
/// and the explicit trusted-startup constructors (`ServerState::start` and
/// `start_with_plugins`, which only receive a database path) both route
/// through here, so the location can never have two maintainers.
#[must_use]
pub fn probe_cache_path_for_state_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(vibemux_probe::cache::PROBE_CACHE_FILE_NAME)
}

impl DaemonPaths {
    pub fn from_project_root(project_root: &Path) -> Result<Self, DaemonPathError> {
        let project_root =
            std::fs::canonicalize(project_root).map_err(|_| DaemonPathError::InvalidProjectRoot)?;
        if !project_root.is_dir() {
            return Err(DaemonPathError::InvalidProjectRoot);
        }
        let state_dir = project_root.join(RUNTIME_DIR_NAME);
        let database_path = state_dir.join(RUST_DATABASE_FILE_NAME);
        let runtime_key = project_runtime_key(&project_root);
        #[cfg(windows)]
        let (control_runtime_root, runtime_dir) = {
            let local_app_data = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .ok_or(DaemonPathError::ControlRuntimeUnavailable)?;
            let local_app_data = std::fs::canonicalize(local_app_data)
                .map_err(|_| DaemonPathError::ControlRuntimeUnavailable)?;
            let control_runtime_root = local_app_data.join("VibeMux").join("runtime");
            let runtime_dir = control_runtime_root.join(&runtime_key);
            (control_runtime_root, runtime_dir)
        };
        #[cfg(unix)]
        let (control_runtime_root, runtime_dir) = (state_dir.clone(), state_dir.clone());
        let descriptor_path = runtime_dir.join(DESCRIPTOR_FILE_NAME);
        #[cfg(windows)]
        let writer_lock_path = runtime_dir.join(CONTROL_WRITER_LOCK_FILE_NAME);
        #[cfg(unix)]
        let writer_lock_path = writer_lock_path_for_database(&database_path);
        let legacy_descriptor_path = state_dir.join(DESCRIPTOR_FILE_NAME);
        let legacy_writer_lock_path = writer_lock_path_for_database(&database_path);
        Ok(Self {
            project_root,
            state_dir,
            control_runtime_root,
            runtime_dir,
            runtime_key,
            database_path,
            descriptor_path,
            writer_lock_path,
            legacy_descriptor_path,
            legacy_writer_lock_path,
        })
    }

    pub fn ensure_runtime_dir(&self) -> Result<(), DaemonPathError> {
        std::fs::create_dir_all(&self.state_dir)
            .map_err(|_| DaemonPathError::RuntimeDirectoryUnavailable)?;
        #[cfg(windows)]
        {
            std::fs::create_dir_all(&self.control_runtime_root)
                .map_err(|_| DaemonPathError::ControlRuntimeUnavailable)?;
            self.ensure_windows_control_acl()?;
        }
        std::fs::create_dir_all(&self.runtime_dir)
            .map_err(|_| DaemonPathError::ControlRuntimeUnavailable)?;
        self.validate_runtime_dir()
    }

    pub fn validate_runtime_dir(&self) -> Result<(), DaemonPathError> {
        self.validate_state_dir()?;
        let canonical_control_root = std::fs::canonicalize(&self.control_runtime_root)
            .map_err(|_| DaemonPathError::ControlRuntimeUnavailable)?;
        if canonical_control_root != self.control_runtime_root {
            return Err(DaemonPathError::UnsafeControlRuntime);
        }
        let canonical_runtime = std::fs::canonicalize(&self.runtime_dir)
            .map_err(|_| DaemonPathError::ControlRuntimeUnavailable)?;
        if canonical_runtime != self.runtime_dir {
            return Err(DaemonPathError::UnsafeControlRuntime);
        }
        #[cfg(windows)]
        self.validate_windows_acl_marker()?;
        Ok(())
    }

    pub fn validate_state_dir(&self) -> Result<(), DaemonPathError> {
        let canonical_state = std::fs::canonicalize(&self.state_dir)
            .map_err(|_| DaemonPathError::RuntimeDirectoryUnavailable)?;
        if canonical_state != self.state_dir {
            return Err(DaemonPathError::UnsafeRuntimeDirectory);
        }
        if std::fs::symlink_metadata(&self.database_path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(DaemonPathError::UnsafeDatabasePath);
        }
        let python_database_path = self.python_database_path();
        if self.database_path.exists()
            && python_database_path.exists()
            && same_file::is_same_file(&self.database_path, &python_database_path)
                .map_err(|_| DaemonPathError::UnsafeDatabasePath)?
        {
            return Err(DaemonPathError::DatabaseCollision);
        }
        Ok(())
    }

    pub fn validate_daemon_start(&self) -> Result<(), DaemonPathError> {
        if self.has_distinct_legacy_runtime() {
            match std::fs::symlink_metadata(&self.legacy_descriptor_path) {
                Ok(_) => return Err(DaemonPathError::LegacyRuntimeConflict),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(DaemonPathError::LegacyRuntimeConflict),
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    #[must_use]
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    #[must_use]
    pub fn control_runtime_root(&self) -> &Path {
        &self.control_runtime_root
    }

    #[must_use]
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    #[must_use]
    pub fn descriptor_path(&self) -> &Path {
        &self.descriptor_path
    }

    #[must_use]
    pub fn writer_lock_path(&self) -> &Path {
        &self.writer_lock_path
    }

    #[must_use]
    pub fn legacy_descriptor_path(&self) -> &Path {
        &self.legacy_descriptor_path
    }

    #[must_use]
    pub fn legacy_writer_lock_path(&self) -> &Path {
        &self.legacy_writer_lock_path
    }

    #[must_use]
    pub fn has_distinct_legacy_runtime(&self) -> bool {
        self.descriptor_path != self.legacy_descriptor_path
            || self.writer_lock_path != self.legacy_writer_lock_path
    }

    #[must_use]
    pub fn runtime_key(&self) -> &str {
        &self.runtime_key
    }

    #[must_use]
    pub fn python_database_path(&self) -> PathBuf {
        self.state_dir.join(PYTHON_DATABASE_FILE_NAME)
    }

    /// The trusted probe cache location: `<state_dir>/probe_cache.json`. The
    /// file name is owned by `vibemux_probe::cache` so the probe writer and
    /// the daemon reader can never disagree about the location.
    #[must_use]
    pub fn probe_cache_path(&self) -> PathBuf {
        probe_cache_path_for_state_dir(&self.state_dir)
    }

    #[cfg(windows)]
    pub fn verify_control_runtime_acl(&self) -> Result<(), DaemonPathError> {
        vibemux_platform::verify_restricted_path_acl(&self.control_runtime_root)
            .and_then(|_| vibemux_platform::verify_restricted_path_acl(&self.runtime_dir))
            .map(|_| ())
            .map_err(|_| DaemonPathError::ControlRuntimeSecurityInvalid)
    }

    #[cfg(windows)]
    pub fn verify_control_artifact_acls(&self) -> Result<(), DaemonPathError> {
        vibemux_platform::verify_restricted_path_acl(&self.descriptor_path)
            .and_then(|_| vibemux_platform::verify_restricted_path_acl(&self.writer_lock_path))
            .map(|_| ())
            .map_err(|_| DaemonPathError::ControlRuntimeSecurityInvalid)
    }

    #[cfg(windows)]
    fn ensure_windows_control_acl(&self) -> Result<(), DaemonPathError> {
        let marker = self.control_runtime_root.join(CONTROL_ACL_MARKER_FILE_NAME);
        if acl_marker_valid(&marker) {
            return Ok(());
        }
        vibemux_platform::secure_user_directory(&self.control_runtime_root)
            .map_err(|_| DaemonPathError::ControlRuntimeSecurityInvalid)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
        {
            Ok(mut file) => {
                file.write_all(CONTROL_ACL_MARKER_CONTENTS)
                    .and_then(|()| file.sync_all())
                    .map_err(|_| DaemonPathError::ControlRuntimeSecurityInvalid)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if !acl_marker_valid(&marker) {
                    return Err(DaemonPathError::ControlRuntimeSecurityInvalid);
                }
            }
            Err(_) => return Err(DaemonPathError::ControlRuntimeSecurityInvalid),
        }
        vibemux_platform::verify_restricted_path_acl(&marker)
            .map_err(|_| DaemonPathError::ControlRuntimeSecurityInvalid)?;
        Ok(())
    }

    #[cfg(windows)]
    fn validate_windows_acl_marker(&self) -> Result<(), DaemonPathError> {
        let marker = self.control_runtime_root.join(CONTROL_ACL_MARKER_FILE_NAME);
        if acl_marker_valid(&marker) {
            Ok(())
        } else {
            Err(DaemonPathError::ControlRuntimeSecurityInvalid)
        }
    }
}

pub(crate) fn project_runtime_key(project_root: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"vibemux-project-runtime-v1\0");
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        for unit in project_root.as_os_str().encode_wide() {
            hasher.update(unit.to_le_bytes());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hasher.update(project_root.as_os_str().as_bytes());
    }
    let digest: [u8; 32] = hasher.finalize().into();
    hex(&digest)
}

#[cfg(windows)]
fn acl_marker_valid(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| {
            metadata.file_type().is_file() && !metadata.file_type().is_symlink()
        })
        && std::fs::read(path).ok().as_deref() == Some(CONTROL_ACL_MARKER_CONTENTS)
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DaemonPathError {
    #[error("daemon project root is invalid")]
    InvalidProjectRoot,
    #[error("daemon runtime directory is unavailable")]
    RuntimeDirectoryUnavailable,
    #[error("daemon control runtime directory is unavailable")]
    ControlRuntimeUnavailable,
    #[error("daemon runtime directory resolves outside the project boundary")]
    UnsafeRuntimeDirectory,
    #[error("daemon control runtime path is unsafe")]
    UnsafeControlRuntime,
    #[error("daemon control runtime access policy is invalid")]
    ControlRuntimeSecurityInvalid,
    #[error("daemon Rust database path is unsafe")]
    UnsafeDatabasePath,
    #[error("daemon Rust and Python database paths refer to the same file")]
    DatabaseCollision,
    #[error("legacy daemon runtime artifacts remain unresolved")]
    LegacyRuntimeConflict,
}

impl DaemonPathError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidProjectRoot => "daemon_invalid_project_root",
            Self::RuntimeDirectoryUnavailable => "daemon_runtime_directory_unavailable",
            Self::ControlRuntimeUnavailable => "daemon_control_runtime_unavailable",
            Self::UnsafeRuntimeDirectory => "daemon_unsafe_runtime_directory",
            Self::UnsafeControlRuntime => "daemon_unsafe_control_runtime",
            Self::ControlRuntimeSecurityInvalid => "daemon_control_runtime_security_invalid",
            Self::UnsafeDatabasePath => "daemon_unsafe_database_path",
            Self::DatabaseCollision => "daemon_database_collision",
            Self::LegacyRuntimeConflict => "daemon_legacy_runtime_conflict",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    struct ControlRuntimeCleanup(PathBuf);

    #[cfg(windows)]
    impl ControlRuntimeCleanup {
        fn new(paths: &DaemonPaths) -> Self {
            Self(paths.runtime_dir().to_path_buf())
        }
    }

    #[cfg(windows)]
    impl Drop for ControlRuntimeCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rust_and_python_database_paths_are_distinct() {
        let temp = tempfile::tempdir().expect("temp directory");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        assert_ne!(paths.database_path(), paths.python_database_path());
        assert_eq!(
            paths
                .database_path()
                .file_name()
                .and_then(|name| name.to_str()),
            Some(RUST_DATABASE_FILE_NAME)
        );
        assert_eq!(
            paths
                .python_database_path()
                .file_name()
                .and_then(|name| name.to_str()),
            Some(PYTHON_DATABASE_FILE_NAME)
        );
    }

    #[test]
    fn probe_cache_path_is_pinned_under_the_state_dir() {
        let temp = tempfile::tempdir().expect("temp directory");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        assert_eq!(
            paths.probe_cache_path(),
            paths.state_dir().join("probe_cache.json")
        );
        assert_eq!(
            paths
                .probe_cache_path()
                .file_name()
                .and_then(|name| name.to_str()),
            Some(vibemux_probe::cache::PROBE_CACHE_FILE_NAME)
        );
    }

    #[test]
    fn invalid_project_root_fails_without_creating_runtime_state() {
        let temp = tempfile::tempdir().expect("temp directory");
        let missing = temp.path().join("missing");
        assert_eq!(
            DaemonPaths::from_project_root(&missing).expect_err("missing root must fail"),
            DaemonPathError::InvalidProjectRoot
        );
        assert!(!missing.exists());
    }

    #[test]
    fn hard_link_to_python_database_is_rejected() {
        let temp = tempfile::tempdir().expect("temp directory");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        #[cfg(windows)]
        let _control_cleanup = ControlRuntimeCleanup::new(&paths);
        paths.ensure_runtime_dir().expect("runtime directory");
        std::fs::write(paths.python_database_path(), b"python").expect("Python database");
        std::fs::hard_link(paths.python_database_path(), paths.database_path())
            .expect("database hard link");
        assert_eq!(
            paths
                .ensure_runtime_dir()
                .expect_err("database alias must fail"),
            DaemonPathError::DatabaseCollision
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_control_runtime_is_hashed_and_acl_restricted() {
        let temp = tempfile::tempdir().expect("temp directory");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        let _control_cleanup = ControlRuntimeCleanup::new(&paths);
        assert!(!paths.runtime_dir().starts_with(paths.project_root()));
        assert_eq!(paths.runtime_key().len(), 64);
        assert!(
            paths
                .runtime_key()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert_eq!(
            paths
                .runtime_dir()
                .file_name()
                .and_then(|name| name.to_str()),
            Some(paths.runtime_key())
        );
        assert_eq!(
            paths
                .writer_lock_path()
                .file_name()
                .and_then(|name| name.to_str()),
            Some(CONTROL_WRITER_LOCK_FILE_NAME)
        );
        assert!(
            paths
                .legacy_descriptor_path()
                .starts_with(paths.state_dir())
        );
        assert!(
            paths
                .legacy_writer_lock_path()
                .starts_with(paths.state_dir())
        );
        paths
            .ensure_runtime_dir()
            .expect("secure runtime directory");
        paths
            .verify_control_runtime_acl()
            .expect("verify control runtime ACL");
        std::fs::write(paths.legacy_descriptor_path(), b"legacy").expect("legacy descriptor");
        assert_eq!(
            paths
                .validate_daemon_start()
                .expect_err("legacy descriptor must block new daemon"),
            DaemonPathError::LegacyRuntimeConflict
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_runtime_directory_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temp directory");
        let outside = tempfile::tempdir().expect("outside directory");
        symlink(outside.path(), temp.path().join(RUNTIME_DIR_NAME)).expect("runtime symlink");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        assert_eq!(
            paths
                .ensure_runtime_dir()
                .expect_err("runtime symlink must fail"),
            DaemonPathError::UnsafeRuntimeDirectory
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_rust_database_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temp directory");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        paths.ensure_runtime_dir().expect("runtime directory");
        std::fs::write(paths.python_database_path(), b"python").expect("Python database");
        symlink(paths.python_database_path(), paths.database_path()).expect("database symlink");
        assert_eq!(
            paths
                .ensure_runtime_dir()
                .expect_err("database symlink must fail"),
            DaemonPathError::UnsafeDatabasePath
        );
    }
}
