//! Owner of the trusted probe cache: the file name, its default location, and
//! atomic write / hardened read helpers.
//!
//! The daemon reads this cache (never re-probing in-process) to learn which
//! harness launchers were verified. Because the file gates harness state
//! writes, [`read_cache`] treats it as untrusted-on-disk input: it refuses
//! non-regular files (symlinks, directories, FIFOs), oversized files, files
//! with group/other permission bits on unix, and malformed JSON. [`write_cache`]
//! writes atomically (temp file in the same directory, then rename) so a
//! partial or torn cache is never observable by a concurrent reader.
//!
//! Error messages never contain secrets or database paths; they carry at most
//! the path the caller supplied via its own context.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

use crate::ProbeReport;

/// The cache file name. This module is the single owner of the name; the
/// daemon's `DaemonPaths::probe_cache_path` and the probe binary both derive
/// their paths from it.
pub const PROBE_CACHE_FILE_NAME: &str = "probe_cache.json";

/// The runtime directory (under the project root) that holds the cache.
const RUNTIME_DIR_NAME: &str = ".vibemux";

/// Hard upper bound on cache size. A probe report for ten agents is a few KB;
/// 256 KiB leaves generous headroom while refusing a runaway/hostile file.
pub const MAX_CACHE_BYTES: u64 = 256 * 1024;

/// Failures surfaced by [`read_cache`] and [`write_cache`]. None of the
/// messages embed a path or secret beyond what the caller already holds.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CacheError {
    #[error("probe cache is missing")]
    Missing,
    #[error("probe cache is not a safe owner-only regular file")]
    UnsafeArtifact,
    #[error("probe cache exceeds the bounded size")]
    TooLarge,
    #[error("probe cache is not valid report JSON")]
    Invalid,
    #[error("probe cache I/O failed")]
    Io,
}

/// The default cache location for a project: `<project_root>/.vibemux/probe_cache.json`.
#[must_use]
pub fn default_cache_path(project_root: &Path) -> PathBuf {
    project_root
        .join(RUNTIME_DIR_NAME)
        .join(PROBE_CACHE_FILE_NAME)
}

/// Atomically write `report` to `path`.
///
/// Creates the parent directory if missing (owner-only `0700` on unix), writes
/// a sibling temp file (`0600` on unix), syncs it, then renames over the
/// target. A reader therefore always observes either the previous complete
/// cache or the new complete cache, never a partial write. On any failure the
/// temp file is removed and the previous cache is left untouched.
pub fn write_cache(report: &ProbeReport, path: &Path) -> Result<(), CacheError> {
    let bytes = serde_json::to_vec(report).map_err(|_| CacheError::Io)?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    create_private_dir(&parent)?;
    let temp = temp_path(&parent);
    if let Err(error) = write_temp(&temp, &bytes) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if fs::rename(&temp, path).is_err() {
        let _ = fs::remove_file(&temp);
        return Err(CacheError::Io);
    }
    Ok(())
}

/// Read and validate the cache at `path`.
///
/// Validation order matters: a missing file is [`CacheError::Missing`]; a
/// non-regular file or (on unix) a file with group/other permission bits is
/// [`CacheError::UnsafeArtifact`]; an oversized file is [`CacheError::TooLarge`];
/// malformed JSON is [`CacheError::Invalid`]. The size is re-checked after a
/// bounded read so a file that grows between the metadata check and the read
/// cannot slip past the bound.
pub fn read_cache(path: &Path) -> Result<ProbeReport, CacheError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CacheError::Missing);
        }
        Err(_) => return Err(CacheError::Io),
    };
    if !metadata.file_type().is_file() {
        return Err(CacheError::UnsafeArtifact);
    }
    if metadata.len() > MAX_CACHE_BYTES {
        return Err(CacheError::TooLarge);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(CacheError::UnsafeArtifact);
        }
    }
    let file = fs::File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CacheError::Missing
        } else {
            CacheError::Io
        }
    })?;
    // Re-stat the opened fd: the path could have been swapped for a symlink or
    // a non-regular file between the metadata check and the open.
    let file_metadata = file.metadata().map_err(|_| CacheError::Io)?;
    if !file_metadata.is_file() {
        return Err(CacheError::UnsafeArtifact);
    }
    let mut bytes = Vec::new();
    file.take(MAX_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CacheError::Io)?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(CacheError::TooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| CacheError::Invalid)
}

