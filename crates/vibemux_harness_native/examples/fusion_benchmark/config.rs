//! Trusted local experiment configuration. No permissions or model defaults added.

use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::{Value, json};
use vibemux_harness_native::{
    Error,
    config::{Config, MAX_CONFIG_BYTES, Route},
    contract::{Harness, Protocol},
    dispatch::Dispatcher,
};

use super::{cases, evidence, workflow};

const MAX_EXPERIMENT_BYTES: usize = 32 * 1024;
pub const ROLE_NAMES: [&str; 5] = ["draft", "strong", "judge", "critic_a", "critic_b"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Experiment {
    schema_version: u32,
    repetitions: usize,
    max_dispatches: usize,
    budget_seconds: u64,
    plugin: PathBuf,
    machine: Machine,
    roles: Vec<RoleSpec>,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    // Operator observations, explicitly labeled as such in the report.
    os_version: String,
    cpu: String,
    rustc: String,
    source_revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleSpec {
    id: String,
    config: PathBuf,
    harness: Harness,
    vendor_version: String,
}

pub struct Role {
    pub id: String,
    pub dispatcher: Dispatcher,
    pub route: Route,
    config_sha256: String,
    executable_sha256: String,
    vendor_version: String,
}

pub struct Prepared {
    pub repetitions: usize,
    pub max_dispatches: usize,
    pub budget_seconds: u64,
    pub roles: Vec<Role>,
    machine: Machine,
    experiment_sha256: String,
    plugin_sha256: String,
    benchmark_sha256: String,
}

impl Prepared {
    pub fn role(&self, name: &str) -> Result<&Role, Error> {
        self.roles
            .iter()
            .find(|role| role.id == name)
            .ok_or_else(|| Error::new("fusion_missing_role"))
    }

    pub fn header(&self) -> Value {
        json!({
            "schema_version": 1, "record_type": "experiment_started",
            "started_at": evidence::now(), "experiment_id": uuid::Uuid::new_v4(),
            "workflow_version": workflow::VERSION,
            "suite_version": cases::SUITE_VERSION, "suite_sha256": cases::suite_digest(),
            "experiment_sha256": self.experiment_sha256,
            "benchmark_binary_sha256": self.benchmark_sha256,
            "plugin_binary_sha256": self.plugin_sha256,
            "package_version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "debug_assertions": cfg!(debug_assertions),
            "logical_parallelism": std::thread::available_parallelism().ok().map(usize::from),
            "machine_operator_reported": self.machine,
            "repetitions": self.repetitions,
            "planned_samples": cases::cases().len() * self.repetitions * workflow::MODES.len(),
            "max_dispatches": self.max_dispatches, "budget_seconds": self.budget_seconds,
            "vendor_ancestor_and_global_context": "unmeasured_operator_controlled",
            "roles": self.roles.iter().map(|role| json!({
                "id": role.id, "harness": role.route.harness, "protocol": role.route.protocol,
                "requested_model": role.route.model, "reported_model": Value::Null,
                "vendor_version_operator_reported": role.vendor_version,
                "config_sha256": role.config_sha256,
                "executable_sha256": role.executable_sha256
            })).collect::<Vec<_>>(),
            "provider_requests": Value::Null, "reported_usage": Value::Null, "known_cost": Value::Null,
            "source": "github/docs@5c9f8941dd6b10b27f350ea7de093aebc5d2a3ef",
            "claims": "Fusion-inspired coding-reasoning experiment; no GitHub router, writable coding, canonical Task/Run, or hostile-code sandbox claim"
        })
    }
}

pub fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, Error> {
    let file = std::fs::File::open(path).map_err(|_| Error::new("fusion_input_unavailable"))?;
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::new("fusion_input_unavailable"))?;
    if bytes.len() > maximum {
        return Err(Error::new("fusion_input_too_large"));
    }
    Ok(bytes)
}

fn resolve(parent: &Path, path: &Path) -> Result<PathBuf, Error> {
    parent
        .join(path)
        .canonicalize()
        .map_err(|_| Error::new("fusion_path_unavailable"))
}

fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.starts_with("REPLACE_")
        && !value.starts_with("RECORD_")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b" ._:/()+-".contains(&b))
}

