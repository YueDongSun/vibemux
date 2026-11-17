//! Turns trusted probe rows into a schema-1 dispatch route config (ADR 029).
//!
//! The probe is the only component that resolves and verifies a harness
//! launcher; the daemon never re-probes. Provisioning therefore takes plain
//! values — agent kind, verified launcher state, resolved executable — and
//! applies the same per-route shape rules [`super::DispatchConfig`] enforces,
//! so a generated config parses and validates. The daemon re-applies its own
//! filesystem trust checks when it loads the file; provisioning runs them
//! first so it never plans a route the daemon would refuse.
//!
//! Only harnesses with an *executing* structured adapter are provisioned.
//! ACP vendors stay out of reach until a verified per-vendor deny posture
//! exists, and a DSH route needs the explicit Node launcher pair (entry
//! script, provider, model) that no probe row carries; both are reported as
//! skips rather than guessed at.
//!
//! Provisioning applies the daemon's executable trust rule to the launcher
//! the probe actually resolved, which is the one place in this crate that
//! reads the filesystem: the path is canonicalized, the target must be a
//! regular file outside the project root, the target must be a directly
//! launchable image, and the harness name must appear in the launch or
//! canonical file stem. A standard versioned install such as
//! `claude -> versions/2.1.326` therefore emits the launch path (the
//! symlink whose stem is `claude`), which the daemon trusts; a `.js` shim
//! behind a symlink has no direct-exec form and is skipped as
//! [`ProvisionSkipReason::NoExecutable`]. Reading the probe cache, writing
//! the config file, and asking the operator to confirm paid turns still
//! belong to the CLI.

use std::path::{Path, PathBuf};

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
    pub executable: Option<PathBuf>,
}

impl ProvisionProbeRow {
    #[must_use]
    pub fn new(
        harness: AgentKind,
        launcher_state: ProbeState,
        executable: Option<PathBuf>,
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
    /// A verified row carried no resolved executable path, or the resolved
    /// launcher has no direct-exec form (for example a `.js` shim that needs
    /// an interpreter the route schema cannot express).
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
/// order rows arrive in. `canonical_project_root` is the already-canonicalized
/// project root the executable trust rule compares against. A duplicate row
/// for one harness resolves last-wins, matching the daemon's row resolution.
#[must_use]
pub fn plan_provisioning(
    rows: &[ProvisionProbeRow],
    canonical_project_root: &Path,
    allow_execution: bool,
    policy: ExecutablePolicy,
) -> ProvisionPlan {
    let mut routes = Vec::new();
    let mut skipped = Vec::new();
    for harness in AgentKind::all() {
        match route_for(
            harness,
            rows,
            canonical_project_root,
            allow_execution,
            policy,
        ) {
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
    canonical_project_root: &Path,
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
    // Last row wins, so a report that carries a stale row and a refreshed one
    // for the same harness resolves the way the daemon would.
    let row = rows
        .iter()
        .rfind(|row| row.harness == harness)
        .ok_or(ProvisionSkipReason::NotProbed)?;
    if row.launcher_state != ProbeState::Verified {
        return Err(ProvisionSkipReason::NotVerified);
    }
    let launch = row
        .executable
        .as_deref()
        .ok_or(ProvisionSkipReason::NoExecutable)?;
    // The launch form must satisfy the route shape rules; the daemon applies
    // the same check when it reads the written file.
    validate_executable(launch, policy).map_err(|_| ProvisionSkipReason::ExecutableInvalid)?;
    // A missing or moved launcher fails here instead of producing a config
    // the daemon would refuse at startup.
    let canonical =
        std::fs::canonicalize(launch).map_err(|_| ProvisionSkipReason::ExecutableInvalid)?;
    let regular = std::fs::metadata(&canonical)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false);
    if !regular || canonical.starts_with(canonical_project_root) {
        return Err(ProvisionSkipReason::ExecutableInvalid);
    }
    // The daemon launches the canonical target, so a script shim such as
    // `codex -> codex.js` has no direct-exec form this schema can express.
    if validate_executable(&canonical, policy).is_err() {
        return Err(ProvisionSkipReason::NoExecutable);
    }
    // A standard versioned install resolves to a file that does not name the
    // harness (`claude -> versions/2.1.326`). The daemon trusts either stem,
    // so emit the launch path in that case; emitting the canonical versioned
    // file would be refused when the daemon loads the config.
    let canonical_named = executable_names_harness(&canonical, harness, policy);
    if !canonical_named && !executable_names_harness(launch, harness, policy) {
        return Err(ProvisionSkipReason::ExecutableInvalid);
    }
    let emitted = if canonical_named {
        canonical
    } else {
        launch.to_path_buf()
    };
    let executable = emitted
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
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::*;
    use crate::dispatch::route_config::{DispatchConfig, DispatchLimits};

    const HOST: ExecutablePolicy = ExecutablePolicy::for_host();

    /// A project root and a vendor directory that stand outside it.
    struct Fixture {
        vendor: PathBuf,
        project: PathBuf,
        canonical_root: PathBuf,
        _temp: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("temp");
            let project = temp.path().join("project");
            let vendor = temp.path().join("vendor");
            std::fs::create_dir_all(&project).expect("project");
            std::fs::create_dir_all(&vendor).expect("vendor");
            let canonical_root = std::fs::canonicalize(&project).expect("canonical root");
            Self {
                vendor,
                project,
                canonical_root,
                _temp: temp,
            }
        }

        fn install(&self, name: &str) -> PathBuf {
            install_executable(&self.vendor, name)
        }
    }

    fn host_path(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\tools\{name}.exe"))
        } else {
            PathBuf::from(format!("/opt/tools/{name}"))
        }
    }

    /// Writes a regular, executable fixture image under the host suffix rule.
    fn install_executable(directory: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(directory).expect("directory");
        let path = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&path, b"fixture image").expect("write executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("executable mode");
        }
        path
    }

