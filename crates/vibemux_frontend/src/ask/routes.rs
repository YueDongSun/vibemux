#![forbid(unsafe_code)]
//! Default read-only dispatch routes for the GUI's one-click setup.
//!
//! The plan covers Claude Code (stream-json) and Codex (app-server), the two
//! harnesses ADR 029 can execute, and only where the probe verified a native
//! executable whose name is the harness command and that lies outside the
//! project. The rendered config is checked with the daemon's own parser
//! before it is written, and an existing operator config is never replaced.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use vibemux_harness::{
    AgentKind,
    dispatch::{
        DispatchError, NativeProtocol,
        route_config::{
            DispatchConfig, ExecutablePolicy, ROUTE_CONFIG_SCHEMA_VERSION,
            executable_names_harness, validate_executable,
        },
    },
};
use vibemux_probe::{ProbeReport, ProbeState};

/// File name the daemon reads under `.vibemux`.
pub const DISPATCH_CONFIG_FILE_NAME: &str = "harness_dispatch.json";
/// Environment variables a vendor CLI may need to find its login.
pub const DEFAULT_ENVIRONMENT_NAMES: [&str; 3] = ["USERPROFILE", "APPDATA", "LOCALAPPDATA"];
/// The harnesses the setup offers, with their execution protocol.
pub const ASK_HARNESSES: [(AgentKind, NativeProtocol); 2] = [
    (AgentKind::Claude, NativeProtocol::ClaudeStreamJson),
    (AgentKind::Codex, NativeProtocol::CodexAppServer),
];

/// One route the setup will write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedRoute {
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub executable: PathBuf,
}

/// Why a harness got no route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipReason {
    NotDetected,
    /// The probe found a script shim (such as `claude.cmd`) or a file whose
    /// name is not the harness command; the daemon accepts only the native
    /// executable.
    NotANativeExecutable,
    InsideProject,
}

