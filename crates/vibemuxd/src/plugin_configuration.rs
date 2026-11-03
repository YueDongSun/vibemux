//! Explicit, bounded daemon startup configuration. No discovery or IPC writes.

use crate::plugin_registry::{
    HARD_MAX_LIFECYCLE_TIMEOUT, PluginRegistry, PluginRegistryConfig, PluginRegistryError,
    PluginRestartPolicy,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use vibemux_plugin_protocol::{
    frame::FrameCodecConfig,
    manifest::{ManifestPluginKind, PluginManifest, SupportedPlatform},
    negotiation::CorePluginPolicy,
    terminal::{
        CLEANUP_INVENTORY_METHOD, FOCUS_METHOD, FOCUS_PERMISSION, INVENTORY_METHOD,
        OBSERVE_PERMISSION,
    },
};
use vibemux_plugin_supervisor::{PluginSupervisorConfig, ResolvedPluginLaunch};

pub const PLUGIN_CONFIGURATION_VERSION: u32 = 2;
pub const MAX_PLUGIN_CONFIGURATION_BYTES: usize = 64 * 1024;
const HEARTBEAT_GRACE_INTERVALS: u32 = 3;

#[derive(Default)]
pub struct PluginStartup {
    pub registry_config: PluginRegistryConfig,
    pub registrations: Vec<(PluginSupervisorConfig, PluginRestartPolicy)>,
}

impl PluginStartup {
    /// Candidates come only from explicit terminal registrations. Availability
    /// and negotiated observe/process grants are still checked on every query.
    #[cfg(windows)]
    pub(crate) fn cleanup_candidates(&self) -> Vec<String> {
        self.registrations
            .iter()
            .filter(|(config, _)| {
                config.manifest.kind == ManifestPluginKind::Terminal
                    && config
                        .manifest
                        .capabilities
                        .iter()
                        .any(|name| name == CLEANUP_INVENTORY_METHOD)
            })
            .map(|(config, _)| config.manifest.plugin_id.clone())
            .collect()
    }
    pub fn validate(&self) -> Result<(), PluginRegistryError> {
        self.registry_config.validate()?;
        if self.registrations.len() > self.registry_config.max_plugins {
            return Err(PluginRegistryError::CapacityExceeded);
        }
        // Registry also enforces these limits; preflight prevents partial startup.
        let mut ids = BTreeSet::new();
        for (config, policy) in &self.registrations {
            PluginRegistry::validate_registration(config, policy)?;
            if !ids.insert(config.manifest.plugin_id.as_str()) {
                return Err(PluginRegistryError::DuplicatePlugin);
            }
        }
        // Validation never registers or launches a process.
        Ok(())
    }

    /// An operator-supplied local file authorizes only explicit startup launches.
    /// Grants are explicit and restricted to the terminal observation surface.
    pub fn from_path(path: &Path) -> Result<Self, &'static str> {
        let invalid = "plugin_configuration_invalid";
        let metadata = std::fs::symlink_metadata(path).map_err(|_| invalid)?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_PLUGIN_CONFIGURATION_BYTES as u64
        {
            return Err(invalid);
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| invalid)?
            .take((MAX_PLUGIN_CONFIGURATION_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid)?;
        if bytes.len() > MAX_PLUGIN_CONFIGURATION_BYTES {
            return Err(invalid);
        }
        let file: PluginConfiguration = serde_json::from_slice(&bytes).map_err(|_| invalid)?;
        if !matches!(file.schema_version, 1 | PLUGIN_CONFIGURATION_VERSION) {
            return Err(invalid);
        }
        let registry_config = PluginRegistryConfig {
            max_plugins: file.max_plugins,
        };
        registry_config.validate().map_err(|_| invalid)?;
        if file.plugins.len() > registry_config.max_plugins {
            return Err(invalid);
        }
        let mut startup = Self {
            registry_config,
            registrations: Vec::new(),
        };
        for entry in file.plugins {
            if file.schema_version == 1
                && (!entry.granted_permissions.is_empty() || !entry.granted_capabilities.is_empty())
            {
                return Err(invalid);
            }
            if !entry.manifest_path.is_absolute()
                || !entry.executable.is_absolute()
                || !entry.working_directory.is_absolute()
            {
                return Err(invalid);
            }
            let manifest = PluginManifest::from_path(&entry.manifest_path).map_err(|_| invalid)?;
            if entry.granted_capabilities.iter().any(|name| {
                !matches!(
                    name.as_str(),
                    INVENTORY_METHOD | CLEANUP_INVENTORY_METHOD | FOCUS_METHOD
                )
            }) || entry.granted_permissions.iter().any(|name| {
                !matches!(
                    name.as_str(),
                    OBSERVE_PERMISSION | FOCUS_PERMISSION | "process:execute"
                )
            }) || ((!entry.granted_capabilities.is_empty()
                || !entry.granted_permissions.is_empty())
                && manifest.kind != ManifestPluginKind::Terminal)
            {
                return Err(invalid);
            }
            let platform = if cfg!(windows) {
                SupportedPlatform::Windows
            } else if cfg!(target_os = "macos") {
                SupportedPlatform::Macos
            } else {
                SupportedPlatform::Linux
            };
            let plugin_policy = CorePluginPolicy::new(
                entry.granted_capabilities,
                entry.granted_permissions,
                FrameCodecConfig::default(),
                8,
                1000,
                platform,
            )
            .map_err(|_| invalid)?;
            let launch = ResolvedPluginLaunch::new(
                &manifest,
                entry.executable,
                entry.working_directory,
                BTreeMap::new(),
            )
            .map_err(|_| invalid)?;
            let mut config = PluginSupervisorConfig::new(
                manifest,
                launch,
                plugin_policy,
                "startup_validation".to_string(),
            );
            config.receive_timeout = Duration::from_millis(config.policy.heartbeat_interval_ms())
                .saturating_mul(HEARTBEAT_GRACE_INTERVALS);
            config.handshake_timeout = HARD_MAX_LIFECYCLE_TIMEOUT;
            config.shutdown_timeout = HARD_MAX_LIFECYCLE_TIMEOUT;
            let policy = entry
                .restart
                .map_or_else(PluginRestartPolicy::default, |restart| {
                    PluginRestartPolicy {
                        max_restarts: restart.max_restarts,
                        initial_backoff: Duration::from_millis(restart.initial_backoff_ms),
                        max_backoff: Duration::from_millis(restart.max_backoff_ms),
                    }
                });
            startup.registrations.push((config, policy));
        }
        startup.validate().map_err(|_| invalid)?;
        Ok(startup)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginConfiguration {
    schema_version: u32,
    #[serde(default = "default_max_plugins")]
    max_plugins: usize,
    plugins: Vec<PluginEntry>,
}

fn default_max_plugins() -> usize {
    PluginRegistryConfig::default().max_plugins
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginEntry {
    manifest_path: PathBuf,
    executable: PathBuf,
    working_directory: PathBuf,
    #[serde(default)]
    granted_capabilities: Vec<String>,
    #[serde(default)]
    granted_permissions: Vec<String>,
    #[serde(default)]
    restart: Option<RestartConfiguration>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestartConfiguration {
    max_restarts: u32,
    initial_backoff_ms: u64,
    max_backoff_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_inventory_requires_an_explicit_version_two_grant() {
        let temp = tempfile::tempdir().expect("owned config fixture");
        let manifest_path = temp.path().join("observer.toml");
        std::fs::write(
            &manifest_path,
            r#"schema_version=1
plugin_id="cleanup_observer"
plugin_version="1.1.0"
kind="terminal"
entry_point=["observer"]
capabilities=["terminal:cleanup_inventory_v1"]
requested_permissions=["terminal:observe","process:execute"]
supported_platforms=["windows","linux","macos"]
protocol_major=1
minimum_protocol_minor=0
maximum_protocol_minor=0
"#,
        )
        .expect("manifest");
        let path = temp.path().join("plugins.json");
        let mut value = serde_json::json!({"schema_version":2,"plugins":[{
            "manifest_path":manifest_path,"executable":std::env::current_exe().expect("existing test image"),
            "working_directory":temp.path(),"granted_capabilities":["terminal:cleanup_inventory_v1"],
            "granted_permissions":["terminal:observe","process:execute"]}]});
        std::fs::write(&path, serde_json::to_vec(&value).expect("config")).expect("write");
        let explicit = PluginStartup::from_path(&path).expect("explicit opt-in cleanup method");
        assert_eq!(explicit.registrations.len(), 1);
        value["schema_version"] = serde_json::json!(1);
        std::fs::write(&path, serde_json::to_vec(&value).expect("config")).expect("write");
        assert!(
            PluginStartup::from_path(&path).is_err(),
            "v1 never acquires grants"
        );
        value["schema_version"] = serde_json::json!(2);
        value["plugins"][0]["granted_capabilities"] = serde_json::json!([]);
        value["plugins"][0]["granted_permissions"] = serde_json::json!([]);
        std::fs::write(&path, serde_json::to_vec(&value).expect("config")).expect("write");
        let ungranted = PluginStartup::from_path(&path).expect("ungranted configured plugin");
        assert_ne!(
            ungranted.registrations[0].0.policy,
            explicit.registrations[0].0.policy
        );
        value["plugins"][0]["granted_capabilities"] =
            serde_json::json!(["terminal:cleanup_inventory_v2"]);
        std::fs::write(&path, serde_json::to_vec(&value).expect("config")).expect("write");
        assert!(
            PluginStartup::from_path(&path).is_err(),
            "unknown method remains refused"
        );
    }

    #[test]
    fn configuration_is_explicit_bounded_and_fail_closed() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("plugins.json");
        for input in [
            br#"{"schema_version":1,"plugins":[],"task":"mutate"}"#.as_slice(),
            br#"{"schema_version":3,"plugins":[]}"#.as_slice(),
            br#"{"schema_version":1,"max_plugins":33,"plugins":[]}"#.as_slice(),
            br#"{"schema_version":1,"plugins":[{"manifest_path":"relative.toml","executable":"relative.exe","working_directory":"."}]}"#.as_slice(),
            &vec![b' '; MAX_PLUGIN_CONFIGURATION_BYTES + 1],
        ] {
            std::fs::write(&path, input).expect("config");
            assert!(PluginStartup::from_path(&path).is_err());
        }
        std::fs::write(&path, br#"{"schema_version":1,"plugins":[]}"#).expect("valid");
        assert!(
            PluginStartup::from_path(&path)
                .expect("empty config")
                .registrations
                .is_empty()
        );
    }
}
