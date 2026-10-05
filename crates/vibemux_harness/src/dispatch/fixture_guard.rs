//! Test-only admission for exact copies of the built synthetic worker.
//!
//! A fixture label or route path is never authority. Registration pins the
//! bytes of an already built source image and its direct children under one
//! canonical vendor directory. Every launch rechecks paths, reparse points,
//! and bytes. This is a test fixture guard, not an OS sandbox or live tool
//! permission profile.

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

use sha2::{Digest, Sha256};

use super::{
    DispatchError, DispatchRoute, NativeProtocol,
    launch_spec::LaunchSpec,
    writable_profile::{WritableGrant, writable_environment_names},
};
use crate::AgentKind;

const MAX_FIXTURE_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
const FILE_CHUNK_BYTES: usize = 64 * 1024;

pub const CODEX_FIXTURE_WRITABLE_PROFILE: &str = "codex_fixture_writable_v1";
pub const CLAUDE_FIXTURE_WRITABLE_PROFILE: &str = "claude_fixture_writable_v1";

/// An opaque, process-local registration of known copied worker images.
/// The fields are private and it cannot be created from serialized labels.
pub struct FixtureLaunchGuard {
    source: PathBuf,
    vendor_root: PathBuf,
    source_digest: [u8; 32],
    copies: BTreeMap<AgentKind, PathBuf>,
    development_runtime: Option<(PathBuf, [u8; 32])>,
}

impl fmt::Debug for FixtureLaunchGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FixtureLaunchGuard")
            .field("registered_aliases", &self.copies.len())
            .finish_non_exhaustive()
    }
}

impl FixtureLaunchGuard {
    /// The caller copies the built image first. This function only verifies
    /// existing direct children; it never creates or replaces an executable.
    pub fn register_worker_copies(
        built_source: &Path,
        vendor_root: &Path,
        aliases: &[AgentKind],
    ) -> Result<Self, DispatchError> {
        let invalid = DispatchError::ConfigExecutableInvalid;
        if aliases.is_empty() || aliases.len() > 2 {
            return Err(invalid);
        }
        let source = regular_canonical(built_source)?;
        let vendor_root = directory_canonical(vendor_root)?;
        if source.starts_with(&vendor_root) {
            return Err(invalid);
        }
        let source_digest = image_digest(&source)?;
        let mut copies = BTreeMap::new();
        for &alias in aliases {
            fixture_profile_id(alias)?;
            if copies.contains_key(&alias) {
                return Err(invalid);
            }
            let expected = vendor_root.join(format!(
                "{}{}",
                alias.command_name(),
                std::env::consts::EXE_SUFFIX
            ));
            let copy = regular_canonical(&expected)?;
            if copy.parent() != Some(vendor_root.as_path()) || image_digest(&copy)? != source_digest
            {
                return Err(invalid);
            }
            copies.insert(alias, copy);
        }
        Ok(Self {
            source,
            vendor_root,
            source_digest,
            copies,
            development_runtime: None,
        })
    }

    /// A harness may pin its already-selected development-test runtime.
    pub fn with_development_runtime(mut self, executable: &Path) -> Result<Self, DispatchError> {
        let executable = regular_canonical(executable)?;
        self.development_runtime = Some((executable.clone(), image_digest(&executable)?));
        Ok(self)
    }

    fn validate_route(&self, route: &DispatchRoute) -> Result<&'static str, DispatchError> {
        let invalid = DispatchError::ConfigExecutableInvalid;
        let profile = fixture_profile_id(route.harness)?;
        if !route.enabled || !route.allow_execution || !route.protocol.accepts(route.harness) {
            return Err(DispatchError::ExecutionDisabled);
        }
        let expected = self.copies.get(&route.harness).ok_or(invalid)?;
        if directory_canonical(&self.vendor_root)? != self.vendor_root
            || regular_canonical(&self.source)? != self.source
            || image_digest(&self.source)? != self.source_digest
        {
            return Err(invalid);
        }
        let actual = regular_canonical(&route.executable)?;
        if &actual != expected
            || actual.parent() != Some(self.vendor_root.as_path())
            || image_digest(&actual)? != self.source_digest
        {
            return Err(invalid);
        }
        Ok(profile)
    }
}

