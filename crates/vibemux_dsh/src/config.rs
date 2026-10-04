//! Operator configuration for one DSH session (ADR 030 Decision 7).
//!
//! `.vibemux/dsh_session.json` (schema 1) is read once, validated, and its
//! trusted paths canonicalized before any process starts. The file carries no
//! prompt text, no credential, and no operator argv: the executable, the
//! launcher script, and the profile are the only inputs, and the prompt
//! travels inside JSON on stdin.
//!
//! Trust rules, applied to canonical targets rather than to the configured
//! strings:
//!
//! - the executable must be a native image whose file stem is `node`, because
//!   DSH's vendor runtime *is* a Node interpreter (ADR 030 Decision 3);
//! - the entry script must be an absolute `.js` file that the installed
//!   `@deepseek-ai/dsh` package's own `bin` map names;
//! - neither target may live inside the project root, so a repository cannot
//!   ship the code a session runs.

use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use vibemux_harness::dispatch::{
    DispatchRoute, NativeProtocol,
    launch_spec::{SessionMode, build_launch_spec},
    route_config::{
        DEFAULT_DSH_PROFILE, DSH_ENTRY_BIN_NAME, DSH_ENTRY_PACKAGE_NAME, ExecutablePolicy,
        executable_is_node_image, profile_is_valid, validate_entry_script_shape,
        validate_executable,
    },
};
use vibemux_harness::dsh::{DshSessionLimits, DshSessionSettings};

use crate::DshDriverError;
use crate::driver::DshLaunch;

pub const DSH_SESSION_CONFIG_FILE_NAME: &str = "dsh_session.json";
pub const DSH_SESSION_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const MAX_SESSION_CONFIG_BYTES: usize = 32 * 1024;
/// How far up from an entry script the owning package manifest may sit.
const MAX_PACKAGE_ANCESTORS: usize = 4;

/// The on-disk document. Deserialize-only projection: nothing here is
/// re-serialized, so a path never leaves through this type.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DshSessionConfigFile {
    pub schema_version: u32,
    pub executable: PathBuf,
    pub entry_script: PathBuf,
    #[serde(default)]
    pub profile: Option<String>,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub environment_names: Vec<String>,
    #[serde(default)]
    pub limits: DshSessionLimits,
}

/// A validated session configuration with canonical, trusted paths.
#[derive(Clone)]
pub struct DshSessionConfig {
    /// Canonical Node image.
    pub executable: PathBuf,
    /// Canonical DSH launcher entry script.
    pub entry_script: PathBuf,
    pub profile: String,
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub max_tokens: Option<u64>,
    /// Environment names forwarded to the runtime, system names first.
    pub environment_names: Vec<String>,
    pub limits: DshSessionLimits,
}

