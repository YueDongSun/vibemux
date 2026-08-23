//! Canonical project paths for the standalone Rust daemon process.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::{control::DESCRIPTOR_FILE_NAME, writer_lock_path_for_database};

pub const RUNTIME_DIR_NAME: &str = ".vibemux";
pub const RUST_DATABASE_FILE_NAME: &str = "vibemux_rust.sqlite3";
pub const PYTHON_DATABASE_FILE_NAME: &str = "vibemux.sqlite3";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonPaths {
    project_root: PathBuf,
    runtime_dir: PathBuf,
    database_path: PathBuf,
    descriptor_path: PathBuf,
    writer_lock_path: PathBuf,
}

impl DaemonPaths {
    pub fn from_project_root(project_root: &Path) -> Result<Self, DaemonPathError> {
        let project_root =
            std::fs::canonicalize(project_root).map_err(|_| DaemonPathError::InvalidProjectRoot)?;
        if !project_root.is_dir() {
            return Err(DaemonPathError::InvalidProjectRoot);
        }
        let runtime_dir = project_root.join(RUNTIME_DIR_NAME);
        let database_path = runtime_dir.join(RUST_DATABASE_FILE_NAME);
        let descriptor_path = runtime_dir.join(DESCRIPTOR_FILE_NAME);
        let writer_lock_path = writer_lock_path_for_database(&database_path);
        Ok(Self {
            project_root,
            runtime_dir,
            database_path,
            descriptor_path,
            writer_lock_path,
        })
    }

    pub fn ensure_runtime_dir(&self) -> Result<(), DaemonPathError> {
        std::fs::create_dir_all(&self.runtime_dir)
            .map_err(|_| DaemonPathError::RuntimeDirectoryUnavailable)?;
        self.validate_runtime_dir()
    }

    pub fn validate_runtime_dir(&self) -> Result<(), DaemonPathError> {
        let canonical_runtime = std::fs::canonicalize(&self.runtime_dir)
            .map_err(|_| DaemonPathError::RuntimeDirectoryUnavailable)?;
        if canonical_runtime != self.runtime_dir {
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

    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    #[must_use]
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
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
    pub fn python_database_path(&self) -> PathBuf {
        self.runtime_dir.join(PYTHON_DATABASE_FILE_NAME)
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DaemonPathError {
    #[error("daemon project root is invalid")]
    InvalidProjectRoot,
    #[error("daemon runtime directory is unavailable")]
    RuntimeDirectoryUnavailable,
    #[error("daemon runtime directory resolves outside the project boundary")]
    UnsafeRuntimeDirectory,
    #[error("daemon Rust database path is unsafe")]
    UnsafeDatabasePath,
    #[error("daemon Rust and Python database paths refer to the same file")]
    DatabaseCollision,
}

impl DaemonPathError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidProjectRoot => "daemon_invalid_project_root",
            Self::RuntimeDirectoryUnavailable => "daemon_runtime_directory_unavailable",
            Self::UnsafeRuntimeDirectory => "daemon_unsafe_runtime_directory",
            Self::UnsafeDatabasePath => "daemon_unsafe_database_path",
            Self::DatabaseCollision => "daemon_database_collision",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
