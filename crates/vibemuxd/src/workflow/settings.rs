//! Pinned operator workflow configuration (ADR 031 §7).
//!
//! The daemon reads `<state dir>/workflow_config.json` once at startup and
//! pins it with the digest of the exact bytes, like the dispatch route
//! config: a changed file takes effect only after a restart. It names the
//! worker and reviewer slots (each bound to a harness whose dispatch route
//! may execute and to one AAG route role), the trusted verifier, the Git
//! executable for worktrees and candidate collection, the optional AAG
//! gateway, and the timing and size limits.
//!
//! The Git executable, the verifier executable, and the verifier directory
//! must be absolute and must resolve outside the project root, so a
//! repository cannot ship the code that collects or judges its own
//! candidates. The verifier directory is hashed at startup; a workflow
//! records that digest and every verification re-checks it.

use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use vibemux_harness::AgentKind;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier, canonical_json::canonical_digest, gateway::GatewayConfig,
    gateway::RouteRole, slots::EvidenceClass,
};

use super::error::WorkflowError;

pub const WORKFLOW_CONFIG_FILE_NAME: &str = "workflow_config.json";
pub const WORKFLOW_CONFIG_SCHEMA_VERSION: u32 = 1;
const CONFIG_DIGEST_DOMAIN: &str = "vibemux.workflow.config.v1";
const VERIFIER_DIGEST_DOMAIN: &str = "vibemux.workflow.verifier_files.v1";
const MAX_WORKFLOW_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_VERIFIER_FILES: usize = 512;
const MAX_VERIFIER_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_VERIFIER_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_VERIFIER_DEPTH: usize = 8;
pub const MIN_HEARTBEAT_MS: u64 = 1_000;
pub const MAX_HEARTBEAT_MS: u64 = 600_000;
pub const MIN_VERIFIER_TIMEOUT_MS: u64 = 1_000;
pub const MAX_VERIFIER_TIMEOUT_MS: u64 = 1_800_000;
const MAX_SLOTS: usize = 8;
const MAX_SUITES: usize = 16;
const MAX_SUITE_ARGUMENTS: usize = 32;
const MAX_ARGUMENT_BYTES: usize = 1024;
const MAX_RETENTION_DAYS: u32 = 365;
const MAX_ENVIRONMENT_NAMES: usize = 16;
/// Placeholders a suite argument may contain.
pub const VERIFIER_DIR_PLACEHOLDER: &str = "{verifier_dir}";
pub const CANDIDATE_PLACEHOLDER: &str = "{candidate}";
pub const OUT_PLACEHOLDER: &str = "{out}";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowConfig {
    pub schema_version: u32,
    /// The class of every slot's evidence and of every workflow result:
    /// fixture workers can never produce live evidence.
    pub evidence_class: EvidenceClass,
    pub git_executable: PathBuf,
    pub slots: Vec<SlotConfig>,
    pub verifier: VerifierConfig,
    #[serde(default)]
    pub gateway: Option<GatewayConfig>,
    pub heartbeat_ms: u64,
    pub candidate_limits: CandidateLimits,
    /// Opt-in private content retention; `None` keeps no content.
    #[serde(default)]
    pub content_retention_days: Option<u32>,
}

/// One worker or reviewer slot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SlotConfig {
    pub slot_id: SpecIdentifier,
    pub harness: AgentKind,
    /// The AAG role this slot's model traffic is bound to: `worker_a`,
    /// `worker_b`, or `reviewer`.
    pub route_role: RouteRole,
}