impl std::fmt::Debug for DshSessionConfig {
    /// Redacted: every path in this struct is a local filesystem location.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DshSessionConfig")
            .field("profile", &self.profile)
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("has_reasoning_effort", &self.reasoning_effort.is_some())
            .field("max_tokens", &self.max_tokens)
            .field("environment_name_count", &self.environment_names.len())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl DshSessionConfig {
    /// The shell-free command line this configuration launches. The argv comes
    /// from the ADR 029 protocol profile, never from the config file.
    pub fn launch(&self, working_directory: &Path) -> Result<DshLaunch, DshDriverError> {
        let route = DispatchRoute {
            harness: vibemux_harness::AgentKind::Dsh,
            protocol: NativeProtocol::DshSdkJsonRpc,
            executable: self.executable.clone(),
            entry_script: Some(self.entry_script.clone()),
            profile: Some(self.profile.clone()),
            provider: Some(self.provider.clone()),
            environment_names: self.environment_names.clone(),
            enabled: true,
            allow_execution: true,
            model: Some(self.model.clone()),
        };
        let spec = build_launch_spec(&route, SessionMode::Execute)
            .map_err(|_| DshDriverError::ConfigInvalid)?;
        Ok(DshLaunch {
            executable: spec.executable,
            arguments: spec.arguments,
            environment_names: spec.environment_names,
            working_directory: working_directory.to_path_buf(),
        })
    }

    /// Session settings for one daemon- or operator-chosen session id.
    pub fn settings(
        &self,
        session_id: String,
        working_directory: &Path,
    ) -> Result<DshSessionSettings, DshDriverError> {
        let working_directory = working_directory
            .to_str()
            .ok_or(DshDriverError::ConfigInvalid)?
            .to_string();
        let settings = DshSessionSettings {
            session_id,
            working_directory,
            provider: self.provider.clone(),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            max_tokens: self.max_tokens,
        };
        settings
            .validate()
            .map_err(|_| DshDriverError::ConfigInvalid)?;
        Ok(settings)
    }

    /// Parses and verifies the exact config bytes.
    pub fn parse(
        bytes: &[u8],
        executable_policy: ExecutablePolicy,
        canonical_root: &Path,
    ) -> Result<Self, DshDriverError> {
        if bytes.len() > MAX_SESSION_CONFIG_BYTES {
            return Err(DshDriverError::ConfigInvalid);
        }
        let file: DshSessionConfigFile =
            serde_json::from_slice(bytes).map_err(|_| DshDriverError::ConfigInvalid)?;
        Self::from_file(file, executable_policy, canonical_root)
    }

    fn from_file(
        file: DshSessionConfigFile,
        executable_policy: ExecutablePolicy,
        canonical_root: &Path,
    ) -> Result<Self, DshDriverError> {
        if file.schema_version != DSH_SESSION_CONFIG_SCHEMA_VERSION {
            return Err(DshDriverError::ConfigInvalid);
        }
        let profile = file
            .profile
            .clone()
            .unwrap_or_else(|| DEFAULT_DSH_PROFILE.to_string());
        if !profile_is_valid(&profile) {
            return Err(DshDriverError::ConfigInvalid);
        }
        // Shape first, so a malformed path never reaches the filesystem.
        validate_executable(&file.executable, executable_policy)
            .map_err(|_| DshDriverError::ExecutableUntrusted)?;
        if !executable_is_node_image(&file.executable, executable_policy) {
            return Err(DshDriverError::ExecutableUntrusted);
        }
        validate_entry_script_shape(&file.entry_script)
            .map_err(|_| DshDriverError::EntryScriptUntrusted)?;

        let executable = canonical_outside_root(&file.executable, canonical_root)
            .filter(|path| validate_executable(path, executable_policy).is_ok())
            .filter(|path| executable_is_node_image(path, executable_policy))
            .ok_or(DshDriverError::ExecutableUntrusted)?;
        let entry_script = canonical_outside_root(&file.entry_script, canonical_root)
            .filter(|path| validate_entry_script_shape(path).is_ok())
            .filter(|path| dsh_package_owns(path))
            .ok_or(DshDriverError::EntryScriptUntrusted)?;

        let environment_names = file
            .environment_names
            .iter()
            .map(|name| name.to_ascii_uppercase())
            .collect();

        let config = Self {
            executable,
            entry_script,
            profile,
            provider: file.provider,
            model: file.model,
            reasoning_effort: file.reasoning_effort,
            max_tokens: file.max_tokens,
            environment_names,
            limits: file.limits,
        };
        config
            .limits
            .validate()
            .map_err(|_| DshDriverError::ConfigInvalid)?;
        // The settings validator owns the provider/model/effort/token rules,
        // so both layers can never disagree about what is acceptable.
        config
            .settings("vibemux_validation".to_string(), Path::new("validation"))
            .map_err(|_| DshDriverError::ConfigInvalid)?;
        // `build_launch_spec` validates the pair again; doing it here turns a
        // config error into a load-time refusal rather than a spawn-time one.
        config
            .launch(Path::new("validation"))
            .map_err(|_| DshDriverError::ConfigInvalid)?;
        Ok(config)
    }
}

/// Reads and verifies `<state dir>/dsh_session.json`. A missing file is
/// `Ok(None)`: DSH is opt-in and an unconfigured project must not fail.
pub fn load_session_config(
    config_path: &Path,
    canonical_root: &Path,
) -> Result<Option<DshSessionConfig>, DshDriverError> {
    let metadata = match std::fs::symlink_metadata(config_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(DshDriverError::Unconfigured),
    };
    if !metadata.is_file() {
        return Err(DshDriverError::ConfigInvalid);
    }
    let limit = MAX_SESSION_CONFIG_BYTES as u64;
    if metadata.len() > limit {
        return Err(DshDriverError::ConfigInvalid);
    }
    let file = File::open(config_path).map_err(|_| DshDriverError::Unconfigured)?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| DshDriverError::Unconfigured)?;
    if bytes.len() as u64 > limit {
        return Err(DshDriverError::ConfigInvalid);
    }
    DshSessionConfig::parse(&bytes, ExecutablePolicy::for_host(), canonical_root).map(Some)
}

