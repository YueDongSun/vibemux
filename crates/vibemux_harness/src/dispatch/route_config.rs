//! Trusted operator route config for harness dispatch (ADR 029 §4).
//!
//! The daemon reads `.vibemux/harness_dispatch.json` once, hands the exact
//! bytes to [`DispatchConfig::parse`], and pins the returned digest for its
//! lifetime. The config names the vendor executable, an optional model, and
//! the environment-variable names the vendor may inherit. It carries no
//! operator argv: every argument comes from the protocol profile in
//! [`super::launch_spec`], because the vendor CLIs expose too many
//! permission, sandbox, working-directory, network, and config-source flags
//! (including clustered short flags) for a denylist to stay complete. It is
//! deserialize-only: paths and environment names never leave the daemon
//! through serialization; [`DispatchConfig::catalog`] is the only public
//! projection.

use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::AgentKind;

use super::{DispatchError, DispatchRequest, Sha256Digest};

pub const ROUTE_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const MAX_ROUTE_CONFIG_BYTES: usize = 64 * 1024;
pub const MAX_ROUTES: usize = 16;
pub const MAX_ENVIRONMENT_NAMES: usize = 24;
pub const MAX_ENVIRONMENT_NAME_BYTES: usize = 128;
pub const MAX_EXECUTABLE_PATH_BYTES: usize = 1024;
pub const MAX_MODEL_BYTES: usize = 256;
pub const MAX_ENTRY_SCRIPT_BYTES: usize = 1024;
pub const MAX_PROFILE_BYTES: usize = 64;

/// The DSH profile a route boots when it does not name one. `dsh --profile sdk`
/// serves "DeepSeek Harness SDK clients over stdio JSON-RPC" (ADR 032).
pub const DEFAULT_DSH_PROFILE: &str = "sdk";
/// File stem the launcher image of a DSH route must carry. DSH's vendor
/// runtime is a Node interpreter, so the dispatch rule "the executable's stem
/// is the harness command name" cannot apply verbatim (ADR 032 Decision 3).
pub const DSH_NODE_IMAGE_STEM: &str = "node";
/// Package that must own a DSH route's entry script.
pub const DSH_ENTRY_PACKAGE_NAME: &str = "@deepseek-ai/dsh";
/// `bin` key the owning package must map to the entry script.
pub const DSH_ENTRY_BIN_NAME: &str = "dsh";

pub const DEFAULT_FRAME_BYTES: usize = 128 * 1024;
pub const MIN_FRAME_BYTES: usize = 64;
pub const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CAPTURE_RECORDS: usize = 16_384;
pub const DEFAULT_DEADLINE_MS: u64 = 600_000;
pub const MIN_DEADLINE_MS: u64 = 1_000;
pub const MAX_DEADLINE_MS: u64 = 3_600_000;
pub const DEFAULT_SHUTDOWN_GRACE_MS: u64 = 2_000;
pub const MIN_SHUTDOWN_GRACE_MS: u64 = 100;
pub const MAX_SHUTDOWN_GRACE_MS: u64 = 10_000;

/// Environment names reserved for VibeMux itself; a route may not forward them.
const RESERVED_ENVIRONMENT_PREFIX: &str = "VIBEMUX_";

/// Structured wire protocol for a native route. Only protocols with a
/// verified request/terminal contract are executable in this slice.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeProtocol {
    /// `codex exec --json`: prompt on stdin, JSONL events on stdout.
    CodexExec,
    /// `codex app-server --stdio`: JSON-RPC thread/turn protocol.
    CodexAppServer,
    /// Claude Code `--input-format stream-json --output-format stream-json`.
    ClaudeStreamJson,
    /// Agent Client Protocol v1 over stdio (OpenCode, Copilot, Grok).
    Acp,
    /// DeepSeek Harness SDK runtime: newline-delimited JSON-RPC 2.0 over
    /// stdio, booted as `dsh --profile <profile>` (ADR 032).
    DshSdkJsonRpc,
}