pub fn prepare(path: &Path) -> Result<Prepared, Error> {
    let path = path
        .canonicalize()
        .map_err(|_| Error::new("fusion_config_unavailable"))?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("fusion_config_unavailable"))?;
    let bytes = read_bounded(&path, MAX_EXPERIMENT_BYTES)?;
    let config: Experiment =
        serde_json::from_slice(&bytes).map_err(|_| Error::new("fusion_invalid_config"))?;
    if config.schema_version != 1
        || !(1..=10).contains(&config.repetitions)
        || !(1..=360).contains(&config.max_dispatches)
        || !(1..=3600).contains(&config.budget_seconds)
        || config.roles.len() != ROLE_NAMES.len()
        || [
            &config.machine.os_version,
            &config.machine.cpu,
            &config.machine.rustc,
            &config.machine.source_revision,
        ]
        .iter()
        .any(|value| !label(value))
    {
        return Err(Error::new("fusion_invalid_config"));
    }
    let plugin = resolve(parent, &config.plugin)?;
    let plugin_sha256 = evidence::hash_file(&plugin)?;
    let mut roles = Vec::new();
    let mut ids = BTreeSet::new();
    let mut directories: Vec<PathBuf> = Vec::new();
    for spec in config.roles {
        if !ROLE_NAMES.contains(&spec.id.as_str())
            || !ids.insert(spec.id.clone())
            || !label(&spec.vendor_version)
        {
            return Err(Error::new("fusion_invalid_role"));
        }
        let route_path = resolve(parent, &spec.config)?;
        let route_bytes = read_bounded(&route_path, MAX_CONFIG_BYTES)?;
        let mut native: Config = serde_json::from_slice(&route_bytes)
            .map_err(|_| Error::new("fusion_invalid_native_config"))?;
        native.validate()?;
        if native.harnesses.len() != 1
            || native.limits.timeout_ms > 120_000
            || native.limits.maximum_in_flight != 1
        {
            return Err(Error::new("fusion_role_requires_one_bounded_route"));
        }
        let mut route = native.harnesses.remove(0);
        if route.harness != spec.harness || !route.enabled || !route.allow_execution
            || !matches!(route.protocol, Protocol::Codex | Protocol::Appserver | Protocol::Claude)
            || route.model.as_ref().is_none_or(|model| !label(model))
            // Prefix arguments could override the experiment's model/transport or
            // load unmeasured hooks. The native fixed launch profile owns argv.
            || !route.arguments.is_empty()
        {
            return Err(Error::new("fusion_requires_enabled_pinned_native_model"));
        }
        let route_parent = route_path
            .parent()
            .ok_or_else(|| Error::new("fusion_invalid_role"))?;
        route.executable = resolve(route_parent, &route.executable)?;
        route.working_directory = resolve(route_parent, &route.working_directory)?;
        if !route.working_directory.is_dir() || !route.executable.is_file() {
            return Err(Error::new("fusion_invalid_role_path"));
        }
        // No shared or nested contexts between independent critics/roles. These
        // are operator-owned read-only working directories, never writable Runs.
        if directories
            .iter()
            .any(|other| overlaps(other, &route.working_directory))
        {
            return Err(Error::new("fusion_role_directories_overlap"));
        }
        directories.push(route.working_directory.clone());
        let dispatcher = Dispatcher::new(&route_path, &plugin)?;
        if read_bounded(&route_path, MAX_CONFIG_BYTES)? != route_bytes {
            return Err(Error::new("fusion_config_changed_during_setup"));
        }
        roles.push(Role {
            id: spec.id,
            dispatcher,
            executable_sha256: evidence::hash_file(&route.executable)?,
            route,
            config_sha256: evidence::digest(&route_bytes),
            vendor_version: spec.vendor_version,
        });
    }
    let prepared = Prepared {
        repetitions: config.repetitions,
        max_dispatches: config.max_dispatches,
        budget_seconds: config.budget_seconds,
        roles,
        machine: config.machine,
        experiment_sha256: evidence::digest(&bytes),
        plugin_sha256,
        benchmark_sha256: evidence::hash_file(
            &std::env::current_exe().map_err(|_| Error::new("fusion_executable_unavailable"))?,
        )?,
    };
    // Labels such as "strong" are operator assignments, not model rankings.
    // Still refuse a comparison that silently uses the same requested model.
    let identity = |role: &Role| role.route.model.clone();
    if identity(prepared.role("draft")?) == identity(prepared.role("strong")?)
        || identity(prepared.role("critic_a")?) == identity(prepared.role("critic_b")?)
    {
        return Err(Error::new("fusion_comparison_requires_distinct_models"));
    }
    Ok(prepared)
}

fn overlaps(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    let (left_owned, right_owned) = (
        PathBuf::from(left.as_os_str().to_string_lossy().to_lowercase()),
        PathBuf::from(right.as_os_str().to_string_lossy().to_lowercase()),
    );
    #[cfg(windows)]
    let (left, right) = (left_owned.as_path(), right_owned.as_path());
    // Path component comparison avoids the worktrees/worktrees-evil prefix trap.
    left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independence_rejects_nested_contexts_without_prefix_traps() {
        assert!(overlaps(Path::new("work/a"), Path::new("work/a/child")));
        assert!(overlaps(Path::new("work/a"), Path::new("work/a")));
        assert!(!overlaps(Path::new("work/a"), Path::new("work/ab")));
        assert!(!overlaps(Path::new("work/a"), Path::new("work/b")));
    }

    #[test]
    fn report_labels_are_bounded_and_have_no_control_text() {
        assert!(label("Rust 1.92.0 (operator observed)"));
        assert!(!label("secret\nsecond line"));
        assert!(!label(&"a".repeat(161)));
    }
}