/// Canonicalizes `path` and refuses it when the result is not a regular file
/// outside `canonical_root`.
fn canonical_outside_root(path: &Path, canonical_root: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let regular = std::fs::metadata(&canonical)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false);
    if !regular || canonical.starts_with(canonical_root) {
        return None;
    }
    Some(canonical)
}

/// Whether an installed `@deepseek-ai/dsh` package names `script` in its own
/// `bin` map. This is what makes the launcher pair a pair: the interpreter is
/// generic, so the script's provenance is the trust anchor.
#[must_use]
pub fn dsh_package_owns(script: &Path) -> bool {
    let mut directory = script.parent();
    for _ in 0..MAX_PACKAGE_ANCESTORS {
        let Some(current) = directory else {
            return false;
        };
        if let Some(owned) = read_package_bin(current, script) {
            return owned;
        }
        directory = current.parent();
    }
    false
}

/// `Some(true)` when the manifest at `package_root` declares `script` as its
/// `dsh` bin entry; `Some(false)` when a manifest exists but does not;
/// `None` when there is no readable manifest at that level.
fn read_package_bin(package_root: &Path, script: &Path) -> Option<bool> {
    let manifest_path = package_root.join("package.json");
    let metadata = std::fs::symlink_metadata(&manifest_path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SESSION_CONFIG_BYTES as u64 {
        return Some(false);
    }
    let mut encoded = String::new();
    File::open(&manifest_path)
        .ok()?
        .take(MAX_SESSION_CONFIG_BYTES as u64)
        .read_to_string(&mut encoded)
        .ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&encoded).ok()?;
    if manifest.get("name").and_then(serde_json::Value::as_str) != Some(DSH_ENTRY_PACKAGE_NAME) {
        return Some(false);
    }
    let bin = manifest.get("bin")?;
    let declared: Option<&str> = match bin {
        serde_json::Value::String(value) => Some(value.as_str()),
        serde_json::Value::Object(map) => map
            .get(DSH_ENTRY_BIN_NAME)
            .and_then(serde_json::Value::as_str),
        _ => None,
    };
    let declared = declared?;
    // Resolve against the package root before comparing, and compare with
    // `==` on canonical paths rather than by string prefix (AGENTS.md §12.3).
    let joined = package_root.join(declared);
    let resolved = std::fs::canonicalize(&joined).unwrap_or(joined);
    Some(resolved == script)
}

/// The content-free projection a status view may print.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshSessionConfigSummary {
    pub profile: String,
    pub provider: String,
    pub model: String,
    pub has_reasoning_effort: bool,
    pub max_tokens: Option<u64>,
    pub environment_name_count: usize,
    pub limits: DshSessionLimits,
}