impl NativeProtocol {
    pub const ALL: [Self; 5] = [
        Self::CodexExec,
        Self::CodexAppServer,
        Self::ClaudeStreamJson,
        Self::Acp,
        Self::DshSdkJsonRpc,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodexExec => "codex_exec",
            Self::CodexAppServer => "codex_app_server",
            Self::ClaudeStreamJson => "claude_stream_json",
            Self::Acp => "acp",
            Self::DshSdkJsonRpc => "dsh_sdk_json_rpc",
        }
    }

    /// Whether this protocol is a verified adapter for `harness`.
    #[must_use]
    pub const fn accepts(self, harness: AgentKind) -> bool {
        matches!(
            (self, harness),
            (Self::CodexExec | Self::CodexAppServer, AgentKind::Codex)
                | (Self::ClaudeStreamJson, AgentKind::Claude)
                | (
                    Self::Acp,
                    AgentKind::OpenCode | AgentKind::Copilot | AgentKind::Grok
                )
                | (Self::DshSdkJsonRpc, AgentKind::Dsh)
        )
    }

    /// Whether an initialize-only handshake exists (no prompt is sent).
    /// `DshSdkJsonRpc` qualifies because `initialize` resolves the provider
    /// route and rejects later prompts when it fails (ADR 032 Decision 3).
    #[must_use]
    pub const fn supports_probe(self) -> bool {
        !matches!(self, Self::CodexExec)
    }

    /// Whether the adapter can execute a prompt turn. Admission paths also
    /// require their permission profile. ACP vendors grant
    /// their own tool permissions (OpenCode allows edit and bash by
    /// default) and no verified per-vendor deny posture exists yet, so ACP
    /// routes are probe-only (ADR 029).
    #[must_use]
    pub const fn supports_execution(self) -> bool {
        !matches!(self, Self::Acp)
    }

    /// ADR 029 has a read-only launch posture for these protocols. DSH's
    /// SDK profile is session execution, not a certified read-only profile.
    #[must_use]
    pub const fn supports_read_only_dispatch(self) -> bool {
        matches!(
            self,
            Self::CodexExec | Self::CodexAppServer | Self::ClaudeStreamJson
        )
    }

    /// Whether this protocol requires the Node launcher pair of ADR 032
    /// Decision 3 instead of the plain "stem is the harness command" rule.
    #[must_use]
    pub const fn requires_launcher_pair(self) -> bool {
        matches!(self, Self::DshSdkJsonRpc)
    }
}

/// Executable rule for the host the daemon runs on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutablePolicy {
    /// Windows: a `.exe` image; `.cmd`, `.bat`, and script shims are rejected
    /// because they would route argv through a command interpreter.
    WindowsNativeImage,
    /// POSIX: any absolute path; interpreter shim extensions are rejected.
    PosixExecutable,
}

impl ExecutablePolicy {
    #[must_use]
    pub const fn for_host() -> Self {
        if cfg!(windows) {
            Self::WindowsNativeImage
        } else {
            Self::PosixExecutable
        }
    }
}

/// Bounded capture and timing limits. Omitted fields take the defaults.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DispatchLimits {
    pub frame_bytes: usize,
    pub capture_bytes: usize,
    pub record_count: usize,
    pub deadline_ms: u64,
    pub shutdown_grace_ms: u64,
}

impl Default for DispatchLimits {
    fn default() -> Self {
        Self {
            frame_bytes: DEFAULT_FRAME_BYTES,
            capture_bytes: MAX_CAPTURE_BYTES,
            record_count: MAX_CAPTURE_RECORDS,
            deadline_ms: DEFAULT_DEADLINE_MS,
            shutdown_grace_ms: DEFAULT_SHUTDOWN_GRACE_MS,
        }
    }
}

