//! Single-launcher observations with no provider, gateway, or A2A discovery.

use std::{env, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    AgentKind, LauncherKind, ProbeConfig, ProbeState, is_terminal_unsafe, now_epoch_seconds,
    observe_launcher, snapshot::public_version,
};

pub(crate) const SCHEMA_VERSION: u16 = 2;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Coverage {
    VersionOnly,
}

/// A private report containing only the result of resolving and version-probing
/// one launcher. It intentionally cannot be parsed as a full v1 report.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionProbeReport {
    schema_version: u16,
    coverage: Coverage,
    observed_at_epoch_seconds: u64,
    platform: String,
    pub observation: LauncherObservation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherObservation {
    pub agent: AgentKind,
    pub launcher_state: ProbeState,
    pub launcher: LauncherKind,
    pub path: Option<String>,
    pub version: Option<String>,
}

impl VersionProbeReport {
    pub(crate) fn is_valid(&self) -> bool {
        let observation = &self.observation;
        let path_valid = observation.path.as_deref().is_some_and(valid_path);
        let has_launcher = observation.launcher != LauncherKind::Unavailable;
        self.schema_version == SCHEMA_VERSION
            && match observation.launcher_state {
                ProbeState::Verified => {
                    has_launcher
                        && path_valid
                        && observation.version.as_deref().is_some_and(|version| {
                            public_version(version).as_deref() == Some(version)
                        })
                }
                ProbeState::Failed => has_launcher && path_valid && observation.version.is_none(),
                ProbeState::Unavailable => {
                    !has_launcher && observation.path.is_none() && observation.version.is_none()
                }
                ProbeState::NotRun => false,
            }
    }
}

fn valid_path(path: &str) -> bool {
    Path::new(path).is_absolute() && !path.chars().any(is_terminal_unsafe)
}

/// Only this launcher is resolved and invoked. Use absolute PATH entries so
/// resolution cannot depend on a relative entry in the current directory.
pub async fn run_version_probe(config: &ProbeConfig, agent: AgentKind) -> VersionProbeReport {
    let paths = config
        .path_entries
        .iter()
        .filter(|path| path.to_str().is_some_and(valid_path))
        .cloned()
        .collect::<Vec<_>>();
    let (launcher, path, version) = observe_launcher(agent, config, &paths).await;
    let version = version.as_deref().and_then(public_version);
    let launcher_state = match (&path, &version) {
        (None, _) => ProbeState::Unavailable,
        (Some(_), None) => ProbeState::Failed,
        (Some(_), Some(_)) => ProbeState::Verified,
    };
    VersionProbeReport {
        schema_version: SCHEMA_VERSION,
        coverage: Coverage::VersionOnly,
        observed_at_epoch_seconds: now_epoch_seconds(),
        platform: env::consts::OS.to_string(),
        observation: LauncherObservation {
            agent,
            launcher_state,
            launcher,
            path,
            version,
        },
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, net::TcpListener, os::unix::fs::PermissionsExt};

    use super::*;
    use crate::{ProbeReport, cache};

    fn write_launcher(path: &Path, program: &str) {
        fs::write(path, program).expect("launcher");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("launcher mode");
    }

    #[tokio::test]
    async fn version_only_observes_one_launcher_and_round_trips_as_v2() {
        let temp = tempfile::tempdir().expect("temp");
        let gateway = TcpListener::bind("127.0.0.1:0").expect("gateway canary");
        gateway.set_nonblocking(true).expect("nonblocking");
        let launcher = temp.path().join("claude");
        write_launcher(
            &launcher,
            "#!/bin/sh\n[ \"$#\" = 1 ] && [ \"$1\" = --version ] || exit 1\nprintf '2.1.322 PRIVATE_VERSION_SUFFIX\\n'\n",
        );
        let unselected = temp.path().join("codex");
        write_launcher(&unselected, "#!/bin/sh\n: > \"$0.ran\"\n");
        let mut config = ProbeConfig::from_environment();
        config.path_entries = vec![temp.path().to_path_buf()];
        config.cc_switch_port = gateway.local_addr().expect("gateway port").port();

        let report = run_version_probe(&config, AgentKind::Claude).await;
        assert_eq!(report.observation.launcher_state, ProbeState::Verified);
        assert_eq!(report.observation.version.as_deref(), Some("2.1.322"));
        assert_eq!(report.observation.path.as_deref(), launcher.to_str());
        let encoded = serde_json::to_string(&report).expect("report");
        assert!(!encoded.contains("PRIVATE_VERSION_SUFFIX"));
        assert!(serde_json::from_str::<ProbeReport>(&encoded).is_err());
        assert!(!temp.path().join("codex.ran").exists());
        assert_eq!(
            gateway.accept().expect_err("no gateway connection").kind(),
            std::io::ErrorKind::WouldBlock
        );

        let path = cache::default_cache_path(temp.path());
        cache::write_version_cache(&report, &path).expect("cache");
        let cache::DetectionCache::VersionOnly(decoded) =
            cache::read_detection_cache(&path).expect("read")
        else {
            panic!("expected v2 cache");
        };
        assert_eq!(decoded, report);

        write_launcher(&launcher, "#!/bin/sh\nprintf 'NOT_A_VERSION\\n'\n");
        let failed = run_version_probe(&config, AgentKind::Claude).await;
        assert_eq!(failed.observation.launcher_state, ProbeState::Failed);
        assert_eq!(failed.observation.version, None);
        assert!(failed.is_valid());
        fs::remove_file(launcher).expect("remove launcher");
        let unavailable = run_version_probe(&config, AgentKind::Claude).await;
        assert_eq!(
            unavailable.observation.launcher_state,
            ProbeState::Unavailable
        );
        assert!(unavailable.is_valid());
    }
}