impl From<&DshSessionConfig> for DshSessionConfigSummary {
    fn from(config: &DshSessionConfig) -> Self {
        Self {
            profile: config.profile.clone(),
            provider: config.provider.clone(),
            model: config.model.clone(),
            has_reasoning_effort: config.reasoning_effort.is_some(),
            max_tokens: config.max_tokens,
            environment_name_count: config.environment_names.len(),
            limits: config.limits,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const POLICY: ExecutablePolicy = ExecutablePolicy::WindowsNativeImage;

    /// Scratch space lives under the workspace `target/` tree. The platform
    /// temporary directory is not writable in every sandbox this project runs
    /// in, and `target/` is already ignored by Git, so cleanup stays automatic
    /// and no test writes outside the checkout.
    fn scratch() -> tempfile::TempDir {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch");
        std::fs::create_dir_all(&root).expect("scratch root");
        tempfile::Builder::new()
            .prefix("dsh-")
            .tempdir_in(&root)
            .expect("scratch dir")
    }

    fn absolute(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\tools\{name}"))
        } else {
            PathBuf::from(format!("/opt/tools/{name}"))
        }
    }

    fn document() -> serde_json::Value {
        json!({
            "schema_version": 1,
            "executable": absolute("node.exe").to_string_lossy(),
            "entry_script": absolute("dsh/lib/bin.js").to_string_lossy(),
            "profile": "sdk",
            "provider": "deepseek-official",
            "model": "deepseek-v4-flash",
            "environment_names": ["deepseek_api_key"],
        })
    }

    fn write_dsh_package(root: &Path) -> PathBuf {
        let package = root.join("node_modules/@deepseek-ai/dsh");
        std::fs::create_dir_all(package.join("lib")).expect("package dir");
        std::fs::write(
            package.join("package.json"),
            json!({"name": DSH_ENTRY_PACKAGE_NAME, "bin": {"dsh": "lib/bin.js"}}).to_string(),
        )
        .expect("manifest");
        let script = package.join("lib/bin.js");
        std::fs::write(&script, "// fixture").expect("script");
        script
    }

    #[test]
    fn a_missing_configuration_is_opt_out_not_an_error() {
        let directory = scratch();
        let missing = directory.path().join(DSH_SESSION_CONFIG_FILE_NAME);
        assert!(
            matches!(load_session_config(&missing, directory.path()), Ok(None)),
            "an unconfigured project must not fail"
        );
    }

    #[test]
    fn the_on_disk_shape_is_strict_and_versioned() {
        let directory = scratch();
        let root = directory.path();
        let script = write_dsh_package(root);
        let node = root.join("node.exe");
        std::fs::write(&node, "binary").expect("node image");

        let mut valid = document();
        valid["executable"] = json!(node.to_string_lossy());
        valid["entry_script"] = json!(script.to_string_lossy());
        let config = DshSessionConfig::parse(
            serde_json::to_vec(&valid).expect("bytes").as_slice(),
            ExecutablePolicy::PosixExecutable,
            &root.join("project"),
        )
        .expect("valid configuration");
        assert_eq!(config.profile, "sdk");
        assert_eq!(config.provider, "deepseek-official");
        // Names are normalized to upper snake case for the environment list.
        assert_eq!(config.environment_names, ["DEEPSEEK_API_KEY"]);
        // The projection carries no path.
        let encoded = serde_json::to_string(&DshSessionConfigSummary::from(&config)).expect("json");
        assert!(!encoded.contains("node.exe"));
        assert!(!encoded.contains("bin.js"));

        for (field, value) in [
            ("schema_version", json!(2)),
            ("profile", json!("Bad Profile")),
            ("provider", json!("")),
            ("model", json!("-flag")),
            ("reasoning_effort", json!("has space")),
            ("max_tokens", json!(0)),
            ("entry_script", json!("relative/lib/bin.js")),
            ("executable", json!(absolute("dsh.exe").to_string_lossy())),
            ("unknown_field", json!(1)),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(
                DshSessionConfig::parse(
                    serde_json::to_vec(&invalid).expect("bytes").as_slice(),
                    ExecutablePolicy::PosixExecutable,
                    &root.join("project"),
                )
                .is_err(),
                "{field} must be refused"
            );
        }
        assert_eq!(POLICY, ExecutablePolicy::WindowsNativeImage);
    }

    #[test]
    fn the_entry_script_must_be_owned_by_the_dsh_package() {
        let directory = scratch();
        let root = directory.path();
        let script = write_dsh_package(root);
        assert!(dsh_package_owns(
            &std::fs::canonicalize(&script).expect("canonical")
        ));

        // A script outside the package is refused even when it is a real file.
        let stray = root.join("elsewhere/lib/bin.js");
        std::fs::create_dir_all(stray.parent().expect("parent")).expect("dirs");
        std::fs::write(&stray, "// stray").expect("script");
        let stray = std::fs::canonicalize(&stray).expect("canonical");
        assert!(!dsh_package_owns(&stray));

        // A neighbouring package that is not DSH does not confer ownership.
        let other = root.join("node_modules/@other/tool");
        std::fs::create_dir_all(other.join("lib")).expect("dirs");
        std::fs::write(
            other.join("package.json"),
            json!({"name": "@other/tool", "bin": {"dsh": "lib/bin.js"}}).to_string(),
        )
        .expect("manifest");
        let other_script = other.join("lib/bin.js");
        std::fs::write(&other_script, "// other").expect("script");
        assert!(!dsh_package_owns(
            &std::fs::canonicalize(&other_script).expect("canonical")
        ));

        // A DSH manifest that names a different bin entry is refused.
        let package = root.join("node_modules/@deepseek-ai/dsh");
        std::fs::write(
            package.join("package.json"),
            json!({"name": DSH_ENTRY_PACKAGE_NAME, "bin": {"dsh": "lib/other.js"}}).to_string(),
        )
        .expect("manifest");
        assert!(!dsh_package_owns(
            &std::fs::canonicalize(&script).expect("canonical")
        ));
    }

    #[test]
    fn a_target_inside_the_project_root_is_refused() {
        let directory = scratch();
        let root = directory.path();
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("project");
        let script = write_dsh_package(&project);
        let node = project.join("node.exe");
        std::fs::write(&node, "binary").expect("node image");
        let canonical_project = std::fs::canonicalize(&project).expect("canonical project");

        let mut document = document();
        document["executable"] = json!(node.to_string_lossy());
        document["entry_script"] = json!(script.to_string_lossy());
        // A repository cannot ship the code a session runs.
        assert!(
            DshSessionConfig::parse(
                serde_json::to_vec(&document).expect("bytes").as_slice(),
                ExecutablePolicy::PosixExecutable,
                &canonical_project,
            )
            .is_err()
        );
        // The same pair outside the project root is accepted.
        DshSessionConfig::parse(
            serde_json::to_vec(&document).expect("bytes").as_slice(),
            ExecutablePolicy::PosixExecutable,
            root,
        )
        .expect("outside the project root");
    }

    #[test]
    fn launch_argv_is_the_node_pair_and_settings_carry_no_prompt() {
        let directory = scratch();
        let root = directory.path();
        let script = write_dsh_package(root);
        let node = root.join("node.exe");
        std::fs::write(&node, "binary").expect("node image");
        let mut document = document();
        document["executable"] = json!(node.to_string_lossy());
        document["entry_script"] = json!(script.to_string_lossy());
        let config = DshSessionConfig::parse(
            serde_json::to_vec(&document).expect("bytes").as_slice(),
            ExecutablePolicy::PosixExecutable,
            root,
        )
        .expect("configuration");

        let launch = config.launch(&root.join("project")).expect("launch");
        assert_eq!(
            launch.arguments[1..],
            ["--profile".to_string(), "sdk".to_string()]
        );
        assert!(launch.arguments[0].ends_with("bin.js"));
        assert_eq!(
            launch.environment_names,
            ["SYSTEMROOT", "WINDIR", "TEMP", "TMP", "DEEPSEEK_API_KEY"]
        );

        let settings = config
            .settings("vibemux_1".to_string(), &root.join("project"))
            .expect("settings");
        assert_eq!(settings.provider, "deepseek-official");
        assert!(settings.working_directory.ends_with("project"));
        // The redacted Debug view exposes no path.
        let debug = format!("{config:?}");
        assert!(!debug.contains("node.exe"));
        assert!(!debug.contains("bin.js"));
    }
}
