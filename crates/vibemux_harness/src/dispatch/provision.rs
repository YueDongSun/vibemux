//! Turns trusted probe rows into a schema-1 dispatch route config (ADR 029).
//!
//! The probe is the only component that resolves and verifies a harness
//! launcher; the daemon never re-probes. Provisioning therefore takes plain
//! values — agent kind, verified launcher state, resolved executable — and
//! applies the same per-route shape rules [`super::DispatchConfig`] enforces,
//! so a generated config parses and validates. The daemon still canonicalizes
//! each executable and checks that it lies outside the project when it loads
//! the file; those filesystem checks belong to its loader, not to this module.
//!
//! Only harnesses with an *executing* structured adapter are provisioned.
//! ACP vendors stay out of reach until a verified per-vendor deny posture
//! exists, and a DSH route needs the explicit Node launcher pair (entry
//! script, provider, model) that no probe row carries; both are reported as
//! skips rather than guessed at.
//!
//! No I/O happens here: reading the probe cache, writing the config file, and
//! asking the operator to confirm paid turns belong to the CLI.

use serde::Serialize;

use crate::{AgentKind, ProbeState};

use super::{
    NativeProtocol,
    route_config::{
        ExecutablePolicy, ROUTE_CONFIG_SCHEMA_VERSION, executable_names_harness,
        validate_executable,
    },
};

/// Schema version of the config a plan emits. It must match the version the
/// daemon accepts.
pub const PROVISION_SCHEMA_VERSION: u32 = ROUTE_CONFIG_SCHEMA_VERSION;

/// One probe result, reduced to the fields provisioning reads. `executable`
/// is the resolved launcher path the probe actually verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisionProbeRow {
    pub harness: AgentKind,
    pub launcher_state: ProbeState,
    pub executable: Option<std::path::PathBuf>,
}

impl ProvisionProbeRow {
    #[must_use]
    pub fn new(
        harness: AgentKind,
        launcher_state: ProbeState,
        executable: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            harness,
            launcher_state,
            executable,
        }
    }
}

/// Why a harness produced no route. The wire code is stable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProvisionSkipReason {
    /// No probe row named this harness.
    NotProbed,
    /// The probe did not verify a launcher (failed, unavailable, or not run).
    NotVerified,
    /// No structured adapter exists, or only a probe-only (ACP) one does.
    NoExecutingProtocol,
    /// The adapter needs the Node launcher pair a probe row cannot supply.
    LauncherPairRequired,
    /// A verified row carried no resolved executable path.
    NoExecutable,
    /// The path failed the route config's executable rules.
    ExecutableInvalid,
}

impl ProvisionSkipReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotProbed => "not_probed",
            Self::NotVerified => "not_verified",
            Self::NoExecutingProtocol => "no_executing_protocol",
            Self::LauncherPairRequired => "launcher_pair_required",
            Self::NoExecutable => "no_executable",
            Self::ExecutableInvalid => "executable_invalid",
        }
    }
}

/// One harness that produced no route, in canonical order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProvisionSkipped {
    pub harness: AgentKind,
    pub reason: ProvisionSkipReason,
}

/// One provisioned route. Field order is the serialized order, so the JSON
/// shape is stable across builds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProvisionedRoute {
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub executable: String,
    pub environment_names: Vec<&'static str>,
    pub enabled: bool,
    pub allow_execution: bool,
}

/// A schema-1 dispatch route config, ready to serialize.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProvisionedConfig {
    pub schema_version: u32,
    pub routes: Vec<ProvisionedRoute>,
}

/// The planned config plus one skip record per harness that got no route.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisionPlan {
    pub config: ProvisionedConfig,
    pub skipped: Vec<ProvisionSkipped>,
}

impl ProvisionPlan {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.config.routes.is_empty()
    }

    /// Content-free skip projection for CLI output: harness wire name plus a
    /// stable reason code.
    #[must_use]
    pub fn skipped_json(&self) -> Vec<serde_json::Value> {
        self.skipped
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "harness": entry.harness,
                    "reason": entry.reason.as_str(),
                })
            })
            .collect()
    }
}

