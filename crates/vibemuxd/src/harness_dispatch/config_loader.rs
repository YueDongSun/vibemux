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
//! or `powershell.exe`). ADR 029 also permits the POSIX Claude native
//! installer layout for probe-only routes. DSH's explicit Node launcher pair requires
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
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let home = (route.harness == AgentKind::Claude && !route.allow_execution)
        .then(std::env::home_dir)
        .flatten();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let home: Option<PathBuf> = None;
    trusted_executable_for_home(route, canonical_root, policy, home.as_deref())
}

fn trusted_executable_for_home(
    route: &DispatchRoute,
    canonical_root: &Path,
    policy: ExecutablePolicy,
    _home: Option<&Path>,
) -> Result<PathBuf, DispatchError> {
    let invalid = DispatchError::ConfigExecutableInvalid;
    let canonical = std::fs::canonicalize(&route.executable).map_err(|_| invalid)?;
    let metadata = std::fs::metadata(&canonical).map_err(|_| invalid)?;
    // The configured path passed the shape rules; the target a link resolves
    // to must pass them too.
    if !metadata.is_file()
        || canonical.starts_with(canonical_root)
        || validate_executable(&canonical, policy).is_err()
    {
        return Err(invalid);
    }
    let named = if route.protocol.requires_launcher_pair() {
        executable_is_node_image(&canonical, policy)
    } else {
        executable_names_harness(&canonical, route.harness, policy)
    };
    if named {
        return Ok(canonical);
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if policy == ExecutablePolicy::PosixExecutable
        && route.protocol == vibemux_harness::dispatch::NativeProtocol::ClaudeStreamJson
        && !route.allow_execution
        && _home.is_some_and(|home| {
            claude_installer_image(home, &route.executable, &canonical, &metadata)
        })
    {
        return Ok(canonical);
    }
    Err(invalid)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn claude_installer_image(
    home: &Path,
    launcher: &Path,
    canonical: &Path,
    metadata: &std::fs::Metadata,
) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Some(version) = canonical.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !vibemux_harness::dispatch::route_config::claude_native_version_name(version)
        || !home.is_absolute()
        || launcher != home.join(".local/bin/claude")
        || !std::fs::symlink_metadata(launcher).is_ok_and(|meta| meta.is_symlink())
        || metadata.permissions().mode() & 0o111 == 0
    {
        return false;
    }
    let Ok(home) = home.canonicalize() else {
        return false;
    };
    if launcher
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        != Some(home.join(".local/bin"))
        || canonical != home.join(".local/share/claude/versions").join(version)
    {
        return false;
    }
    let mut magic = [0; 4];
    if File::open(canonical)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_err()
    {
        return false;
    }
    #[cfg(target_os = "linux")]
    return magic == *b"\x7fELF";
    #[cfg(target_os = "macos")]
    return matches!(
        u32::from_be_bytes(magic),
        0xfeed_face
            | 0xcefa_edfe
            | 0xfeed_facf
            | 0xcffa_edfe
            | 0xcafe_babe
            | 0xbeba_feca
            | 0xcafe_babf
            | 0xbfba_feca
    );
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

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn claude_installer_probe_pins_only_the_external_native_version_image() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temp = tempfile::tempdir().expect("fixture");
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        let launcher = home.join(".local/bin/claude");
        let versions = home.join(".local/share/claude/versions");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::create_dir_all(launcher.parent().expect("launcher parent")).expect("bin");
        std::fs::create_dir_all(&versions).expect("versions");
        let image = versions.join("2.1.322");
        // Use the native header from the running test. The fixture is never launched.
        let mut magic = [0; 4];
        File::open(std::env::current_exe().expect("test executable"))
            .expect("open test executable")
            .read_exact(&mut magic)
            .expect("native header");
        std::fs::write(&image, magic).expect("image");
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        symlink(&image, &launcher).expect("installer symlink");
        let root = project.canonicalize().expect("canonical root");
        let config = DispatchConfig::parse(
            &serde_json::to_vec(&json!({"schema_version": 1, "routes": [{
                "harness": "claude", "protocol": "claude_stream_json",
                "executable": launcher, "enabled": true, "allow_execution": false
            }]}))
            .expect("config JSON"),
            ExecutablePolicy::for_host(),
        )
        .expect("config");
        let mut route = config.config.routes[0].clone();
        let check = |route: &DispatchRoute, root: &Path| {
            trusted_executable_for_home(route, root, ExecutablePolicy::for_host(), Some(&home))
        };
        let invalid = Err(DispatchError::ConfigExecutableInvalid);
        assert_eq!(
            check(&route, &root),
            Ok(image.canonicalize().expect("canonical image"))
        );
        assert_eq!(
            check(&route, &home.canonicalize().expect("home as project")),
            invalid
        );

        route.allow_execution = true;
        assert_eq!(check(&route, &root), invalid);
        route.allow_execution = false;
        route.executable = image.clone();
        assert_eq!(check(&route, &root), invalid);
        route.executable = launcher.clone();

        let alias = temp.path().join("claude");
        symlink(&image, &alias).expect("unrelated symlink");
        route.executable = alias;
        assert_eq!(check(&route, &root), invalid);
        route.executable = launcher.clone();

        std::fs::remove_file(&launcher).expect("remove link");
        symlink("/bin/sh", &launcher).expect("interpreter link");
        assert_eq!(check(&route, &root), invalid);
        std::fs::remove_file(&launcher).expect("remove link");

        let other = project.join("2.1.322");
        std::fs::rename(&image, &other).expect("move image into project");
        symlink(&other, &image).expect("version link");
        symlink(&image, &launcher).expect("installer symlink");
        assert_eq!(check(&route, &root), invalid);
        std::fs::remove_file(&image).expect("remove version link");

        std::fs::write(&image, b"#!/bin/sh\nexit 0\n").expect("script");
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(check(&route, &root), invalid);
    }

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