fn create_private_dir(parent: &Path) -> Result<(), CacheError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|_| CacheError::Io)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(parent).map_err(|_| CacheError::Io)
    }
}

fn temp_path(dir: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    dir.join(format!(
        ".{PROBE_CACHE_FILE_NAME}.{}.{}.tmp",
        std::process::id(),
        nanos
    ))
}

fn write_temp(temp: &Path, bytes: &[u8]) -> Result<(), CacheError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(temp).map_err(|_| CacheError::Io)?;
    file.write_all(bytes).map_err(|_| CacheError::Io)?;
    file.sync_all().map_err(|_| CacheError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{A2aSelfTestProbe, AgentProbe, GatewayProbe, LauncherKind, ProbeState, RouteKind};

    fn sample_report() -> ProbeReport {
        ProbeReport {
            schema_version: crate::PROBE_SCHEMA_VERSION,
            observed_at_epoch_seconds: 1,
            platform: "test".to_string(),
            agents: vec![AgentProbe {
                agent: crate::AgentKind::Claude,
                launcher_state: ProbeState::Verified,
                authentication_state: ProbeState::NotRun,
                inference_state: ProbeState::NotRun,
                launcher: LauncherKind::DirectExecutable,
                path: Some("C:\\tools\\claude.exe".to_string()),
                version: Some("1.0.0".to_string()),
                route: RouteKind::Unknown,
                endpoints: Vec::new(),
                code: "version_verified".to_string(),
            }],
            gateway: GatewayProbe {
                state: ProbeState::Unavailable,
                host: "127.0.0.1".to_string(),
                port: crate::DEFAULT_CC_SWITCH_PORT,
                tcp_reachable: false,
                health_status: None,
                telemetry_state: ProbeState::Unavailable,
                telemetry: Vec::new(),
                code: "gateway_unavailable".to_string(),
            },
            a2a: A2aSelfTestProbe {
                state: ProbeState::NotRun,
                correlation_preserved: false,
                listener_closed: false,
                code: "a2a_not_run".to_string(),
            },
        }
    }

    /// Write raw bytes with owner-only perms on unix so the read validation
    /// reaches the intended check instead of tripping the permission gate.
    fn write_raw(path: &Path, bytes: &[u8]) {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path).expect("open raw");
        file.write_all(bytes).expect("write raw");
        file.sync_all().expect("sync raw");
    }

    #[test]
    fn default_path_is_runtime_dir_over_cache_name() {
        let path = default_cache_path(Path::new("/tmp/project"));
        assert_eq!(
            path,
            Path::new("/tmp/project")
                .join(".vibemux")
                .join("probe_cache.json")
        );
        assert_eq!(PROBE_CACHE_FILE_NAME, "probe_cache.json");
    }

    #[test]
    fn write_then_read_round_trips_report() {
        let temp = tempfile::tempdir().expect("temp");
        let path = default_cache_path(temp.path());
        let report = sample_report();
        write_cache(&report, &path).expect("write cache");
        let decoded = read_cache(&path).expect("read cache");
        assert_eq!(decoded, report);
        assert_eq!(
            decoded.agents[0].path.as_deref(),
            Some("C:\\tools\\claude.exe")
        );
    }

    #[test]
    fn write_creates_missing_runtime_dir_and_leaves_only_final_file() {
        let temp = tempfile::tempdir().expect("temp");
        let path = default_cache_path(temp.path());
        assert!(!path.parent().expect("parent").exists());
        write_cache(&sample_report(), &path).expect("write cache");
        assert!(path.is_file());
        // Atomicity basics: the temp+rename leaves exactly the final file, no
        // stray sibling temp artifacts.
        let entries: Vec<_> = fs::read_dir(path.parent().expect("parent"))
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], std::ffi::OsStr::new(PROBE_CACHE_FILE_NAME));
    }

    #[test]
    fn rewrite_replaces_previous_cache_atomically() {
        let temp = tempfile::tempdir().expect("temp");
        let path = default_cache_path(temp.path());
        write_cache(&sample_report(), &path).expect("first write");
        let mut second = sample_report();
        second.observed_at_epoch_seconds = 42;
        write_cache(&second, &path).expect("second write");
        assert_eq!(
            read_cache(&path).expect("read").observed_at_epoch_seconds,
            42
        );
        let entries = fs::read_dir(path.parent().expect("parent"))
            .expect("read dir")
            .count();
        assert_eq!(entries, 1, "no temp file left behind");
    }

    #[test]
    fn read_missing_file_is_missing() {
        let temp = tempfile::tempdir().expect("temp");
        let path = default_cache_path(temp.path());
        assert_eq!(read_cache(&path), Err(CacheError::Missing));
    }

    #[test]
    fn read_directory_is_unsafe_artifact() {
        let temp = tempfile::tempdir().expect("temp");
        // A directory at the cache path is not a regular file.
        let dir = default_cache_path(temp.path());
        fs::create_dir_all(&dir).expect("create dir at cache path");
        assert_eq!(read_cache(&dir), Err(CacheError::UnsafeArtifact));
    }

    #[test]
    fn read_oversized_file_is_too_large() {
        let temp = tempfile::tempdir().expect("temp");
        let path = default_cache_path(temp.path());
        fs::create_dir_all(path.parent().expect("parent")).expect("runtime dir");
        write_raw(&path, &vec![b'x'; (MAX_CACHE_BYTES + 1) as usize]);
        assert_eq!(read_cache(&path), Err(CacheError::TooLarge));
    }

    #[test]
    fn read_malformed_json_is_invalid() {
        let temp = tempfile::tempdir().expect("temp");
        let path = default_cache_path(temp.path());
        fs::create_dir_all(path.parent().expect("parent")).expect("runtime dir");
        write_raw(&path, b"not json at all");
        assert_eq!(read_cache(&path), Err(CacheError::Invalid));
    }

    #[test]
    fn read_valid_json_wrong_shape_is_invalid() {
        let temp = tempfile::tempdir().expect("temp");
        let path = default_cache_path(temp.path());
        fs::create_dir_all(path.parent().expect("parent")).expect("runtime dir");
        write_raw(&path, br#"{"unexpected":"shape"}"#);
        assert_eq!(read_cache(&path), Err(CacheError::Invalid));
    }

    #[test]
    fn legacy_cache_without_path_field_parses_as_none() {
        let temp = tempfile::tempdir().expect("temp");
        let path = default_cache_path(temp.path());
        fs::create_dir_all(path.parent().expect("parent")).expect("runtime dir");
        // A cache written by an older build has no `path` field on the agent.
        write_raw(
            &path,
            br#"{
                "schema_version": 1,
                "observed_at_epoch_seconds": 1,
                "platform": "test",
                "agents": [{
                    "agent": "claude",
                    "launcher_state": "verified",
                    "authentication_state": "not_run",
                    "inference_state": "not_run",
                    "launcher": "direct_executable",
                    "version": "1.0.0",
                    "route": "unknown",
                    "endpoints": [],
                    "code": "version_verified"
                }],
                "gateway": {"state":"unavailable","host":"127.0.0.1","port":15721,"tcp_reachable":false,"health_status":null,"telemetry_state":"unavailable","telemetry":[],"code":"gateway_unavailable"},
                "a2a": {"state":"not_run","correlation_preserved":false,"listener_closed":false,"code":"a2a_not_run"}
            }"#,
        );
        let decoded = read_cache(&path).expect("legacy cache parses");
        assert_eq!(decoded.agents[0].path, None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_write_sets_owner_only_perms() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let path = default_cache_path(temp.path());
        write_cache(&sample_report(), &path).expect("write cache");
        let dir_mode = fs::metadata(path.parent().expect("parent"))
            .expect("dir meta")
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        let file_mode = fs::metadata(&path).expect("file meta").permissions().mode();
        assert_eq!(file_mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn unix_read_rejects_group_readable_cache() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let path = default_cache_path(temp.path());
        write_cache(&sample_report(), &path).expect("write cache");
        // Loosen perms to owner+group read; the cache must now be refused.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("chmod");
        assert_eq!(read_cache(&path), Err(CacheError::UnsafeArtifact));
    }

    #[cfg(unix)]
    #[test]
    fn unix_read_rejects_symlink() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("temp");
        let real = default_cache_path(temp.path());
        write_cache(&sample_report(), &real).expect("write cache");
        let link = temp.path().join("link.json");
        symlink(&real, &link).expect("symlink");
        assert_eq!(read_cache(&link), Err(CacheError::UnsafeArtifact));
    }
}
