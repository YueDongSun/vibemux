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
//! or `powershell.exe`). DSH's explicit Node launcher pair instead requires
//! a canonical external entry script owned by the DSH package (ADR 032).
//! Failures carry fixed codes only.

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
            executable_is_node_image, executable_names_harness, validate_entry_script_shape,
            validate_executable,
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
    let mut pinned = DispatchConfig::parse(&bytes, policy)?;
    let mut executables = BTreeMap::new();
    for route in pinned
        .config
        .routes
        .iter_mut()
        .filter(|route| route.enabled)
    {
        executables.insert(
            route.harness,
            trusted_executable(route, canonical_root, policy)?,
        );
        if route.protocol.requires_launcher_pair() {
            route.entry_script = Some(trusted_entry_script(route, canonical_root)?);
        }
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
        || !(if route.protocol.requires_launcher_pair() {
            executable_is_node_image(&canonical, policy)
        } else {
            executable_names_harness(&canonical, route.harness, policy)
        })
    {
        return Err(invalid);
    }
    Ok(canonical)
}

/// Pins the same bounded package ownership rule used by the session driver.
fn trusted_entry_script(
    route: &DispatchRoute,
    canonical_root: &Path,
) -> Result<PathBuf, DispatchError> {
    let invalid = DispatchError::ConfigExecutableInvalid;
    let configured = route.entry_script.as_ref().ok_or(invalid)?;
    let canonical = std::fs::canonicalize(configured).map_err(|_| invalid)?;
    if !canonical.is_file()
        || canonical.starts_with(canonical_root)
        || validate_entry_script_shape(&canonical).is_err()
        || !vibemux_dsh::config::dsh_package_owns(&canonical)
    {
        return Err(invalid);
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vibemux_harness::dispatch::route_config::ROUTE_CONFIG_SCHEMA_VERSION;

    #[test]
    fn dsh_launcher_pair_is_pinned_and_rejects_unowned_or_project_files() {
        let temp = tempfile::tempdir().expect("fixture");
        let project = temp.path().join("project");
        let package = temp.path().join("vendor/node_modules/@deepseek-ai/dsh");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(package.join("lib")).expect("package");
        let node = temp
            .path()
            .join(format!("node{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&node, b"synthetic node image").expect("image");
        let script = package.join("lib/bin.js");
        std::fs::write(&script, "// fixture\n").expect("script");
        let manifest = package.join("package.json");
        std::fs::write(
            &manifest,
            json!({"name":"@deepseek-ai/dsh",
            "bin":{"dsh":"lib/bin.js"}})
            .to_string(),
        )
        .expect("manifest");
        let config_path = project.join(DISPATCH_CONFIG_FILE_NAME);
        let mut config = json!({"schema_version": ROUTE_CONFIG_SCHEMA_VERSION,
            "routes": [{"harness":"dsh", "protocol":"dsh_sdk_json_rpc",
                "executable":node,"entry_script":script,"provider":"fixture",
                "model":"fixture","enabled":true,"allow_execution":true}]});
        std::fs::write(&config_path, config.to_string()).expect("config");
        let root = std::fs::canonicalize(&project).expect("canonical project");
        let loaded = load_dispatch_config(&config_path, &root).expect("trusted pair");
        assert_eq!(
            loaded.config().routes[0].entry_script,
            Some(std::fs::canonicalize(&script).expect("canonical script"))
        );
        std::fs::write(
            &manifest,
            json!({"name":"unrelated_package",
            "bin":{"dsh":"lib/bin.js"}})
            .to_string(),
        )
        .expect("unowned manifest");
        assert!(matches!(
            load_dispatch_config(&config_path, &root),
            Err(DispatchError::ConfigExecutableInvalid)
        ));
        std::fs::write(
            &manifest,
            json!({"name":"@deepseek-ai/dsh", "bin":{"dsh":"lib/bin.js"}}).to_string(),
        )
        .expect("restore owned manifest");
        let local_node = project.join(format!("node{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&local_node, b"repository image").expect("local image");
        config["routes"][0]["executable"] = json!(local_node);
        std::fs::write(&config_path, config.to_string()).expect("local config");
        assert!(matches!(
            load_dispatch_config(&config_path, &root),
            Err(DispatchError::ConfigExecutableInvalid)
        ));
        let local_package = project.join("node_modules/@deepseek-ai/dsh");
        std::fs::create_dir_all(local_package.join("lib")).expect("local package");
        let local_script = local_package.join("lib/bin.js");
        std::fs::write(&local_script, "// repository script\n").expect("local script");
        std::fs::write(
            local_package.join("package.json"),
            json!({"name":"@deepseek-ai/dsh", "bin":{"dsh":"lib/bin.js"}}).to_string(),
        )
        .expect("local manifest");
        config["routes"][0]["executable"] = json!(node);
        config["routes"][0]["entry_script"] = json!(local_script);
        std::fs::write(&config_path, config.to_string()).expect("local script config");
        assert!(matches!(
            load_dispatch_config(&config_path, &root),
            Err(DispatchError::ConfigExecutableInvalid)
        ));
    }
}