impl DispatchLimits {
    pub fn validate(&self) -> Result<(), DispatchError> {
        let valid = (MIN_FRAME_BYTES..=DEFAULT_FRAME_BYTES).contains(&self.frame_bytes)
            && (self.frame_bytes..=MAX_CAPTURE_BYTES).contains(&self.capture_bytes)
            && (1..=MAX_CAPTURE_RECORDS).contains(&self.record_count)
            && (MIN_DEADLINE_MS..=MAX_DEADLINE_MS).contains(&self.deadline_ms)
            && (MIN_SHUTDOWN_GRACE_MS..=MAX_SHUTDOWN_GRACE_MS).contains(&self.shutdown_grace_ms);
        if valid {
            Ok(())
        } else {
            Err(DispatchError::ConfigInvalid)
        }
    }
}

/// One operator-configured native adapter route.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchRoute {
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub executable: PathBuf,
    /// Node entry script of a launcher-pair protocol (`DshSdkJsonRpc` only).
    /// The daemon canonicalizes it and verifies its owning package before the
    /// route is trusted (ADR 032 Decision 3).
    #[serde(default)]
    pub entry_script: Option<PathBuf>,
    /// Profile the launcher boots. Only a launcher-pair protocol may set it.
    #[serde(default)]
    pub profile: Option<String>,
    /// Provider route an SDK session runs on. `InitializeParams.provider` is
    /// required by the DSH SDK protocol, so a launcher-pair route must set it.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub environment_names: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allow_execution: bool,
    #[serde(default)]
    pub model: Option<String>,
}

impl DispatchRoute {
    /// The profile this route boots, defaulting to [`DEFAULT_DSH_PROFILE`].
    #[must_use]
    pub fn effective_profile(&self) -> &str {
        self.profile.as_deref().unwrap_or(DEFAULT_DSH_PROFILE)
    }
}

impl fmt::Debug for DispatchRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DispatchRoute")
            .field("harness", &self.harness)
            .field("protocol", &self.protocol)
            .field("enabled", &self.enabled)
            .field("allow_execution", &self.allow_execution)
            .field("launcher_pair", &self.entry_script.is_some())
            .field("environment_name_count", &self.environment_names.len())
            .finish_non_exhaustive()
    }
}

/// Public, content-free projection of one route.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchCatalogEntry {
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub enabled: bool,
    pub allow_execution: bool,
    pub probe_supported: bool,
}

/// Validated operator route config.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub limits: DispatchLimits,
    pub routes: Vec<DispatchRoute>,
}

/// A validated config together with the digest of the exact bytes parsed.
#[derive(Clone, Debug)]
pub struct PinnedDispatchConfig {
    pub config: DispatchConfig,
    pub digest: Sha256Digest,
}

impl DispatchConfig {
    /// Parses and validates the exact config bytes. The digest covers the same
    /// bytes, so the pinned identity cannot drift from the validated content.
    pub fn parse(
        bytes: &[u8],
        policy: ExecutablePolicy,
    ) -> Result<PinnedDispatchConfig, DispatchError> {
        if bytes.len() > MAX_ROUTE_CONFIG_BYTES {
            return Err(DispatchError::ConfigTooLarge);
        }
        let config: Self =
            serde_json::from_slice(bytes).map_err(|_| DispatchError::ConfigInvalid)?;
        config.validate(policy)?;
        Ok(PinnedDispatchConfig {
            config,
            digest: Sha256Digest::of(bytes),
        })
    }

    pub fn validate(&self, policy: ExecutablePolicy) -> Result<(), DispatchError> {
        if self.schema_version != ROUTE_CONFIG_SCHEMA_VERSION
            || self.routes.is_empty()
            || self.routes.len() > MAX_ROUTES
        {
            return Err(DispatchError::ConfigInvalid);
        }
        self.limits.validate()?;
        let mut harnesses = BTreeSet::new();
        for route in &self.routes {
            if !harnesses.insert(route.harness) {
                return Err(DispatchError::ConfigRouteInvalid);
            }
            validate_route(route, policy)?;
        }
        Ok(())
    }

