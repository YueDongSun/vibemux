//! Loads and pins the operator route config (ADR 029 §4). The daemon reads
//! `<state dir>/harness_dispatch.json` once at startup; a changed file takes
//! effect only after a restart, so every attempt of one daemon runs under
//! one config digest.
//!
//! Beyond the pure shape rules in [`DispatchConfig::parse`], each enabled
//! route's executable must be trusted on this host: its canonical target
//! is a regular file outside the project root (so a repository cannot ship
//! the binary a route runs), and the target's file stem is the harness
//! command name (so a route cannot name an interpreter such as `cmd.exe`
//! or `powershell.exe`). Failures carry fixed codes only.

use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use vibemux_harness::{
    AgentKind,
    dispatch::{
        DispatchConfig, DispatchError, DispatchRoute, Sha256Digest,
        route_config::{
            ExecutablePolicy, MAX_ROUTE_CONFIG_BYTES, PinnedDispatchConfig,
            executable_names_harness, validate_executable,
        },
    },
};

pub const DISPATCH_CONFIG_FILE_NAME: &str = "harness_dispatch.json";

/// A validated config and the canonical executable of each enabled route.
pub struct LoadedDispatchConfig {
    pinned: PinnedDispatchConfig,
    executables: BTreeMap<AgentKind, PathBuf>,
}

impl fmt::Debug for LoadedDispatchConfig {
    /// Redacted: executable paths never reach a log.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoadedDispatchConfig")
            .field("digest", &self.pinned.digest)
            .field("enabled_routes", &self.executables.len())
            .finish_non_exhaustive()
    }
}

impl LoadedDispatchConfig {
    #[must_use]
    pub fn config(&self) -> &DispatchConfig {
        &self.pinned.config
    }

    #[must_use]
    pub fn digest(&self) -> Sha256Digest {
        self.pinned.digest
    }

    /// The canonical executable the loader verified for `harness`.
    pub fn executable(&self, harness: AgentKind) -> Result<&Path, DispatchError> {
        self.executables
            .get(&harness)
            .map(PathBuf::as_path)
            .ok_or(DispatchError::ExecutableUnavailable)
    }
}

/// Reads, parses, and verifies the config at `config_path`. A missing file
/// means dispatch is not configured; a symlink or other non-regular file is
/// refused rather than followed.
pub fn load_dispatch_config(
    config_path: &Path,
    canonical_root: &Path,
) -> Result<LoadedDispatchConfig, DispatchError> {
    let bytes = read_config(config_path)?;
    let policy = ExecutablePolicy::for_host();
    let pinned = DispatchConfig::parse(&bytes, policy)?;
    let mut executables = BTreeMap::new();
    for route in pinned.config.routes.iter().filter(|route| route.enabled) {
        executables.insert(
            route.harness,
            trusted_executable(route, canonical_root, policy)?,
        );
    }
    Ok(LoadedDispatchConfig {
        pinned,
        executables,
    })
}

fn read_config(config_path: &Path) -> Result<Vec<u8>, DispatchError> {
    let metadata = match std::fs::symlink_metadata(config_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(DispatchError::Unconfigured);
        }
        Err(_) => return Err(DispatchError::ConfigInvalid),
    };
    if !metadata.is_file() {
        return Err(DispatchError::ConfigInvalid);
    }
    let limit = MAX_ROUTE_CONFIG_BYTES as u64;
    if metadata.len() > limit {
        return Err(DispatchError::ConfigTooLarge);
    }
    let file = File::open(config_path).map_err(|_| DispatchError::ConfigInvalid)?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| DispatchError::ConfigInvalid)?;
    if bytes.len() as u64 > limit {
        return Err(DispatchError::ConfigTooLarge);
    }
    Ok(bytes)
}

fn trusted_executable(
    route: &DispatchRoute,
    canonical_root: &Path,
    policy: ExecutablePolicy,
) -> Result<PathBuf, DispatchError> {
    let invalid = DispatchError::ConfigExecutableInvalid;
    let canonical = std::fs::canonicalize(&route.executable).map_err(|_| invalid)?;
    let regular = std::fs::metadata(&canonical)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false);
    // The configured path passed the shape rules; the target a link resolves
    // to must pass them too.
    if !regular
        || canonical.starts_with(canonical_root)
        || validate_executable(&canonical, policy).is_err()
        || !executable_names_harness(&canonical, route.harness, policy)
    {
        return Err(invalid);
    }
    Ok(canonical)
}
