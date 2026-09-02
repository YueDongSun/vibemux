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
use vibemux_plugin_protocol::{manifest::PluginManifest, negotiation::CorePluginPolicy};
use vibemux_plugin_supervisor::{PluginSupervisorConfig, ResolvedPluginLaunch};

pub const PLUGIN_CONFIGURATION_VERSION: u32 = 1;
pub const MAX_PLUGIN_CONFIGURATION_BYTES: usize = 64 * 1024;
const HEARTBEAT_GRACE_INTERVALS: u32 = 3;

#[derive(Default)]
pub struct PluginStartup {
    pub registry_config: PluginRegistryConfig,
    pub registrations: Vec<(PluginSupervisorConfig, PluginRestartPolicy)>,
}

impl PluginStartup {
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
    /// Manifest permissions are declarations: none are granted in M4.2.
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
        if file.schema_version != PLUGIN_CONFIGURATION_VERSION {
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
            if !entry.manifest_path.is_absolute()
                || !entry.executable.is_absolute()
                || !entry.working_directory.is_absolute()
            {
                return Err(invalid);
            }
            let manifest = PluginManifest::from_path(&entry.manifest_path).map_err(|_| invalid)?;
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
                CorePluginPolicy::default(),
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
    fn configuration_is_explicit_bounded_and_fail_closed() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("plugins.json");
        for input in [
            br#"{"schema_version":1,"plugins":[],"task":"mutate"}"#.as_slice(),
            br#"{"schema_version":2,"plugins":[]}"#.as_slice(),
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