/// The vendor-login environment names a provisioned route forwards.
///
/// The daemon starts each vendor with an empty environment plus
/// `SYSTEMROOT`/`WINDIR`/`TEMP`/`TMP`; a missing allowlisted name fails the
/// attempt, so the list stays to variables the host normally sets. Windows
/// vendor CLIs find their login under these three profile variables (the
/// documented ADR 029 example); POSIX harnesses use `HOME`.
#[must_use]
pub const fn default_environment_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["APPDATA", "LOCALAPPDATA", "USERPROFILE"]
    } else {
        &["HOME"]
    }
}

/// The one adapter provisioning selects per harness. Codex gets
/// `codex_app_server` because it supports both `probe` and `submit`; the
/// ACP vendors and the harnesses without an adapter yield `None`.
#[must_use]
pub const fn provisioning_protocol(harness: AgentKind) -> Option<NativeProtocol> {
    match harness {
        AgentKind::Codex => Some(NativeProtocol::CodexAppServer),
        AgentKind::Claude => Some(NativeProtocol::ClaudeStreamJson),
        AgentKind::Dsh => Some(NativeProtocol::DshSdkJsonRpc),
        AgentKind::OpenCode
        | AgentKind::Copilot
        | AgentKind::Grok
        | AgentKind::Qwen
        | AgentKind::Iflow
        | AgentKind::Trae
        | AgentKind::Codebuddy
        | AgentKind::Kimi => None,
    }
}

/// Builds the plan in canonical [`AgentKind::all`] order, independent of the
/// order rows arrive in. A duplicate row for one harness is ignored after the
/// first, because a probe report holds at most one row per harness.
#[must_use]
pub fn plan_provisioning(
    rows: &[ProvisionProbeRow],
    allow_execution: bool,
    policy: ExecutablePolicy,
) -> ProvisionPlan {
    let mut routes = Vec::new();
    let mut skipped = Vec::new();
    for harness in AgentKind::all() {
        match route_for(harness, rows, allow_execution, policy) {
            Ok(route) => routes.push(route),
            Err(reason) => skipped.push(ProvisionSkipped { harness, reason }),
        }
    }
    ProvisionPlan {
        config: ProvisionedConfig {
            schema_version: PROVISION_SCHEMA_VERSION,
            routes,
        },
        skipped,
    }
}