pub fn fixture_profile_id(alias: AgentKind) -> Result<&'static str, DispatchError> {
    match alias {
        AgentKind::Codex => Ok(CODEX_FIXTURE_WRITABLE_PROFILE),
        AgentKind::Claude => Ok(CLAUDE_FIXTURE_WRITABLE_PROFILE),
        _ => Err(DispatchError::ExecutionDisabled),
    }
}

/// Only a registered exact worker copy may use the synthetic fixture CLI.
/// The grant is serialized as strict JSON; the synthetic binary must enforce
/// it on every requested effect, rather than trusting a fixture label.
pub fn build_writable_fixture_launch_spec(
    route: &DispatchRoute,
    grant: WritableGrant,
    guard: &FixtureLaunchGuard,
) -> Result<LaunchSpec, DispatchError> {
    let profile = guard.validate_route(route)?;
    let grant_json = serde_json::to_string(&grant).map_err(|_| DispatchError::ConfigInvalid)?;
    let mut arguments = match route.protocol {
        NativeProtocol::CodexExec => vec!["exec".to_string(), "--json".to_string()],
        NativeProtocol::ClaudeStreamJson => vec![
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
        ],
        _ => return Err(DispatchError::ExecutionDisabled),
    };
    arguments.extend([
        "--fixture_profile".to_string(),
        profile.to_string(),
        "--fixture_grant_json".to_string(),
        grant_json,
    ]);
    if let Some((runtime, digest)) = &guard.development_runtime {
        if regular_canonical(runtime)? != *runtime || image_digest(runtime)? != *digest {
            return Err(DispatchError::ConfigExecutableInvalid);
        }
        arguments.extend([
            "--fixture_node".to_string(),
            runtime.to_string_lossy().into_owned(),
        ]);
    }
    if route.protocol == NativeProtocol::CodexExec {
        // The prompt stays on stdin, never in argv.
        arguments.push("-".to_string());
    }
    Ok(LaunchSpec {
        executable: route.executable.clone(),
        arguments,
        environment_names: writable_environment_names(route),
    })
}

fn directory_canonical(path: &Path) -> Result<PathBuf, DispatchError> {
    let invalid = DispatchError::ConfigExecutableInvalid;
    reject_reparse_components(path)?;
    let canonical = fs::canonicalize(path).map_err(|_| invalid)?;
    if !canonical.is_dir() {
        return Err(invalid);
    }
    Ok(canonical)
}

fn regular_canonical(path: &Path) -> Result<PathBuf, DispatchError> {
    let invalid = DispatchError::ConfigExecutableInvalid;
    reject_reparse_components(path)?;
    let canonical = fs::canonicalize(path).map_err(|_| invalid)?;
    if !canonical.is_file() {
        return Err(invalid);
    }
    Ok(canonical)
}

fn reject_reparse_components(path: &Path) -> Result<(), DispatchError> {
    let invalid = DispatchError::ConfigExecutableInvalid;
    if !path.is_absolute() {
        return Err(invalid);
    }
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| invalid)?;
        if metadata.file_type().is_symlink() {
            return Err(invalid);
        }
        #[cfg(windows)]
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid);
        }
    }
    Ok(())
}

fn image_digest(path: &Path) -> Result<[u8; 32], DispatchError> {
    let invalid = DispatchError::ConfigExecutableInvalid;
    let mut file = File::open(path).map_err(|_| invalid)?;
    let metadata = file.metadata().map_err(|_| invalid)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_FIXTURE_IMAGE_BYTES {
        return Err(invalid);
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; FILE_CHUNK_BYTES];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|_| invalid)?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or(invalid)?;
        if total > MAX_FIXTURE_IMAGE_BYTES {
            return Err(invalid);
        }
        hasher.update(&buffer[..read]);
    }
    if total != metadata.len() {
        return Err(invalid);
    }
    Ok(hasher.finalize().into())
}
