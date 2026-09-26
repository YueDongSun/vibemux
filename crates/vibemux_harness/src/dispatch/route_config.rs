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
}

impl NativeProtocol {
    pub const ALL: [Self; 4] = [
        Self::CodexExec,
        Self::CodexAppServer,
        Self::ClaudeStreamJson,
        Self::Acp,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodexExec => "codex_exec",
            Self::CodexAppServer => "codex_app_server",
            Self::ClaudeStreamJson => "claude_stream_json",
            Self::Acp => "acp",
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
        )
    }

    /// Whether an initialize-only handshake exists (no prompt is sent).
    #[must_use]
    pub const fn supports_probe(self) -> bool {
        !matches!(self, Self::CodexExec)
    }

    /// Whether prompt execution is allowed in this slice. ACP vendors grant
    /// their own tool permissions (OpenCode allows edit and bash by
    /// default) and no verified per-vendor deny posture exists yet, so ACP
    /// routes are probe-only (ADR 029).
    #[must_use]
    pub const fn supports_execution(self) -> bool {
        !matches!(self, Self::Acp)
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
    #[serde(default)]
    pub environment_names: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allow_execution: bool,
    #[serde(default)]
    pub model: Option<String>,
}

impl fmt::Debug for DispatchRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DispatchRoute")
            .field("harness", &self.harness)
            .field("protocol", &self.protocol)
            .field("enabled", &self.enabled)
            .field("allow_execution", &self.allow_execution)
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
        if !route.allow_execution || !route.protocol.supports_execution() {
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
                allow_execution: route.allow_execution,
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
    if let Some(model) = &route.model {
        // ACP v1 has no standard model selection; refuse rather than ignore.
        if route.protocol == NativeProtocol::Acp || !is_valid_model(model) {
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

fn is_valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= MAX_MODEL_BYTES
        && !model.starts_with('-')
        && model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
        })
}