    fn verified(harness: AgentKind, executable: PathBuf) -> ProvisionProbeRow {
        ProvisionProbeRow::new(harness, ProbeState::Verified, Some(executable))
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
        let fixture = Fixture::new();
        let claude = fixture.install("claude");
        let codex = fixture.install("codex");
        let plan = plan_provisioning(
            &[
                verified(AgentKind::Claude, claude.clone()),
                verified(AgentKind::Codex, codex.clone()),
            ],
            &fixture.canonical_root,
            true,
            HOST,
        );
        assert_eq!(
            plan.config.routes,
            [
                ProvisionedRoute {
                    harness: AgentKind::Claude,
                    protocol: NativeProtocol::ClaudeStreamJson,
                    executable: std::fs::canonicalize(&claude)
                        .expect("canonical")
                        .to_string_lossy()
                        .into_owned(),
                    environment_names: default_environment_names().to_vec(),
                    enabled: true,
                    allow_execution: true,
                },
                ProvisionedRoute {
                    harness: AgentKind::Codex,
                    protocol: NativeProtocol::CodexAppServer,
                    executable: std::fs::canonicalize(&codex)
                        .expect("canonical")
                        .to_string_lossy()
                        .into_owned(),
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
        let fixture = Fixture::new();
        let plan = plan_provisioning(
            &[verified(AgentKind::Codex, fixture.install("codex"))],
            &fixture.canonical_root,
            false,
            HOST,
        );
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
        let fixture = Fixture::new();
        let claude = verified(AgentKind::Claude, fixture.install("claude"));
        let codex = verified(AgentKind::Codex, fixture.install("codex"));
        let forward = plan_provisioning(
            &[claude.clone(), codex.clone()],
            &fixture.canonical_root,
            true,
            HOST,
        );
        let reversed = plan_provisioning(&[codex, claude], &fixture.canonical_root, true, HOST);
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
        let fixture = Fixture::new();
        let plan = plan_provisioning(
            &[
                verified(AgentKind::Claude, fixture.install("claude")),
                verified(AgentKind::Codex, fixture.install("codex")),
            ],
            &fixture.canonical_root,
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
        let fixture = Fixture::new();
        let plan = plan_provisioning(
            &[
                verified(AgentKind::OpenCode, host_path("opencode")),
                verified(AgentKind::Dsh, host_path("dsh")),
                ProvisionProbeRow::new(
                    AgentKind::Codex,
                    ProbeState::Failed,
                    Some(fixture.install("codex")),
                ),
                verified(AgentKind::Kimi, host_path("kimi")),
            ],
            &fixture.canonical_root,
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
        let fixture = Fixture::new();
        let missing = ProvisionProbeRow::new(AgentKind::Claude, ProbeState::Verified, None);
        let mismatched = verified(AgentKind::Claude, fixture.install("codex"));
        let relative = verified(AgentKind::Claude, PathBuf::from("claude"));
        for (row, expected) in [
            (missing, ProvisionSkipReason::NoExecutable),
            (mismatched, ProvisionSkipReason::ExecutableInvalid),
            (relative, ProvisionSkipReason::ExecutableInvalid),
        ] {
            let plan = plan_provisioning(&[row], &fixture.canonical_root, true, HOST);
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
    fn a_missing_project_relative_or_non_regular_target_is_executable_invalid() {
        let fixture = Fixture::new();
        let missing = fixture
            .vendor
            .join(format!("claude{}", std::env::consts::EXE_SUFFIX));
        let inside = install_executable(&fixture.project, "claude");
        let directory = fixture
            .vendor
            .join(format!("claude{}", std::env::consts::EXE_SUFFIX));
        std::fs::create_dir_all(&directory).expect("directory target");
        for row in [
            verified(AgentKind::Claude, missing),
            verified(AgentKind::Claude, inside),
            verified(AgentKind::Claude, directory),
        ] {
            let plan = plan_provisioning(&[row], &fixture.canonical_root, true, HOST);
            assert!(plan.config.routes.is_empty());
            assert_eq!(
                plan.skipped
                    .iter()
                    .find(|entry| entry.harness == AgentKind::Claude)
                    .map(|entry| entry.reason),
                Some(ProvisionSkipReason::ExecutableInvalid)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_versioned_symlink_is_emitted_as_its_launch_path() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let versions = fixture.vendor.join("versions");
        let versioned = install_executable(&versions, "2.1.326");
        let bin = fixture.vendor.join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let launch = bin.join("claude");
        symlink(&versioned, &launch).expect("symlink");
        let plan = plan_provisioning(
            &[verified(AgentKind::Claude, launch.clone())],
            &fixture.canonical_root,
            true,
            HOST,
        );
        assert_eq!(
            plan.config.routes[0].executable,
            launch.to_string_lossy().into_owned()
        );
        // The emitted launch path still parses; only the daemon can canonicalize.
        let bytes = serde_json::to_vec(&plan.config).expect("config bytes");
        DispatchConfig::parse(&bytes, HOST).expect("route shape is valid");
    }

    #[cfg(unix)]
    #[test]
    fn an_interpreter_shim_target_is_no_executable() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let script = fixture.vendor.join("codex.js");
        std::fs::write(&script, "#!/usr/bin/env node\n").expect("script");
        let launch = fixture
            .vendor
            .join(format!("codex{}", std::env::consts::EXE_SUFFIX));
        symlink(&script, &launch).expect("symlink");
        let plan = plan_provisioning(
            &[verified(AgentKind::Codex, launch)],
            &fixture.canonical_root,
            true,
            HOST,
        );
        assert!(plan.config.routes.is_empty());
        assert_eq!(
            plan.skipped
                .iter()
                .find(|entry| entry.harness == AgentKind::Codex)
                .map(|entry| entry.reason),
            Some(ProvisionSkipReason::NoExecutable)
        );
    }

    #[test]
    fn duplicate_rows_for_one_harness_resolve_last_wins() {
        let fixture = Fixture::new();
        let stale = install_executable(&fixture.vendor.join("stale"), "codex");
        let fresh = install_executable(&fixture.vendor.join("fresh"), "codex");
        let plan = plan_provisioning(
            &[
                verified(AgentKind::Codex, stale),
                verified(AgentKind::Codex, fresh.clone()),
            ],
            &fixture.canonical_root,
            true,
            HOST,
        );
        assert_eq!(
            plan.config.routes[0].executable,
            std::fs::canonicalize(&fresh)
                .expect("canonical")
                .to_string_lossy()
                .into_owned()
        );
    }

    #[test]
    fn the_serialized_shape_is_stable_and_carries_no_unknown_fields() {
        let fixture = Fixture::new();
        let executable = fixture.install("codex");
        let plan = plan_provisioning(
            &[verified(AgentKind::Codex, executable.clone())],
            &fixture.canonical_root,
            true,
            HOST,
        );
        let text = serde_json::to_string(&plan.config).expect("config text");
        let executable = serde_json::to_string(
            &std::fs::canonicalize(&executable)
                .expect("canonical")
                .to_string_lossy()
                .into_owned(),
        )
        .expect("json path");
        let environment = if cfg!(windows) {
            r#"["APPDATA","LOCALAPPDATA","USERPROFILE"]"#
        } else {
            r#"["HOME"]"#
        };
        let expected = format!(
            r#"{{"schema_version":1,"routes":[{{"harness":"codex","protocol":"codex_app_server","executable":{executable},"environment_names":{environment},"enabled":true,"allow_execution":true}}]}}"#
        );
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
