#![forbid(unsafe_code)]
//! Read-only local agent, gateway, and A2A diagnostics, plus the trusted
//! probe cache ([`cache`]) that persists one report for the daemon to read.

pub mod cache;

use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;
use tokio::{
    io::AsyncReadExt, net::TcpStream, process::Command, task::spawn_blocking, time::timeout,
};
use toml::Value as TomlValue;
use url::Url;
use vibemux_a2a::{InformationShare, LocalInformationServer, share_information};

// The agent/launcher/probe-state vocabulary is owned by `vibemux_harness`
// (pure logic, no I/O) and re-exported here so every consumer of the
// historical `vibemux_probe::{AgentKind, LauncherKind, ProbeState}` paths
// compiles unchanged.
pub use vibemux_harness::{AgentKind, LauncherKind, ProbeState};

pub const PROBE_SCHEMA_VERSION: u16 = 1;
pub const DEFAULT_CC_SWITCH_PORT: u16 = 15_721;
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_NETWORK_TIMEOUT: Duration = Duration::from_secs(3);
pub const DEFAULT_TELEMETRY_WINDOW_SECONDS: u64 = 7_200;
pub const MAX_VERSION_TEXT_BYTES: usize = 256;
pub const MAX_COMMAND_OUTPUT_BYTES: u64 = 16 * 1024;
pub const MAX_CONFIG_FILE_BYTES: u64 = 256 * 1024;
pub const MAX_CONFIG_NESTING_DEPTH: usize = 32;
pub const MAX_ENDPOINTS_PER_AGENT: usize = 8;
pub const A2A_PROBE_MARKER: &str = "VIBEMUX_A2A_PROBE_OK";
pub const PROBE_ENVIRONMENT_ALLOWLIST: [&str; 15] = [
    "APPDATA",
    "COMSPEC",
    "HOME",
    "LANG",
    "LC_ALL",
    "LOCALAPPDATA",
    "NO_COLOR",
    "PATH",
    "PATHEXT",
    "SHELL",
    "SystemRoot",
    "TEMP",
    "TMP",
    "TMPDIR",
    "USERPROFILE",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    Direct,
    LocalGateway,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SafeEndpoint {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentProbe {
    pub agent: AgentKind,
    pub launcher_state: ProbeState,
    pub authentication_state: ProbeState,
    pub inference_state: ProbeState,
    pub launcher: LauncherKind,
    /// The resolved launcher artifact the PATH scan found and used (direct
    /// executable, or the `.ps1` script a PowerShell companion wraps). `None`
    /// when no launcher resolved. `#[serde(default)]` keeps caches written by
    /// older builds (which predate this field) parseable as `path: None`.
    #[serde(default)]
    pub path: Option<String>,
    pub version: Option<String>,
    pub route: RouteKind,
    pub endpoints: Vec<SafeEndpoint>,
    pub code: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GatewayTelemetry {
    pub app_type: String,
    pub requests: u64,
    pub failures: u64,
    pub latest_epoch_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GatewayProbe {
    pub state: ProbeState,
    pub host: String,
    pub port: u16,
    pub tcp_reachable: bool,
    pub health_status: Option<u16>,
    pub telemetry_state: ProbeState,
    pub telemetry: Vec<GatewayTelemetry>,
    pub code: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct A2aSelfTestProbe {
    pub state: ProbeState,
    pub correlation_preserved: bool,
    pub listener_closed: bool,
    pub code: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProbeReport {
    pub schema_version: u16,
    pub observed_at_epoch_seconds: u64,
    pub platform: String,
    pub agents: Vec<AgentProbe>,
    pub gateway: GatewayProbe,
    pub a2a: A2aSelfTestProbe,
}

#[derive(Clone, Debug)]
pub struct ProbeConfig {
    pub home_dir: PathBuf,
    pub path_entries: Vec<PathBuf>,
    pub cc_switch_port: u16,
    pub command_timeout: Duration,
    pub network_timeout: Duration,
    pub telemetry_window_seconds: u64,
    pub run_a2a_self_test: bool,
}

impl ProbeConfig {
    #[must_use]
    pub fn from_environment() -> Self {
        let home_dir = env::var_os("USERPROFILE")
            .or_else(|| env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let path_entries = env::var_os("PATH")
            .map(|value| env::split_paths(&value).collect())
            .unwrap_or_default();
        Self {
            home_dir,
            path_entries,
            cc_switch_port: DEFAULT_CC_SWITCH_PORT,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            network_timeout: DEFAULT_NETWORK_TIMEOUT,
            telemetry_window_seconds: DEFAULT_TELEMETRY_WINDOW_SECONDS,
            run_a2a_self_test: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("probe serialization failed")]
    Serialization,
}

#[derive(Clone, Debug)]
struct LauncherSpec {
    executable: PathBuf,
    prefix_args: Vec<String>,
    kind: LauncherKind,
    /// The launcher artifact the PATH scan resolved: the direct executable, or
    /// the `.ps1` script a PowerShell companion wraps (mirrors the Python
    /// reference, which reports the script — not the wrapper — as the path).
    resolved_path: PathBuf,
}

pub async fn run_probe(config: &ProbeConfig) -> ProbeReport {
    // Agent version probes are independent (own launcher resolution, own
    // child process, own deadline), so they run concurrently instead of
    // serializing five worst-case timeouts. join_all preserves the
    // AgentKind::all() order the dashboard and fixtures rely on.
    let gateway = probe_gateway(config);
    let a2a = async {
        if config.run_a2a_self_test {
            probe_a2a().await
        } else {
            A2aSelfTestProbe {
                state: ProbeState::NotRun,
                correlation_preserved: false,
                listener_closed: false,
                code: "a2a_not_run".to_string(),
            }
        }
    };
    let agent_probes = futures::future::join_all(
        AgentKind::all()
            .into_iter()
            .map(|agent| probe_agent(agent, config)),
    );
    let (gateway, a2a, agents) = tokio::join!(gateway, a2a, agent_probes);
    ProbeReport {
        schema_version: PROBE_SCHEMA_VERSION,
        observed_at_epoch_seconds: now_epoch_seconds(),
        platform: env::consts::OS.to_string(),
        agents,
        gateway,
        a2a,
    }
}

pub fn report_json(report: &ProbeReport) -> Result<String, ProbeError> {
    serde_json::to_string_pretty(report).map_err(|_| ProbeError::Serialization)
}

async fn probe_agent(agent: AgentKind, config: &ProbeConfig) -> AgentProbe {
    let endpoints = discover_endpoints(agent, config);
    let route = classify_route(&endpoints);
    let Some(launcher) =
        resolve_launcher(agent.command_name(), &config.path_entries, &config.home_dir)
    else {
        return AgentProbe {
            agent,
            launcher_state: ProbeState::Unavailable,
            authentication_state: ProbeState::NotRun,
            inference_state: ProbeState::NotRun,
            launcher: LauncherKind::Unavailable,
            path: None,
            version: None,
            route,
            endpoints,
            code: "launcher_unavailable".to_string(),
        };
    };
    let path = Some(launcher.resolved_path.to_string_lossy().into_owned());
    match probe_version(&launcher, config.command_timeout).await {
        Ok(version) => AgentProbe {
            agent,
            launcher_state: ProbeState::Verified,
            authentication_state: ProbeState::NotRun,
            inference_state: ProbeState::NotRun,
            launcher: launcher.kind,
            path,
            version: Some(version),
            route,
            endpoints,
            code: "version_verified".to_string(),
        },
        Err(()) => AgentProbe {
            agent,
            launcher_state: ProbeState::Failed,
            authentication_state: ProbeState::NotRun,
            inference_state: ProbeState::NotRun,
            launcher: launcher.kind,
            path,
            version: None,
            route,
            endpoints,
            code: "version_probe_failed".to_string(),
        },
    }
}

async fn probe_version(launcher: &LauncherSpec, deadline: Duration) -> Result<String, ()> {
    let mut command = Command::new(&launcher.executable);
    command.args(&launcher.prefix_args).arg("--version");
    command.env_clear();
    for key in PROBE_ENVIRONMENT_ALLOWLIST {
        if let Some(value) = env::var_os(key) {
            command.env(key, value);
        }
    }
    command.kill_on_drop(true);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Windows: probe children (harness shims, powershell/pwsh wrappers
    // for .ps1 launchers) must never flash a console window of their
    // own, even when the parent is a GUI-subsystem process.
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|_| ())?;
    let stdout = child.stdout.take().ok_or(())?;
    let stderr = child.stderr.take().ok_or(())?;
    let read_output = async move {
        let mut stdout = stdout.take(MAX_COMMAND_OUTPUT_BYTES + 1);
        let mut stderr = stderr.take(MAX_COMMAND_OUTPUT_BYTES + 1);
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let (stdout_result, stderr_result, status_result) = tokio::join!(
            stdout.read_to_end(&mut stdout_bytes),
            stderr.read_to_end(&mut stderr_bytes),
            child.wait()
        );
        stdout_result.map_err(|_| ())?;
        stderr_result.map_err(|_| ())?;
        let status = status_result.map_err(|_| ())?;
        Ok::<_, ()>((status, stdout_bytes, stderr_bytes))
    };
    let (status, stdout, stderr) = timeout(deadline, read_output).await.map_err(|_| ())??;
    if !status.success()
        || stdout.len() > MAX_COMMAND_OUTPUT_BYTES as usize
        || stderr.len() > MAX_COMMAND_OUTPUT_BYTES as usize
    {
        return Err(());
    }
    let text = String::from_utf8_lossy(&stdout);
    let fallback = String::from_utf8_lossy(&stderr);
    let line = text
        .lines()
        .chain(fallback.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or(())?;
    Ok(sanitize_text(line, MAX_VERSION_TEXT_BYTES))
}

fn resolve_launcher(command: &str, paths: &[PathBuf], home_dir: &Path) -> Option<LauncherSpec> {
    if cfg!(windows) {
        for directory in paths {
            for suffix in [".exe", ".com", ".ps1", ".cmd"] {
                let candidate = directory.join(format!("{command}{suffix}"));
                if !candidate.is_file() {
                    continue;
                }
                if suffix == ".exe" || suffix == ".com" {
                    return Some(LauncherSpec {
                        resolved_path: candidate.clone(),
                        executable: candidate,
                        prefix_args: Vec::new(),
                        kind: LauncherKind::DirectExecutable,
                    });
                }
                let script = if suffix == ".cmd" {
                    candidate.with_extension("ps1")
                } else {
                    candidate
                };
                if script.is_file() {
                    let powershell = resolve_powershell(paths, home_dir)?;
                    return Some(LauncherSpec {
                        executable: powershell,
                        prefix_args: vec![
                            "-NoLogo".to_string(),
                            "-NoProfile".to_string(),
                            "-NonInteractive".to_string(),
                            "-ExecutionPolicy".to_string(),
                            "Bypass".to_string(),
                            "-File".to_string(),
                            script.to_string_lossy().into_owned(),
                        ],
                        kind: LauncherKind::PowerShellCompanion,
                        // The wrapped script — not the wrapper — is the
                        // launcher artifact, mirroring the Python reference.
                        resolved_path: script,
                    });
                }
            }
        }
        return None;
    }
    paths
        .iter()
        .map(|directory| directory.join(command))
        .find(|candidate| is_executable_regular_file(candidate))
        .map(|executable| LauncherSpec {
            resolved_path: executable.clone(),
            executable,
            prefix_args: Vec::new(),
            kind: LauncherKind::DirectExecutable,
        })
}

/// Unix PATH-scan predicate: a regular file with at least one execute bit
/// (mirrors `shutil.which` in the Python reference). On other platforms the
/// caller-side `cfg!(windows)` branch already resolved the launcher, so the
/// plain regular-file check is the compiled fallback.
fn is_executable_regular_file(candidate: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(candidate)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        candidate.is_file()
    }
}

fn resolve_powershell(paths: &[PathBuf], home_dir: &Path) -> Option<PathBuf> {
    let from_path = paths
        .iter()
        .flat_map(|directory| [directory.join("powershell.exe"), directory.join("pwsh.exe")])
        .find(|candidate| candidate.is_file());
    from_path.or_else(|| {
        let system_root = env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir.join("Windows"));
        let candidate = system_root.join("System32/WindowsPowerShell/v1.0/powershell.exe");
        candidate.is_file().then_some(candidate)
    })
}

fn discover_endpoints(agent: AgentKind, config: &ProbeConfig) -> Vec<SafeEndpoint> {
    let mut values = Vec::new();
    match agent {
        AgentKind::Claude => {
            if let Ok(value) = env::var("ANTHROPIC_BASE_URL") {
                values.push(value);
            }
            collect_json_endpoints(
                &config.home_dir.join(".claude/settings.json"),
                &["ANTHROPIC_BASE_URL", "baseURL", "base_url"],
                &mut values,
            );
        }
        AgentKind::Codex => collect_toml_endpoints(
            &config.home_dir.join(".codex/config.toml"),
            &["base_url"],
            &mut values,
        ),
        AgentKind::OpenCode => collect_json_endpoints(
            &config.home_dir.join(".config/opencode/opencode.json"),
            &["baseURL", "base_url"],
            &mut values,
        ),
        // The domestic CLI agents expose their endpoints through their own
        // onboarding flows; discovery lands here once those paths are
        // validated. Absence of endpoints keeps the route `unknown` and the
        // launcher/version probe still runs.
        AgentKind::Qwen
        | AgentKind::Iflow
        | AgentKind::Trae
        | AgentKind::Codebuddy
        | AgentKind::Kimi => {}
        AgentKind::Copilot => {}
        AgentKind::Grok => collect_toml_endpoints(
            &config.home_dir.join(".grok/config.toml"),
            &["base_url"],
            &mut values,
        ),
    }
    let mut unique = BTreeSet::new();
    values
        .into_iter()
        .filter_map(|value| safe_endpoint(&value))
        .filter(|endpoint| {
            unique.insert(format!(
                "{}://{}{:?}{}",
                endpoint.scheme, endpoint.host, endpoint.port, endpoint.path
            ))
        })
        .take(MAX_ENDPOINTS_PER_AGENT)
        .collect()
}

fn collect_json_endpoints(path: &Path, keys: &[&str], output: &mut Vec<String>) {
    let Some(encoded) = read_bounded_config(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<JsonValue>(&encoded) else {
        return;
    };
    collect_json_values(&value, keys, output, MAX_CONFIG_NESTING_DEPTH);
}

fn collect_json_values(
    value: &JsonValue,
    keys: &[&str],
    output: &mut Vec<String>,
    remaining_depth: usize,
) {
    if remaining_depth == 0 || output.len() >= MAX_ENDPOINTS_PER_AGENT {
        return;
    }
    match value {
        JsonValue::Object(fields) => {
            for (key, value) in fields {
                if keys.contains(&key.as_str()) {
                    if let Some(endpoint) = value.as_str() {
                        output.push(endpoint.to_string());
                    }
                } else {
                    collect_json_values(value, keys, output, remaining_depth - 1);
                }
            }
        }
        JsonValue::Array(items) => {
            for item in items {
                collect_json_values(item, keys, output, remaining_depth - 1);
            }
        }
        _ => {}
    }
}

fn collect_toml_endpoints(path: &Path, keys: &[&str], output: &mut Vec<String>) {
    let Some(encoded) = read_bounded_config(path) else {
        return;
    };
    let Ok(value) = encoded.parse::<TomlValue>() else {
        return;
    };
    collect_toml_values(&value, keys, output, MAX_CONFIG_NESTING_DEPTH);
}

fn collect_toml_values(
    value: &TomlValue,
    keys: &[&str],
    output: &mut Vec<String>,
    remaining_depth: usize,
) {
    if remaining_depth == 0 || output.len() >= MAX_ENDPOINTS_PER_AGENT {
        return;
    }
    match value {
        TomlValue::Table(fields) => {
            for (key, value) in fields {
                if keys.contains(&key.as_str()) {
                    if let Some(endpoint) = value.as_str() {
                        output.push(endpoint.to_string());
                    }
                } else {
                    collect_toml_values(value, keys, output, remaining_depth - 1);
                }
            }
        }
        TomlValue::Array(items) => {
            for item in items {
                collect_toml_values(item, keys, output, remaining_depth - 1);
            }
        }
        _ => {}
    }
}

fn read_bounded_config(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn safe_endpoint(value: &str) -> Option<SafeEndpoint> {
    let parsed = Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(SafeEndpoint {
        scheme: parsed.scheme().to_string(),
        host: parsed.host_str()?.to_string(),
        port: parsed.port_or_known_default(),
        path: sanitize_text(parsed.path(), MAX_VERSION_TEXT_BYTES),
    })
}

fn classify_route(endpoints: &[SafeEndpoint]) -> RouteKind {
    if endpoints.iter().any(|endpoint| {
        endpoint.host.eq_ignore_ascii_case("localhost")
            || endpoint
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    }) {
        RouteKind::LocalGateway
    } else if endpoints.is_empty() {
        RouteKind::Unknown
    } else {
        RouteKind::Direct
    }
}

async fn probe_gateway(config: &ProbeConfig) -> GatewayProbe {
    let address = format!("127.0.0.1:{}", config.cc_switch_port);
    let tcp_reachable = timeout(config.network_timeout, TcpStream::connect(&address))
        .await
        .is_ok_and(|result| result.is_ok());
    let health_status = if tcp_reachable {
        if let Ok(client) = reqwest::Client::builder().no_proxy().build() {
            timeout(
                config.network_timeout,
                client.get(format!("http://{address}/health")).send(),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .map(|response| response.status().as_u16())
        } else {
            None
        }
    } else {
        None
    };
    let telemetry_path = config.home_dir.join(".cc-switch/cc-switch.db");
    let window = config.telemetry_window_seconds;
    let telemetry_result = spawn_blocking(move || read_gateway_telemetry(&telemetry_path, window))
        .await
        .ok()
        .and_then(Result::ok);
    let telemetry_state = if telemetry_result.is_some() {
        ProbeState::Verified
    } else {
        ProbeState::Unavailable
    };
    let state = if tcp_reachable && health_status.is_some_and(|status| status < 400) {
        ProbeState::Verified
    } else if tcp_reachable {
        ProbeState::Failed
    } else {
        ProbeState::Unavailable
    };
    GatewayProbe {
        state,
        host: "127.0.0.1".to_string(),
        port: config.cc_switch_port,
        tcp_reachable,
        health_status,
        telemetry_state,
        telemetry: telemetry_result.unwrap_or_default(),
        code: match state {
            ProbeState::Verified => "gateway_verified",
            ProbeState::Failed => "gateway_health_failed",
            ProbeState::Unavailable | ProbeState::NotRun => "gateway_unavailable",
        }
        .to_string(),
    }
}

fn read_gateway_telemetry(path: &Path, window_seconds: u64) -> Result<Vec<GatewayTelemetry>, ()> {
    if !path.is_file() {
        return Err(());
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| ())?;
    let since = now_epoch_seconds().saturating_sub(window_seconds);
    let mut statement = connection
        .prepare(
            "SELECT app_type, COUNT(*), SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END), MAX(created_at) FROM proxy_request_logs WHERE created_at >= ? GROUP BY app_type ORDER BY MAX(created_at) DESC LIMIT 32",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map([since], |row| {
            Ok(GatewayTelemetry {
                app_type: sanitize_text(&row.get::<_, String>(0)?, 64),
                requests: row.get(1)?,
                failures: row.get(2)?,
                latest_epoch_seconds: row.get(3)?,
            })
        })
        .map_err(|_| ())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|_| ())
}

async fn probe_a2a() -> A2aSelfTestProbe {
    let result = async {
        let server = LocalInformationServer::start("probe_receiver")
            .await
            .map_err(|_| ())?;
        let address = server.address();
        let correlation_id = format!("probe_{}", now_epoch_seconds());
        let share = InformationShare {
            correlation_id: correlation_id.clone(),
            sender_peer_id: "probe_sender".to_string(),
            information_kind: "health".to_string(),
            text: A2A_PROBE_MARKER.to_string(),
            metadata: Default::default(),
        };
        let acknowledgement = share_information(server.base_url(), &share)
            .await
            .map_err(|_| ())?;
        let correlation_preserved = acknowledgement.correlation_id == correlation_id;
        server.shutdown().await.map_err(|_| ())?;
        let listener_closed = TcpStream::connect(address).await.is_err();
        Ok::<_, ()>((correlation_preserved, listener_closed))
    }
    .await;
    match result {
        Ok((correlation_preserved, listener_closed))
            if correlation_preserved && listener_closed =>
        {
            A2aSelfTestProbe {
                state: ProbeState::Verified,
                correlation_preserved,
                listener_closed,
                code: "a2a_self_test_verified".to_string(),
            }
        }
        Ok((correlation_preserved, listener_closed)) => A2aSelfTestProbe {
            state: ProbeState::Failed,
            correlation_preserved,
            listener_closed,
            code: "a2a_self_test_incomplete".to_string(),
        },
        Err(()) => A2aSelfTestProbe {
            state: ProbeState::Failed,
            correlation_preserved: false,
            listener_closed: false,
            code: "a2a_self_test_failed".to_string(),
        },
    }
}

fn sanitize_text(value: &str, max_bytes: usize) -> String {
    let mut sanitized = String::new();
    for character in value.chars().filter(|character| !character.is_control()) {
        if sanitized.len() + character.len_utf8() > max_bytes {
            break;
        }
        sanitized.push(character);
    }
    sanitized
}

fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    const PROBE_REPORT_FIXTURE: &str = include_str!("../tests/fixtures/probe_report_v1.json");

    #[test]
    fn safe_endpoint_rejects_credentials() {
        assert!(safe_endpoint("https://token@example.com/v1").is_none());
        assert!(safe_endpoint("https://example.com/v1").is_some());
    }

    #[test]
    fn endpoint_discovery_reads_only_allowlisted_keys() {
        let temporary = TempDir::new().expect("temporary home");
        let config_root = temporary.path().join(".config/opencode");
        fs::create_dir_all(&config_root).expect("config directory");
        fs::write(
            config_root.join("opencode.json"),
            r#"{"provider":{"safe":{"baseURL":"http://127.0.0.1:15721/v1","apiKey":"must-not-appear"}}}"#,
        )
        .expect("write fixture");
        let config = ProbeConfig {
            home_dir: temporary.path().to_path_buf(),
            path_entries: Vec::new(),
            cc_switch_port: DEFAULT_CC_SWITCH_PORT,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            network_timeout: DEFAULT_NETWORK_TIMEOUT,
            telemetry_window_seconds: DEFAULT_TELEMETRY_WINDOW_SECONDS,
            run_a2a_self_test: false,
        };
        let endpoints = discover_endpoints(AgentKind::OpenCode, &config);
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].host, "127.0.0.1");
        assert_eq!(classify_route(&endpoints), RouteKind::LocalGateway);
        assert!(
            !serde_json::to_string(&endpoints)
                .expect("serialize")
                .contains("must-not-appear")
        );
    }

    #[test]
    fn oversized_config_is_ignored_before_parsing() {
        let temporary = TempDir::new().expect("temporary home");
        let config_root = temporary.path().join(".config/opencode");
        fs::create_dir_all(&config_root).expect("config directory");
        fs::write(
            config_root.join("opencode.json"),
            vec![b'x'; MAX_CONFIG_FILE_BYTES as usize + 1],
        )
        .expect("write oversized fixture");
        let config = ProbeConfig {
            home_dir: temporary.path().to_path_buf(),
            path_entries: Vec::new(),
            cc_switch_port: DEFAULT_CC_SWITCH_PORT,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            network_timeout: DEFAULT_NETWORK_TIMEOUT,
            telemetry_window_seconds: DEFAULT_TELEMETRY_WINDOW_SECONDS,
            run_a2a_self_test: false,
        };
        assert!(discover_endpoints(AgentKind::OpenCode, &config).is_empty());
    }

    #[test]
    fn telemetry_reader_uses_aggregate_columns_only() {
        let temporary = TempDir::new().expect("temporary database");
        let database = temporary.path().join("cc-switch.db");
        let connection = Connection::open(&database).expect("open fixture database");
        connection
            .execute_batch(
                "CREATE TABLE proxy_request_logs(app_type TEXT, status_code INTEGER, created_at INTEGER, prompt TEXT); INSERT INTO proxy_request_logs VALUES('claude', 200, 4102444800, 'must-not-appear'); INSERT INTO proxy_request_logs VALUES('grokbuild', 502, 4102444801, 'must-not-appear');",
            )
            .expect("fixture schema");
        drop(connection);
        let rows = read_gateway_telemetry(&database, u64::MAX).expect("read telemetry");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.iter().map(|row| row.failures).sum::<u64>(), 1);
        assert!(
            !serde_json::to_string(&rows)
                .expect("serialize")
                .contains("must-not-appear")
        );
    }

    #[tokio::test]
    async fn a2a_self_test_is_owned_and_repeatable() {
        let first = probe_a2a().await;
        let second = probe_a2a().await;
        assert_eq!(first.state, ProbeState::Verified);
        assert_eq!(second.state, ProbeState::Verified);
        assert!(first.listener_closed && second.listener_closed);
    }

    #[test]
    fn report_json_round_trips() {
        let report = ProbeReport {
            schema_version: PROBE_SCHEMA_VERSION,
            observed_at_epoch_seconds: 1,
            platform: "windows".to_string(),
            agents: Vec::new(),
            gateway: GatewayProbe {
                state: ProbeState::Unavailable,
                host: "127.0.0.1".to_string(),
                port: DEFAULT_CC_SWITCH_PORT,
                tcp_reachable: false,
                health_status: None,
                telemetry_state: ProbeState::Unavailable,
                telemetry: Vec::new(),
                code: "gateway_unavailable".to_string(),
            },
            a2a: A2aSelfTestProbe {
                state: ProbeState::NotRun,
                correlation_preserved: false,
                listener_closed: false,
                code: "a2a_not_run".to_string(),
            },
        };
        let encoded = report_json(&report).expect("serialize report");
        assert_eq!(
            serde_json::from_str::<ProbeReport>(&encoded).expect("decode report"),
            report
        );
        let fixture: ProbeReport =
            serde_json::from_str(PROBE_REPORT_FIXTURE).expect("decode checked-in fixture");
        assert_eq!(fixture.schema_version, PROBE_SCHEMA_VERSION);
        // The checked-in fixture predates the `path` field; it must still
        // parse, defaulting the resolved path to None.
        assert_eq!(fixture.agents.len(), 1);
        assert_eq!(fixture.agents[0].path, None);
        let fixture_encoded = report_json(&fixture).expect("encode checked-in fixture");
        assert_eq!(
            serde_json::from_str::<ProbeReport>(&fixture_encoded)
                .expect("round-trip checked-in fixture"),
            fixture
        );
    }

    #[test]
    fn resolve_launcher_records_the_path_it_used() {
        let temp = TempDir::new().expect("temp path dir");
        let home = TempDir::new().expect("temp home dir");
        let paths = [temp.path().to_path_buf()];

        #[cfg(windows)]
        let expected = {
            let exe = temp.path().join("myagent.exe");
            fs::write(&exe, b"dummy").expect("write exe");
            exe
        };
        #[cfg(unix)]
        let expected = {
            use std::os::unix::fs::PermissionsExt;
            let bin = temp.path().join("myagent");
            fs::write(&bin, b"#!/bin/sh\n").expect("write bin");
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
            bin
        };

        let spec = resolve_launcher("myagent", &paths, home.path()).expect("launcher resolved");
        assert_eq!(spec.kind, LauncherKind::DirectExecutable);
        assert_eq!(spec.resolved_path, expected);
    }

    #[test]
    fn resolve_launcher_without_a_match_has_no_path() {
        let temp = TempDir::new().expect("temp path dir");
        let home = TempDir::new().expect("temp home dir");
        // An empty PATH directory resolves nothing, so no launcher (and thus
        // no resolved path) is produced.
        assert!(resolve_launcher("nope", &[temp.path().to_path_buf()], home.path()).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unix_resolve_launcher_requires_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().expect("temp path dir");
        let home = TempDir::new().expect("temp home dir");
        let bin = temp.path().join("myagent");
        fs::write(&bin, b"data").expect("write bin");
        // A non-executable regular file must not resolve (mirrors shutil.which).
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(resolve_launcher("myagent", &[temp.path().to_path_buf()], home.path()).is_none());
    }
}