    /// The enabled route for `harness`, or `RouteUnavailable`.
    pub fn route(&self, harness: AgentKind) -> Result<&DispatchRoute, DispatchError> {
        self.routes
            .iter()
            .find(|route| route.harness == harness && route.enabled)
            .ok_or(DispatchError::RouteUnavailable)
    }

    /// Admission gates for a prompt-bearing request, in order: request
    /// shape, enabled route, operator execution consent, then registry
    /// detection (ADR 013). No state changes before all of them pass.
    pub fn execution_route(
        &self,
        request: &DispatchRequest,
        harness_detected: bool,
    ) -> Result<&DispatchRoute, DispatchError> {
        request.validate()?;
        let route = self.route(request.harness)?;
        if !route.allow_execution || !route.protocol.supports_read_only_dispatch() {
            return Err(DispatchError::ExecutionDisabled);
        }
        if !harness_detected {
            return Err(DispatchError::NotDetected);
        }
        Ok(route)
    }

    /// Gates for an initialize-only probe. A probe sends no prompt, so it
    /// needs an enabled, detected route but not `allow_execution`.
    pub fn probe_route(
        &self,
        harness: AgentKind,
        harness_detected: bool,
    ) -> Result<&DispatchRoute, DispatchError> {
        let route = self.route(harness)?;
        if !route.protocol.supports_probe() {
            return Err(DispatchError::ProbeUnsupported);
        }
        if !harness_detected {
            return Err(DispatchError::NotDetected);
        }
        Ok(route)
    }

    #[must_use]
    pub fn catalog(&self) -> Vec<DispatchCatalogEntry> {
        self.routes
            .iter()
            .map(|route| DispatchCatalogEntry {
                harness: route.harness,
                protocol: route.protocol,
                enabled: route.enabled,
                allow_execution: route.allow_execution
                    && route.protocol.supports_read_only_dispatch(),
                probe_supported: route.protocol.supports_probe(),
            })
            .collect()
    }
}

fn validate_route(route: &DispatchRoute, policy: ExecutablePolicy) -> Result<(), DispatchError> {
    if !route.protocol.accepts(route.harness)
        || (route.allow_execution && !route.protocol.supports_execution())
    {
        return Err(DispatchError::ConfigRouteInvalid);
    }
    validate_executable(&route.executable, policy)?;
    validate_environment_names(&route.environment_names)?;
    // The launcher pair is protocol-conditional in both directions: a
    // launcher-pair protocol must carry a well-shaped entry script and a Node
    // image, and no other protocol may carry either field. Without the second
    // half, a route could smuggle an unused interpreter through the config.
    let paired = route.protocol.requires_launcher_pair();
    match (&route.entry_script, paired) {
        (Some(script), true) => {
            validate_entry_script_shape(script)?;
            if !executable_is_node_image(&route.executable, policy) {
                return Err(DispatchError::ConfigExecutableInvalid);
            }
            if let Some(profile) = &route.profile
                && !profile_is_valid(profile)
            {
                return Err(DispatchError::ConfigRouteInvalid);
            }
            // `initialize` names the provider and model, so both are required
            // and validated here rather than at launch time.
            match &route.provider {
                Some(provider) if is_valid_route_label(provider) => {}
                _ => return Err(DispatchError::ConfigRouteInvalid),
            }
            match &route.model {
                Some(model) if is_valid_route_label(model) => {}
                _ => return Err(DispatchError::ConfigRouteInvalid),
            }
        }
        (None, true) => return Err(DispatchError::ConfigRouteInvalid),
        (Some(_), false) => return Err(DispatchError::ConfigRouteInvalid),
        (None, false) => {
            if route.profile.is_some() || route.provider.is_some() {
                return Err(DispatchError::ConfigRouteInvalid);
            }
        }
    }
    if let Some(model) = &route.model {
        // ACP v1 has no standard model selection; refuse rather than ignore.
        if route.protocol == NativeProtocol::Acp || !is_valid_route_label(model) {
            return Err(DispatchError::ConfigRouteInvalid);
        }
    }
    Ok(())
}