fn route_for(
    harness: AgentKind,
    rows: &[ProvisionProbeRow],
    allow_execution: bool,
    policy: ExecutablePolicy,
) -> Result<ProvisionedRoute, ProvisionSkipReason> {
    let protocol =
        provisioning_protocol(harness).ok_or(ProvisionSkipReason::NoExecutingProtocol)?;
    if !protocol.supports_execution() || !protocol.accepts(harness) {
        return Err(ProvisionSkipReason::NoExecutingProtocol);
    }
    // A launcher-pair protocol cannot be described from a probe row; the
    // operator must supply the entry script, provider, and model by hand.
    if protocol.requires_launcher_pair() {
        return Err(ProvisionSkipReason::LauncherPairRequired);
    }
    let row = rows
        .iter()
        .find(|row| row.harness == harness)
        .ok_or(ProvisionSkipReason::NotProbed)?;
    if row.launcher_state != ProbeState::Verified {
        return Err(ProvisionSkipReason::NotVerified);
    }
    let executable = row
        .executable
        .as_deref()
        .ok_or(ProvisionSkipReason::NoExecutable)?;
    validate_executable(executable, policy).map_err(|_| ProvisionSkipReason::ExecutableInvalid)?;
    if !executable_names_harness(executable, harness, policy) {
        return Err(ProvisionSkipReason::ExecutableInvalid);
    }
    let executable = executable
        .to_str()
        .ok_or(ProvisionSkipReason::ExecutableInvalid)?
        .to_owned();
    Ok(ProvisionedRoute {
        harness,
        protocol,
        executable,
        environment_names: default_environment_names().to_vec(),
        enabled: true,
        allow_execution,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{Value, json};

    use super::*;
    use crate::dispatch::route_config::{DispatchConfig, DispatchLimits};

    const HOST: ExecutablePolicy = ExecutablePolicy::for_host();

    fn host_path(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\tools\{name}.exe"))
        } else {
            PathBuf::from(format!("/opt/tools/{name}"))
        }
    }

    fn verified(harness: AgentKind) -> ProvisionProbeRow {
        ProvisionProbeRow::new(
            harness,
            ProbeState::Verified,
            Some(host_path(harness.command_name())),
        )
    }

    fn reasons(plan: &ProvisionPlan) -> Vec<(AgentKind, ProvisionSkipReason)> {
        plan.skipped
            .iter()
            .map(|entry| (entry.harness, entry.reason))
            .collect()
    }

    fn config_value(plan: &ProvisionPlan) -> Value {
        serde_json::to_value(&plan.config).expect("config serializes")
    }

    #[test]
    fn verified_codex_and_claude_rows_become_executing_routes() {
        let plan = plan_provisioning(
            &[verified(AgentKind::Claude), verified(AgentKind::Codex)],
            true,
            HOST,
        );
        assert_eq!(
            plan.config.routes,
            [
                ProvisionedRoute {
                    harness: AgentKind::Claude,
                    protocol: NativeProtocol::ClaudeStreamJson,
                    executable: host_path("claude").to_string_lossy().into_owned(),
                    environment_names: default_environment_names().to_vec(),
                    enabled: true,
                    allow_execution: true,
                },
                ProvisionedRoute {
                    harness: AgentKind::Codex,
                    protocol: NativeProtocol::CodexAppServer,
                    executable: host_path("codex").to_string_lossy().into_owned(),
                    environment_names: default_environment_names().to_vec(),
                    enabled: true,
                    allow_execution: true,
                },
            ]
        );
        assert!(!plan.is_empty());
        assert_eq!(plan.config.schema_version, 1);
    }

    #[test]
    fn the_allow_execution_parameter_is_written_through() {
        let plan = plan_provisioning(&[verified(AgentKind::Codex)], false, HOST);
        assert!(
            plan.config
                .routes
                .iter()
                .all(|route| !route.allow_execution)
        );
        let value = config_value(&plan);
        assert_eq!(value["routes"][0]["allow_execution"], json!(false));
    }

    #[test]
    fn canonical_order_does_not_depend_on_row_order() {
        let forward = plan_provisioning(
            &[verified(AgentKind::Claude), verified(AgentKind::Codex)],
            true,
            HOST,
        );
        let reversed = plan_provisioning(
            &[verified(AgentKind::Codex), verified(AgentKind::Claude)],
            true,
            HOST,
        );
        assert_eq!(forward, reversed);
        let order: Vec<AgentKind> = forward
            .config
            .routes
            .iter()
            .map(|route| route.harness)
            .collect();
        assert_eq!(order, [AgentKind::Claude, AgentKind::Codex]);
    }

    #[test]
    fn the_generated_config_is_accepted_by_the_route_parser() {
        let plan = plan_provisioning(
            &[verified(AgentKind::Claude), verified(AgentKind::Codex)],
            true,
            HOST,
        );
        let bytes = serde_json::to_vec(&plan.config).expect("config bytes");
        let parsed = DispatchConfig::parse(&bytes, HOST).expect("daemon accepts the config");
        assert_eq!(parsed.config.routes.len(), 2);
        assert_eq!(parsed.config.limits, DispatchLimits::default());
        assert_eq!(
            parsed
                .config
                .catalog()
                .iter()
                .map(|entry| (entry.harness, entry.probe_supported))
                .collect::<Vec<_>>(),
            [(AgentKind::Claude, true), (AgentKind::Codex, true)]
        );
    }

    #[test]
    fn acp_dsh_unverified_and_terminal_only_rows_are_skipped_with_reasons() {
        let plan = plan_provisioning(
            &[
                verified(AgentKind::OpenCode),
                verified(AgentKind::Dsh),
                ProvisionProbeRow::new(
                    AgentKind::Codex,
                    ProbeState::Failed,
                    Some(host_path("codex")),
                ),
                verified(AgentKind::Kimi),
            ],
            true,
            HOST,
        );
        assert!(plan.is_empty());
        assert_eq!(
            reasons(&plan),
            [
                (AgentKind::Claude, ProvisionSkipReason::NotProbed),
                (AgentKind::Codex, ProvisionSkipReason::NotVerified),
                (
                    AgentKind::OpenCode,
                    ProvisionSkipReason::NoExecutingProtocol
                ),
                (AgentKind::Copilot, ProvisionSkipReason::NoExecutingProtocol),
                (AgentKind::Grok, ProvisionSkipReason::NoExecutingProtocol),
                (AgentKind::Qwen, ProvisionSkipReason::NoExecutingProtocol),
                (AgentKind::Iflow, ProvisionSkipReason::NoExecutingProtocol),
                (AgentKind::Trae, ProvisionSkipReason::NoExecutingProtocol),
                (
                    AgentKind::Codebuddy,
                    ProvisionSkipReason::NoExecutingProtocol
                ),
                (AgentKind::Kimi, ProvisionSkipReason::NoExecutingProtocol),
                (AgentKind::Dsh, ProvisionSkipReason::LauncherPairRequired),
            ]
        );
        let skipped = plan.skipped_json();
        assert_eq!(
            skipped[0],
            json!({"harness": "claude", "reason": "not_probed"})
        );
    }

    #[test]
    fn a_verified_row_needs_a_path_and_a_harness_named_executable() {
        let missing = ProvisionProbeRow::new(AgentKind::Claude, ProbeState::Verified, None);
        let mismatched = ProvisionProbeRow::new(
            AgentKind::Claude,
            ProbeState::Verified,
            Some(host_path("codex")),
        );
        let relative = ProvisionProbeRow::new(
            AgentKind::Claude,
            ProbeState::Verified,
            Some(PathBuf::from("claude")),
        );
        for (row, expected) in [
            (missing, ProvisionSkipReason::NoExecutable),
            (mismatched, ProvisionSkipReason::ExecutableInvalid),
            (relative, ProvisionSkipReason::ExecutableInvalid),
        ] {
            let plan = plan_provisioning(&[row], true, HOST);
            assert!(
                plan.config.routes.is_empty(),
                "{expected:?} unexpectedly provisioned"
            );
            assert_eq!(
                plan.skipped
                    .iter()
                    .find(|entry| entry.harness == AgentKind::Claude)
                    .map(|entry| entry.reason),
                Some(expected)
            );
        }
    }

    #[test]
    fn the_serialized_shape_is_stable_and_carries_no_unknown_fields() {
        let plan = plan_provisioning(&[verified(AgentKind::Codex)], true, HOST);
        let text = serde_json::to_string(&plan.config).expect("config text");
        let expected = if cfg!(windows) {
            r#"{"schema_version":1,"routes":[{"harness":"codex","protocol":"codex_app_server","executable":"C:\\tools\\codex.exe","environment_names":["APPDATA","LOCALAPPDATA","USERPROFILE"],"enabled":true,"allow_execution":true}]}"#
        } else {
            r#"{"schema_version":1,"routes":[{"harness":"codex","protocol":"codex_app_server","executable":"/opt/tools/codex","environment_names":["HOME"],"enabled":true,"allow_execution":true}]}"#
        };
        assert_eq!(text, expected);
    }

    #[test]
    fn every_selected_protocol_accepts_its_harness_and_can_execute() {
        for harness in AgentKind::all() {
            let Some(protocol) = provisioning_protocol(harness) else {
                continue;
            };
            assert!(
                protocol.accepts(harness),
                "{} does not accept {harness:?}",
                protocol.as_str()
            );
            assert!(protocol.supports_execution(), "{}", protocol.as_str());
        }
        // Codex and Claude are the two provisionable adapters; Codex's
        // `codex_exec` alternate carries no probe, so the route uses
        // `codex_app_server`.
        assert_eq!(
            AgentKind::all()
                .into_iter()
                .filter(|harness| provisioning_protocol(*harness).is_some())
                .collect::<Vec<_>>(),
            [AgentKind::Claude, AgentKind::Codex, AgentKind::Dsh]
        );
        assert_eq!(PROVISION_SCHEMA_VERSION, 1);
        assert!(default_environment_names().iter().all(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
        }));
    }
}