impl SlotConfig {
    #[must_use]
    pub fn is_reviewer(&self) -> bool {
        self.route_role == RouteRole::Reviewer
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierConfig {
    pub directory: PathBuf,
    pub executable: PathBuf,
    /// Suite ID to argv after the executable.
    pub suites: BTreeMap<SpecIdentifier, Vec<String>>,
    pub timeout_ms: u64,
    /// Daemon environment names forwarded to the verifier besides the
    /// system names (for example `LOCALAPPDATA` for a browser suite).
    #[serde(default)]
    pub environment_names: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateLimits {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

/// A validated config with its canonical paths and pinned digests.
pub struct LoadedWorkflowConfig {
    pub config: WorkflowConfig,
    pub digest: Sha256Digest,
    pub verifier_digest: Sha256Digest,
    pub git_executable: PathBuf,
    pub verifier_directory: PathBuf,
    pub verifier_executable: PathBuf,
}

impl fmt::Debug for LoadedWorkflowConfig {
    /// Redacted: paths never reach a log.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoadedWorkflowConfig")
            .field("digest", &self.digest)
            .field("verifier_digest", &self.verifier_digest)
            .field("slots", &self.config.slots.len())
            .finish_non_exhaustive()
    }
}

impl LoadedWorkflowConfig {
    #[must_use]
    pub fn slot(&self, slot_id: &SpecIdentifier) -> Option<&SlotConfig> {
        self.config
            .slots
            .iter()
            .find(|slot| &slot.slot_id == slot_id)
    }

    /// The route a slot's model traffic is bound to: the explicit AAG
    /// binding of its role, or the fixture label when no gateway exists
    /// (only fixture-class configs may omit the gateway).
    #[must_use]
    pub fn route_label(&self, slot: &SlotConfig) -> String {
        match self
            .config
            .gateway
            .as_ref()
            .and_then(|gateway| gateway.binding(slot.route_role))
        {
            Some(binding) => format!("aag:{}:{}", slot.route_role.as_str(), binding.model_alias),
            None => format!("fixture:{}", slot.route_role.as_str()),
        }
    }

    /// Recomputes the verifier digest; a difference means the trusted
    /// verifier changed under a running daemon.
    pub fn check_verifier_unchanged(&self) -> Result<(), WorkflowError> {
        let current = verifier_digest(&self.verifier_directory, &self.config.verifier)?;
        if current == self.verifier_digest {
            Ok(())
        } else {
            Err(WorkflowError::VerifierChanged)
        }
    }
}

/// Reads, validates, and pins the config. A missing file means workflows
/// are not configured.
pub fn load_workflow_config(
    config_path: &Path,
    canonical_root: &Path,
) -> Result<LoadedWorkflowConfig, WorkflowError> {
    let bytes = read_regular_file(config_path, MAX_WORKFLOW_CONFIG_BYTES)?;
    let config: WorkflowConfig =
        serde_json::from_slice(&bytes).map_err(|_| WorkflowError::ConfigInvalid)?;
    validate(&config)?;
    let git_executable = trusted_file(&config.git_executable, canonical_root)?;
    let verifier_executable = trusted_file(&config.verifier.executable, canonical_root)?;
    let verifier_directory = trusted_directory(&config.verifier.directory, canonical_root)?;
    let verifier_digest = verifier_digest(&verifier_directory, &config.verifier)?;
    let digest = Sha256Digest::of_fields(CONFIG_DIGEST_DOMAIN, &[&bytes]);
    Ok(LoadedWorkflowConfig {
        config,
        digest,
        verifier_digest,
        git_executable,
        verifier_directory,
        verifier_executable,
    })
}

fn validate(config: &WorkflowConfig) -> Result<(), WorkflowError> {
    let invalid = || WorkflowError::ConfigInvalid;
    if config.schema_version != WORKFLOW_CONFIG_SCHEMA_VERSION
        || config.slots.is_empty()
        || config.slots.len() > MAX_SLOTS
        || !(MIN_HEARTBEAT_MS..=MAX_HEARTBEAT_MS).contains(&config.heartbeat_ms)
        || !(MIN_VERIFIER_TIMEOUT_MS..=MAX_VERIFIER_TIMEOUT_MS)
            .contains(&config.verifier.timeout_ms)
        || config.verifier.suites.is_empty()
        || config.verifier.suites.len() > MAX_SUITES
        || config
            .content_retention_days
            .is_some_and(|days| days == 0 || days > MAX_RETENTION_DAYS)
    {
        return Err(invalid());
    }
    if config.verifier.environment_names.len() > MAX_ENVIRONMENT_NAMES
        || !config.verifier.environment_names.iter().all(|name| {
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        })
    {
        return Err(invalid());
    }
    // Live evidence needs every slot's model traffic bound to an explicit
    // AAG route; only fixture workers may run without a gateway.
    if config.evidence_class == EvidenceClass::Live && config.gateway.is_none() {
        return Err(invalid());
    }
    let limits = config.candidate_limits;
    if limits.max_files == 0
        || limits.max_files > 4096
        || limits.max_file_bytes == 0
        || limits.max_file_bytes > 16 * 1024 * 1024
        || limits.max_total_bytes < limits.max_file_bytes
        || limits.max_total_bytes > 256 * 1024 * 1024
    {
        return Err(invalid());
    }
    let mut ids: Vec<&SpecIdentifier> = config.slots.iter().map(|slot| &slot.slot_id).collect();
    ids.sort();
    ids.dedup();
    if ids.len() != config.slots.len() {
        return Err(invalid());
    }
    for slot in &config.slots {
        if !matches!(
            slot.route_role,
            RouteRole::WorkerA | RouteRole::WorkerB | RouteRole::Reviewer
        ) {
            return Err(invalid());
        }
    }
    for arguments in config.verifier.suites.values() {
        if arguments.is_empty()
            || arguments.len() > MAX_SUITE_ARGUMENTS
            || arguments.iter().any(|argument| {
                argument.is_empty()
                    || argument.len() > MAX_ARGUMENT_BYTES
                    || argument.chars().any(char::is_control)
            })
            || !arguments
                .iter()
                .any(|argument| argument.contains(CANDIDATE_PLACEHOLDER))
            || !arguments
                .iter()
                .any(|argument| argument.contains(OUT_PLACEHOLDER))
        {
            return Err(invalid());
        }
    }
    if let Some(gateway) = &config.gateway {
        gateway.validate().map_err(|_| invalid())?;
        // Every slot's role must be bound: a slot never falls back to a
        // gateway default.
        for slot in &config.slots {
            if gateway.binding(slot.route_role).is_none() {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

/// Absolute, regular, and outside the project root once resolved.
fn trusted_file(path: &Path, canonical_root: &Path) -> Result<PathBuf, WorkflowError> {
    if !path.is_absolute() {
        return Err(WorkflowError::Untrusted);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| WorkflowError::Untrusted)?;
    let metadata = std::fs::metadata(&canonical).map_err(|_| WorkflowError::Untrusted)?;
    if !metadata.is_file() || canonical.starts_with(canonical_root) {
        return Err(WorkflowError::Untrusted);
    }
    Ok(canonical)
}

fn trusted_directory(path: &Path, canonical_root: &Path) -> Result<PathBuf, WorkflowError> {
    if !path.is_absolute() {
        return Err(WorkflowError::Untrusted);
    }
    let link = std::fs::symlink_metadata(path).map_err(|_| WorkflowError::Untrusted)?;
    if !link.is_dir() {
        return Err(WorkflowError::Untrusted);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| WorkflowError::Untrusted)?;
    if canonical.starts_with(canonical_root) || canonical_root.starts_with(&canonical) {
        return Err(WorkflowError::Untrusted);
    }
    Ok(canonical)
}

/// Digest of every verifier file (relative path and content hash) and the
/// suite argv templates. Symlinks and special files are refused.
fn verifier_digest(
    directory: &Path,
    verifier: &VerifierConfig,
) -> Result<Sha256Digest, WorkflowError> {
    let mut files = Vec::new();
    let mut total = 0_u64;
    collect_verifier_files(directory, "", 0, &mut files, &mut total)?;
    files.sort();
    let executable_name = verifier
        .executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    canonical_digest(
        VERIFIER_DIGEST_DOMAIN,
        &json!({
            "files": files,
            "executable_name": executable_name,
            "suites": verifier.suites,
        }),
    )
    .map_err(|_| WorkflowError::Untrusted)
}

fn collect_verifier_files(
    directory: &Path,
    prefix: &str,
    depth: usize,
    files: &mut Vec<(String, String)>,
    total: &mut u64,
) -> Result<(), WorkflowError> {
    if depth > MAX_VERIFIER_DEPTH {
        return Err(WorkflowError::Untrusted);
    }
    let entries = std::fs::read_dir(directory).map_err(|_| WorkflowError::Untrusted)?;
    for entry in entries {
        let entry = entry.map_err(|_| WorkflowError::Untrusted)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| WorkflowError::Untrusted)?;
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let metadata =
            std::fs::symlink_metadata(entry.path()).map_err(|_| WorkflowError::Untrusted)?;
        if metadata.is_dir() {
            collect_verifier_files(&entry.path(), &relative, depth + 1, files, total)?;
        } else if metadata.is_file() {
            let bytes = read_regular_file(&entry.path(), MAX_VERIFIER_FILE_BYTES)
                .map_err(|_| WorkflowError::Untrusted)?;
            *total += bytes.len() as u64;
            if files.len() >= MAX_VERIFIER_FILES || *total > MAX_VERIFIER_TOTAL_BYTES {
                return Err(WorkflowError::Untrusted);
            }
            files.push((relative, Sha256Digest::of(&bytes).to_hex()));
        } else {
            return Err(WorkflowError::Untrusted);
        }
    }
    Ok(())
}

fn read_regular_file(path: &Path, maximum: u64) -> Result<Vec<u8>, WorkflowError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(WorkflowError::Unconfigured);
        }
        Err(_) => return Err(WorkflowError::ConfigInvalid),
    };
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(WorkflowError::ConfigInvalid);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(maximum + 1).read_to_end(&mut bytes))
        .map_err(|_| WorkflowError::ConfigInvalid)?;
    if bytes.len() as u64 > maximum {
        return Err(WorkflowError::ConfigInvalid);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_json(verifier_dir: &Path, executable: &Path) -> serde_json::Value {
        json!({
            "schema_version": 1,
            "evidence_class": "fixture",
            "git_executable": executable,
            "slots": [
                {"slot_id": "slot_a", "harness": "claude", "route_role": "worker_a"},
                {"slot_id": "review", "harness": "codex", "route_role": "reviewer"},
            ],
            "verifier": {
                "directory": verifier_dir,
                "executable": executable,
                "suites": {"api": ["{verifier_dir}/run.mjs", "--candidate", "{candidate}", "--out", "{out}"]},
                "timeout_ms": 60_000,
            },
            "heartbeat_ms": 30_000,
            "candidate_limits": {"max_files": 64, "max_file_bytes": 262_144, "max_total_bytes": 1_048_576},
        })
    }

    #[test]
    fn verifier_and_executables_must_live_outside_the_project_root() {
        let outside = tempfile::tempdir().expect("outside");
        let project = tempfile::tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let verifier = outside.path().join("verifier");
        std::fs::create_dir(&verifier).expect("verifier dir");
        std::fs::write(verifier.join("run.mjs"), b"// trusted").expect("verifier file");
        let executable = outside.path().join("tool.exe");
        std::fs::write(&executable, b"binary").expect("executable");
        let path = root.join(WORKFLOW_CONFIG_FILE_NAME);
        std::fs::write(&path, config_json(&verifier, &executable).to_string()).expect("config");
        let loaded = load_workflow_config(&path, &root).expect("valid config");
        loaded.check_verifier_unchanged().expect("unchanged");
        std::fs::write(verifier.join("run.mjs"), b"// edited").expect("edit verifier");
        assert_eq!(
            loaded.check_verifier_unchanged(),
            Err(WorkflowError::VerifierChanged)
        );

        let inside = root.join("verifier");
        std::fs::create_dir(&inside).expect("inside dir");
        std::fs::write(&path, config_json(&inside, &executable).to_string()).expect("config");
        assert_eq!(
            load_workflow_config(&path, &root).map(|_| ()),
            Err(WorkflowError::Untrusted)
        );
    }

    #[test]
    fn slots_must_bind_worker_or_reviewer_roles_and_suites_need_placeholders() {
        let outside = tempfile::tempdir().expect("outside");
        let executable = outside.path().join("tool.exe");
        let mut value = config_json(outside.path(), &executable);
        value["slots"][0]["route_role"] = json!("supervisor");
        let config: WorkflowConfig = serde_json::from_value(value).expect("shape");
        assert_eq!(validate(&config), Err(WorkflowError::ConfigInvalid));
        let mut value = config_json(outside.path(), &executable);
        value["verifier"]["suites"]["api"] = json!(["run.mjs"]);
        let config: WorkflowConfig = serde_json::from_value(value).expect("shape");
        assert_eq!(validate(&config), Err(WorkflowError::ConfigInvalid));
        let mut value = config_json(outside.path(), &executable);
        value["unknown"] = json!(true);
        assert!(serde_json::from_value::<WorkflowConfig>(value).is_err());
    }
}