/// Checks the configured path shape. Existence and canonicalization are the
/// daemon's job at load time.
pub fn validate_executable(path: &Path, policy: ExecutablePolicy) -> Result<(), DispatchError> {
    let Some(text) = path.to_str() else {
        return Err(DispatchError::ConfigExecutableInvalid);
    };
    if text.is_empty()
        || text.len() > MAX_EXECUTABLE_PATH_BYTES
        || text.contains('\0')
        || !path.is_absolute()
    {
        return Err(DispatchError::ConfigExecutableInvalid);
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let shim = matches!(
        extension.as_deref(),
        Some("bat" | "cmd" | "ps1" | "psm1" | "vbs" | "js" | "com")
    );
    let allowed = match policy {
        ExecutablePolicy::WindowsNativeImage => extension.as_deref() == Some("exe"),
        ExecutablePolicy::PosixExecutable => !shim,
    };
    if allowed {
        Ok(())
    } else {
        Err(DispatchError::ConfigExecutableInvalid)
    }
}

/// Checks the shape of a launcher-pair entry script. The owning package is
/// verified by the daemon, which is the only layer allowed to touch the
/// filesystem (ADR 032 Decision 3).
pub fn validate_entry_script_shape(path: &Path) -> Result<(), DispatchError> {
    let Some(text) = path.to_str() else {
        return Err(DispatchError::ConfigRouteInvalid);
    };
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let well_formed = !text.is_empty()
        && text.len() <= MAX_ENTRY_SCRIPT_BYTES
        && !text.contains('\0')
        && path.is_absolute()
        && extension.as_deref() == Some("js")
        && path.file_stem().is_some_and(|stem| !stem.is_empty());
    if well_formed {
        Ok(())
    } else {
        Err(DispatchError::ConfigRouteInvalid)
    }
}

/// Whether `path` carries the launcher-image stem of a launcher-pair protocol.
/// Windows compares ASCII case-insensitively; POSIX is exact.
#[must_use]
pub fn executable_is_node_image(path: &Path, policy: ExecutablePolicy) -> bool {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    match policy {
        ExecutablePolicy::WindowsNativeImage => stem.eq_ignore_ascii_case(DSH_NODE_IMAGE_STEM),
        ExecutablePolicy::PosixExecutable => stem == DSH_NODE_IMAGE_STEM,
    }
}

/// Whether `profile` is a bounded, shell-safe DSH profile name. The value
/// reaches argv, so the alphabet is deliberately narrow.
#[must_use]
pub fn profile_is_valid(profile: &str) -> bool {
    let bytes = profile.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_PROFILE_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

/// Whether the file stem of `path` is the harness's command name (ADR 029
/// §4 executable trust). The daemon applies it to the canonical target so a
/// route cannot point at an interpreter such as `cmd.exe` or `powershell.exe`
/// under another harness's name. Windows compares ASCII case-insensitively.
///
/// A launcher-pair protocol never satisfies this rule, because its executable
/// is the vendor's own interpreter; the daemon uses
/// [`executable_is_node_image`] plus the owning-package check instead.
#[must_use]
pub fn executable_names_harness(path: &Path, harness: AgentKind, policy: ExecutablePolicy) -> bool {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    let command = harness.command_name();
    match policy {
        ExecutablePolicy::WindowsNativeImage => stem.eq_ignore_ascii_case(command),
        ExecutablePolicy::PosixExecutable => stem == command,
    }
}

fn validate_environment_names(names: &[String]) -> Result<(), DispatchError> {
    if names.len() > MAX_ENVIRONMENT_NAMES {
        return Err(DispatchError::ConfigEnvironmentInvalid);
    }
    let mut seen = BTreeSet::new();
    for name in names {
        let bytes = name.as_bytes();
        let well_formed = !bytes.is_empty()
            && bytes.len() <= MAX_ENVIRONMENT_NAME_BYTES
            && !bytes[0].is_ascii_digit()
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_');
        if !well_formed
            || name.starts_with(RESERVED_ENVIRONMENT_PREFIX)
            || !seen.insert(name.as_str())
        {
            return Err(DispatchError::ConfigEnvironmentInvalid);
        }
    }
    Ok(())
}

/// Whether `label` is a bounded, argv-safe route label. Used for the model
/// name and for the DSH provider route; both reach protocol params, and the
/// alphabet is deliberately narrow.
fn is_valid_route_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_MODEL_BYTES
        && !label.starts_with('-')
        && label.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
        })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::*;
    use crate::dispatch::request::DISPATCH_REQUEST_SCHEMA_VERSION;

    const HOST: ExecutablePolicy = ExecutablePolicy::for_host();

    fn host_path(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\tools\{name}"))
        } else {
            PathBuf::from(format!("/opt/tools/{name}"))
        }
    }

    /// An absolute path on this host that is exactly `length` bytes long and
    /// ends in `suffix`.
    fn absolute_of_length(length: usize, suffix: &str) -> PathBuf {
        let prefix = if cfg!(windows) { r"C:\" } else { "/" };
        let filler = "x".repeat(length - prefix.len() - suffix.len());
        PathBuf::from(format!("{prefix}{filler}{suffix}"))
    }

    fn route_value(harness: AgentKind, protocol: NativeProtocol) -> Value {
        let mut value = json!({
            "harness": harness,
            "protocol": protocol,
            "executable": host_path(&format!("{}.exe", harness.command_name())),
            "enabled": true,
            "allow_execution": protocol.supports_execution(),
        });
        if protocol.requires_launcher_pair() {
            value["executable"] = json!(host_path("node.exe"));
            value["entry_script"] = json!(host_path("dsh/bin.js"));
            value["provider"] = json!("synthetic_provider");
            value["model"] = json!("synthetic_model");
        }
        value
    }

    fn with(mut route: Value, key: &str, value: Value) -> Value {
        route[key] = value;
        route
    }

    fn config_bytes(routes: Vec<Value>) -> Vec<u8> {
        serde_json::to_vec(&json!({ "schema_version": 1, "routes": routes })).expect("config")
    }

    fn parse_routes(routes: Vec<Value>) -> Result<DispatchConfig, DispatchError> {
        DispatchConfig::parse(&config_bytes(routes), HOST).map(|pinned| pinned.config)
    }

    fn outcome(routes: Vec<Value>) -> Result<(), DispatchError> {
        parse_routes(routes).map(|_| ())
    }

    fn request(harness: AgentKind) -> DispatchRequest {
        DispatchRequest {
            schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
            request_id: Uuid::from_u128(0x5eed),
            harness,
            prompt: "synthetic prompt".to_owned(),
        }
    }

    #[test]
    fn a_config_holds_at_most_64_kib_and_sixteen_routes() {
        let mut padded = config_bytes(vec![route_value(
            AgentKind::Codex,
            NativeProtocol::CodexExec,
        )]);
        padded.resize(65_536, b' ');
        assert!(DispatchConfig::parse(&padded, HOST).is_ok());
        padded.push(b' ');
        assert_eq!(
            DispatchConfig::parse(&padded, HOST).err(),
            Some(DispatchError::ConfigTooLarge)
        );
        // Sixteen routes pass the count gate and reach the duplicate check.
        let codex = route_value(AgentKind::Codex, NativeProtocol::CodexExec);
        assert_eq!(
            outcome(vec![codex.clone(); 16]),
            Err(DispatchError::ConfigRouteInvalid)
        );
        assert_eq!(outcome(vec![codex; 17]), Err(DispatchError::ConfigInvalid));
    }

    #[test]
    fn the_default_limits_keep_their_values() {
        let limits = DispatchLimits::default();
        assert_eq!(
            limits,
            DispatchLimits {
                frame_bytes: 131_072,
                capture_bytes: 16_777_216,
                record_count: 16_384,
                deadline_ms: 600_000,
                shutdown_grace_ms: 2_000,
            }
        );
        assert_eq!(limits.validate(), Ok(()));
    }

    #[test]
    fn only_the_read_only_protocols_admit_execution_and_only_with_consent() {
        let read_only: Vec<_> = NativeProtocol::ALL
            .into_iter()
            .filter(|protocol| protocol.supports_read_only_dispatch())
            .collect();
        assert_eq!(
            read_only,
            [
                NativeProtocol::CodexExec,
                NativeProtocol::CodexAppServer,
                NativeProtocol::ClaudeStreamJson,
            ]
        );
        let config = parse_routes(vec![
            route_value(AgentKind::Codex, NativeProtocol::CodexAppServer),
            route_value(AgentKind::Claude, NativeProtocol::ClaudeStreamJson),
            route_value(AgentKind::Dsh, NativeProtocol::DshSdkJsonRpc),
        ])
        .expect("config");
        for harness in [AgentKind::Codex, AgentKind::Claude] {
            assert_eq!(
                config
                    .execution_route(&request(harness), true)
                    .map(|route| route.harness),
                Ok(harness)
            );
        }
        assert_eq!(
            config.execution_route(&request(AgentKind::Dsh), true).err(),
            Some(DispatchError::ExecutionDisabled)
        );
        // A read-only protocol still needs the operator's consent.
        let refused = parse_routes(vec![with(
            route_value(AgentKind::Codex, NativeProtocol::CodexExec),
            "allow_execution",
            json!(false),
        )])
        .expect("config");
        assert_eq!(
            refused
                .execution_route(&request(AgentKind::Codex), true)
                .err(),
            Some(DispatchError::ExecutionDisabled)
        );
    }

    #[test]
    fn a_probe_refuses_an_unsupported_protocol_before_detection() {
        let config = parse_routes(vec![
            route_value(AgentKind::Codex, NativeProtocol::CodexExec),
            with(
                route_value(AgentKind::Claude, NativeProtocol::ClaudeStreamJson),
                "enabled",
                json!(false),
            ),
        ])
        .expect("config");
        assert_eq!(
            config.probe_route(AgentKind::Codex, false).err(),
            Some(DispatchError::ProbeUnsupported)
        );
        let enabled: Vec<_> = config.catalog().iter().map(|entry| entry.enabled).collect();
        assert_eq!(enabled, [true, false]);
    }

    #[test]
    fn the_host_policy_matches_the_platform() {
        let extensionless = validate_executable(&host_path("codex"), ExecutablePolicy::for_host());
        if cfg!(windows) {
            assert_eq!(
                ExecutablePolicy::for_host(),
                ExecutablePolicy::WindowsNativeImage
            );
            assert_eq!(extensionless, Err(DispatchError::ConfigExecutableInvalid));
        } else {
            assert_eq!(
                ExecutablePolicy::for_host(),
                ExecutablePolicy::PosixExecutable
            );
            assert_eq!(extensionless, Ok(()));
        }
    }

    #[test]
    fn route_debug_shows_only_the_shape_and_counts() {
        let config = parse_routes(vec![
            with(
                route_value(AgentKind::Dsh, NativeProtocol::DshSdkJsonRpc),
                "environment_names",
                json!(["SYNTHETIC_KEY"]),
            ),
            route_value(AgentKind::Codex, NativeProtocol::CodexExec),
        ])
        .expect("config");
        assert_eq!(
            format!("{:?}", config.routes[0]),
            "DispatchRoute { harness: Dsh, protocol: DshSdkJsonRpc, enabled: true, \
             allow_execution: true, launcher_pair: true, environment_name_count: 1, .. }"
        );
        assert_eq!(
            format!("{:?}", config.routes[1]),
            "DispatchRoute { harness: Codex, protocol: CodexExec, enabled: true, \
             allow_execution: true, launcher_pair: false, environment_name_count: 0, .. }"
        );
    }

    #[test]
    fn a_launcher_pair_route_needs_a_valid_provider_and_a_model() {
        let dsh = route_value(AgentKind::Dsh, NativeProtocol::DshSdkJsonRpc);
        assert_eq!(outcome(vec![dsh.clone()]), Ok(()));
        for provider in [json!("-p"), json!("a b"), json!("")] {
            assert_eq!(
                outcome(vec![with(dsh.clone(), "provider", provider.clone())]),
                Err(DispatchError::ConfigRouteInvalid),
                "{provider}"
            );
        }
        let mut without_model = dsh;
        without_model
            .as_object_mut()
            .expect("route object")
            .remove("model");
        assert_eq!(
            outcome(vec![without_model]),
            Err(DispatchError::ConfigRouteInvalid)
        );
    }

    #[test]
    fn executable_and_entry_script_paths_hold_at_most_1024_bytes() {
        assert_eq!(
            validate_executable(&absolute_of_length(1024, ".exe"), HOST),
            Ok(())
        );
        assert_eq!(
            validate_executable(&absolute_of_length(1025, ".exe"), HOST),
            Err(DispatchError::ConfigExecutableInvalid)
        );
        assert_eq!(
            validate_entry_script_shape(&absolute_of_length(1024, ".js")),
            Ok(())
        );
        assert_eq!(
            validate_entry_script_shape(&absolute_of_length(1025, ".js")),
            Err(DispatchError::ConfigRouteInvalid)
        );
    }

    #[test]
    fn an_entry_script_refuses_nul_and_folds_the_extension_case() {
        assert_eq!(
            validate_entry_script_shape(&host_path("dsh\0bin.js")),
            Err(DispatchError::ConfigRouteInvalid)
        );
        assert_eq!(validate_entry_script_shape(&host_path("BIN.JS")), Ok(()));
    }

    #[test]
    fn launcher_and_harness_stems_fold_case_only_on_windows() {
        let windows = ExecutablePolicy::WindowsNativeImage;
        let posix = ExecutablePolicy::PosixExecutable;
        assert!(executable_is_node_image(Path::new("NODE.EXE"), windows));
        assert!(executable_is_node_image(Path::new("node"), posix));
        assert!(!executable_is_node_image(Path::new("Node"), posix));
        // A path without a file stem names no image at all.
        for policy in [windows, posix] {
            assert!(!executable_is_node_image(Path::new(".."), policy));
            assert!(!executable_names_harness(
                Path::new(".."),
                AgentKind::Codex,
                policy
            ));
        }
    }

    #[test]
    fn profiles_are_bounded_lowercase_names() {
        assert!(profile_is_valid(&"a".repeat(64)));
        assert!(!profile_is_valid(&"a".repeat(65)));
        for valid in ["sdk", "sdk_2", "sdk-v2"] {
            assert!(profile_is_valid(valid), "{valid}");
        }
        for invalid in ["1sdk", "sDk", "s.dk"] {
            assert!(!profile_is_valid(invalid), "{invalid}");
        }
    }

    #[test]
    fn environment_names_and_route_labels_reach_their_exact_bounds() {
        let names: Vec<String> = (0..24).map(|index| format!("NAME_{index}")).collect();
        assert_eq!(validate_environment_names(&names), Ok(()));
        assert_eq!(validate_environment_names(&["X".repeat(128)]), Ok(()));
        assert!(is_valid_route_label(&"m".repeat(256)));
        assert!(!is_valid_route_label(&"m".repeat(257)));
    }
}