impl SkipReason {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotDetected => "not detected on this machine",
            Self::NotANativeExecutable => {
                "found only as a script shim; install its native executable to use it here"
            }
            Self::InsideProject => "its executable is inside this project folder",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RoutePlan {
    pub routes: Vec<PlannedRoute>,
    pub skipped: Vec<(AgentKind, SkipReason)>,
}

/// Plans the default routes from a probe report.
#[must_use]
pub fn plan_routes(report: &ProbeReport, project_root: &Path, policy: ExecutablePolicy) -> RoutePlan {
    let mut plan = RoutePlan::default();
    for (harness, protocol) in ASK_HARNESSES {
        let probe = report.agents.iter().find(|probe| probe.agent == harness);
        let path = probe
            .filter(|probe| probe.launcher_state == ProbeState::Verified)
            .and_then(|probe| probe.path.as_deref())
            .map(PathBuf::from);
        let Some(executable) = path else {
            plan.skipped.push((harness, SkipReason::NotDetected));
            continue;
        };
        if validate_executable(&executable, policy).is_err()
            || !executable_names_harness(&executable, harness, policy)
        {
            plan.skipped
                .push((harness, SkipReason::NotANativeExecutable));
            continue;
        }
        if executable.starts_with(project_root) {
            plan.skipped.push((harness, SkipReason::InsideProject));
            continue;
        }
        plan.routes.push(PlannedRoute {
            harness,
            protocol,
            executable,
        });
    }
    plan
}

/// The config document for `routes`: enabled, executing, read-only routes
/// with the default environment names and no model override.
pub fn render_config(routes: &[PlannedRoute]) -> Result<Vec<u8>, serde_json::Error> {
    let routes = routes
        .iter()
        .map(|route| {
            Ok(serde_json::json!({
                "harness": serde_json::to_value(route.harness)?,
                "protocol": serde_json::to_value(route.protocol)?,
                "executable": route.executable,
                "environment_names": DEFAULT_ENVIRONMENT_NAMES,
                "enabled": true,
                "allow_execution": true,
            }))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": ROUTE_CONFIG_SCHEMA_VERSION,
        "routes": routes,
    }))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Checks rendered bytes with the daemon's own parser.
pub fn validate_config(bytes: &[u8], policy: ExecutablePolicy) -> Result<(), DispatchError> {
    DispatchConfig::parse(bytes, policy).map(|_| ())
}

/// Where the daemon reads the config for `project_root`.
#[must_use]
pub fn config_path(project_root: &Path) -> PathBuf {
    project_root.join(".vibemux").join(DISPATCH_CONFIG_FILE_NAME)
}

/// Writes `bytes` as the project's dispatch config. Refuses with
/// `AlreadyExists` when a config is already there, so an operator's file is
/// never replaced. The file appears whole: it is written to a new sibling
/// and renamed into place.
pub fn write_config(project_root: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let path = config_path(project_root);
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a dispatch config already exists",
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let staging = path.with_file_name(format!(
        ".{DISPATCH_CONFIG_FILE_NAME}.{}.{stamp}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a dispatch config appeared while writing",
            ));
        }
        fs::rename(&staging, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result.map(|()| path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_probe::{A2aSelfTestProbe, AgentProbe, GatewayProbe, LauncherKind, RouteKind};

    fn probe(agent: AgentKind, state: ProbeState, path: Option<&Path>) -> AgentProbe {
        AgentProbe {
            agent,
            launcher_state: state,
            authentication_state: ProbeState::NotRun,
            inference_state: ProbeState::NotRun,
            launcher: LauncherKind::Unavailable,
            path: path.map(|path| path.display().to_string()),
            version: None,
            route: RouteKind::Unknown,
            endpoints: vec![],
            code: "test".into(),
        }
    }

    fn report(agents: Vec<AgentProbe>) -> ProbeReport {
        ProbeReport {
            schema_version: 1,
            observed_at_epoch_seconds: 0,
            platform: "test".into(),
            agents,
            gateway: GatewayProbe {
                state: ProbeState::NotRun,
                host: String::new(),
                port: 0,
                tcp_reachable: false,
                health_status: None,
                telemetry_state: ProbeState::NotRun,
                telemetry: vec![],
                code: "test".into(),
            },
            a2a: A2aSelfTestProbe {
                state: ProbeState::NotRun,
                correlation_preserved: false,
                listener_closed: true,
                code: "test".into(),
            },
        }
    }

    fn native(directory: &Path, command: &str) -> PathBuf {
        let name = if cfg!(windows) {
            format!("{command}.exe")
        } else {
            command.to_string()
        };
        directory.join(name)
    }

    #[test]
    fn native_executables_outside_the_project_get_routes() {
        let tools = tempfile::tempdir().expect("tools");
        let project = tempfile::tempdir().expect("project");
        let claude = native(tools.path(), "claude");
        let codex = native(tools.path(), "codex");
        let plan = plan_routes(
            &report(vec![
                probe(AgentKind::Claude, ProbeState::Verified, Some(&claude)),
                probe(AgentKind::Codex, ProbeState::Verified, Some(&codex)),
            ]),
            project.path(),
            ExecutablePolicy::for_host(),
        );
        assert_eq!(plan.skipped, vec![]);
        assert_eq!(
            plan.routes,
            vec![
                PlannedRoute {
                    harness: AgentKind::Claude,
                    protocol: NativeProtocol::ClaudeStreamJson,
                    executable: claude,
                },
                PlannedRoute {
                    harness: AgentKind::Codex,
                    protocol: NativeProtocol::CodexAppServer,
                    executable: codex,
                },
            ]
        );
    }

    #[test]
    fn missing_shimmed_and_in_project_harnesses_are_skipped_with_a_reason() {
        let tools = tempfile::tempdir().expect("tools");
        let project = tempfile::tempdir().expect("project");
        let shim = if cfg!(windows) {
            tools.path().join("claude.cmd")
        } else {
            tools.path().join("claude.js")
        };
        let inside = native(project.path(), "codex");
        let plan = plan_routes(
            &report(vec![
                probe(AgentKind::Claude, ProbeState::Verified, Some(&shim)),
                probe(AgentKind::Codex, ProbeState::Verified, Some(&inside)),
            ]),
            project.path(),
            ExecutablePolicy::for_host(),
        );
        assert!(plan.routes.is_empty());
        assert_eq!(
            plan.skipped,
            vec![
                (AgentKind::Claude, SkipReason::NotANativeExecutable),
                (AgentKind::Codex, SkipReason::InsideProject),
            ]
        );
        let unverified = plan_routes(
            &report(vec![probe(
                AgentKind::Claude,
                ProbeState::Failed,
                Some(&native(tools.path(), "claude")),
            )]),
            project.path(),
            ExecutablePolicy::for_host(),
        );
        assert_eq!(
            unverified.skipped,
            vec![
                (AgentKind::Claude, SkipReason::NotDetected),
                (AgentKind::Codex, SkipReason::NotDetected),
            ]
        );
    }

    #[test]
    fn the_rendered_config_passes_the_daemon_parser() {
        let tools = tempfile::tempdir().expect("tools");
        let routes = vec![
            PlannedRoute {
                harness: AgentKind::Claude,
                protocol: NativeProtocol::ClaudeStreamJson,
                executable: native(tools.path(), "claude"),
            },
            PlannedRoute {
                harness: AgentKind::Codex,
                protocol: NativeProtocol::CodexAppServer,
                executable: native(tools.path(), "codex"),
            },
        ];
        let bytes = render_config(&routes).expect("render");
        validate_config(&bytes, ExecutablePolicy::for_host()).expect("valid config");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(value["routes"][0]["harness"], "claude");
        assert_eq!(value["routes"][0]["protocol"], "claude_stream_json");
        assert_eq!(value["routes"][1]["protocol"], "codex_app_server");
        assert_eq!(value["routes"][1]["allow_execution"], true);
    }

    #[test]
    fn writing_never_replaces_an_existing_config() {
        let project = tempfile::tempdir().expect("project");
        let path = write_config(project.path(), b"{\"first\":true}\n").expect("first write");
        assert_eq!(path, config_path(project.path()));
        let error = write_config(project.path(), b"{\"second\":true}\n").expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&path).expect("config"),
            b"{\"first\":true}\n",
            "the operator's file is untouched"
        );
        let leftovers = fs::read_dir(path.parent().expect("parent"))
            .expect("listing")
            .count();
        assert_eq!(leftovers, 1, "no staging file is left behind");
    }
}
