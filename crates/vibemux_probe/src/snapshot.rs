//! Bounded diagnostic snapshots and an explicit metadata-only public projection.
use std::{collections::BTreeSet, io::Read, path::PathBuf};

use crate::{AgentKind, PROBE_SCHEMA_VERSION, ProbeConfig, ProbeReport, ProbeState, run_probe};
use serde_json::{Value, json};

const MAX_REPORT_BYTES: usize = 256 * 1024;
const MAX_TELEMETRY_ITEMS: usize = 64;
const MAX_ENDPOINTS_PER_AGENT: usize = 16;
const MAX_FIELD_BYTES: usize = 4096;

/// Errors retain private causes, while public formatting exposes only stable codes.
pub struct Error {
    code: &'static str,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}
impl Error {
    fn new(code: &'static str) -> Self {
        Self { code, source: None }
    }
    fn with_source(
        code: &'static str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            code,
            source: Some(Box::new(source)),
        }
    }
    pub fn code(&self) -> &'static str {
        self.code
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code)
    }
}
impl std::fmt::Debug for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.code.fmt(formatter)
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}

pub struct Snapshot {
    pub report: ProbeReport,
    pub source: &'static str,
}

impl Snapshot {
    pub fn public_json(&self) -> Value {
        let report = &self.report;
        json!({
            "schema_version": PROBE_SCHEMA_VERSION,
            "source": self.source,
            "observed_at_epoch_seconds": report.observed_at_epoch_seconds,
            "platform": report.platform,
            "endpoint_details_omitted": true,
            "agents": report.agents.iter().map(|agent| json!({
                "agent":agent.agent,"launcher_state":agent.launcher_state,"authentication_state":agent.authentication_state,
                "inference_state":agent.inference_state,"launcher":agent.launcher,"version":agent.version,
                "route":agent.route,"code":agent.code,
            })).collect::<Vec<_>>(),
            "gateway": {"state":report.gateway.state,"tcp_reachable":report.gateway.tcp_reachable,
                "health_status":report.gateway.health_status,"telemetry_state":report.gateway.telemetry_state,
                "telemetry":report.gateway.telemetry,"code":report.gateway.code},
            "a2a":report.a2a,
        })
    }
}

pub async fn load(report_path: Option<PathBuf>) -> Result<Snapshot, Error> {
    let (report, source) = if let Some(report_path) = report_path {
        let report = tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(report_path)
                .map_err(|source| Error::with_source("probe_report_unavailable", source))?;
            let mut bytes = Vec::new();
            file.take(MAX_REPORT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|source| Error::with_source("probe_report_unavailable", source))?;
            if bytes.len() > MAX_REPORT_BYTES {
                return Err(Error::new("probe_report_too_large"));
            }
            serde_json::from_slice::<ProbeReport>(&bytes)
                .map_err(|source| Error::with_source("probe_report_invalid", source))
        })
        .await
        .map_err(|source| Error::with_source("probe_report_read_failed", source))??;
        (report, "saved_report")
    } else {
        (
            run_probe(&ProbeConfig::from_environment()).await,
            "local_probe",
        )
    };
    Ok(Snapshot {
        report: sanitize(report)?,
        source,
    })
}

fn sanitize(mut report: ProbeReport) -> Result<ProbeReport, Error> {
    if report.schema_version != PROBE_SCHEMA_VERSION {
        return Err(Error::new("probe_report_version_unsupported"));
    }
    if report.agents.len() > AgentKind::all().len()
        || report.gateway.telemetry.len() > MAX_TELEMETRY_ITEMS
    {
        return Err(Error::new("probe_report_too_large"));
    }
    let mut seen = BTreeSet::new();
    for agent in &mut report.agents {
        if !seen.insert(agent.agent) {
            return Err(Error::new("probe_report_duplicate_agent"));
        }
        if agent.endpoints.len() > MAX_ENDPOINTS_PER_AGENT
            || agent.code.len() > MAX_FIELD_BYTES
            || agent
                .version
                .as_ref()
                .is_some_and(|version| version.len() > MAX_FIELD_BYTES)
        {
            return Err(Error::new("probe_report_too_large"));
        }
        agent.version = agent.version.as_deref().and_then(public_version);
        agent.code = public_code(&agent.code, agent.launcher_state);
        agent.endpoints.clear();
    }
    report.platform = match report.platform.as_str() {
        "windows" | "linux" | "macos" => report.platform,
        _ => "unknown".to_string(),
    };
    report.gateway.host = "endpoint_withheld".to_string();
    report.gateway.port = 0;
    report.gateway.code = public_code(&report.gateway.code, report.gateway.state);
    for row in &mut report.gateway.telemetry {
        row.app_type = if AgentKind::all()
            .iter()
            .any(|agent| row.app_type == agent.command_name())
        {
            row.app_type.clone()
        } else {
            "other".to_string()
        };
        if row.failures > row.requests {
            return Err(Error::new("probe_report_invalid_telemetry"));
        }
    }
    report.a2a.code = public_code(&report.a2a.code, report.a2a.state);
    Ok(report)
}

fn public_version(value: &str) -> Option<String> {
    // Expose numeric version components only, never arbitrary launcher output.
    value.split_whitespace().find_map(|word| {
        let word = word.strip_prefix('v').unwrap_or(word);
        let version = word
            .chars()
            .take_while(|character| character.is_ascii_digit() || *character == '.')
            .take(32)
            .collect::<String>();
        let parts = version.split('.').collect::<Vec<_>>();
        if (2..=3).contains(&parts.len())
            && parts.iter().all(|part| !part.is_empty() && part.len() <= 8)
        {
            Some(version)
        } else {
            None
        }
    })
}

fn public_code(code: &str, state: ProbeState) -> String {
    const CODES: &[&str] = &[
        "version_verified",
        "launcher_unavailable",
        "version_probe_failed",
        "version_probe_timeout",
        "gateway_verified",
        "gateway_unavailable",
        "gateway_failed",
        "a2a_self_test_verified",
        "a2a_self_test_failed",
        "not_run",
    ];
    if CODES.contains(&code) {
        code.to_string()
    } else {
        format!(
            "reported_{}",
            match state {
                ProbeState::Verified => "verified",
                ProbeState::Failed => "failed",
                ProbeState::Unavailable => "unavailable",
                ProbeState::NotRun => "not_run",
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ProbeReport {
        serde_json::from_str(include_str!("../tests/fixtures/probe_report_v1.json")).unwrap()
    }

    #[test]
    fn saved_report_projection_never_echoes_topology_or_arbitrary_text() {
        let mut report = fixture();
        report.platform = "PRIVATE_PLATFORM".to_string();
        report.agents[0].version = Some("PRIVATE_TOKEN\u{1b}[31m".to_string());
        report.agents[0].code = "PRIVATE_CODE".to_string();
        report.gateway.host = "PRIVATE_HOST".to_string();
        report.gateway.telemetry[0].app_type = "PRIVATE_APPLICATION".to_string();
        report.a2a.code = "PRIVATE_A2A_CODE".to_string();
        let snapshot = Snapshot {
            report: sanitize(report).unwrap(),
            source: "saved_report",
        };
        let encoded = snapshot.public_json().to_string();
        assert!(!encoded.contains("PRIVATE_"));
        assert!(!encoded.contains("15721"));
        assert!(!encoded.contains("127.0.0.1"));
        assert!(!encoded.contains("\\u001b"));
        assert_eq!(snapshot.public_json()["source"], "saved_report");
    }

    #[test]
    fn schema_duplicates_and_invalid_telemetry_fail_closed() {
        let mut report = fixture();
        report.schema_version = 999;
        assert!(sanitize(report).is_err());
        let mut report = fixture();
        report.agents.push(report.agents[0].clone());
        assert!(sanitize(report).is_err());
        let mut report = fixture();
        report.gateway.telemetry[0].failures = 5;
        assert!(sanitize(report).is_err());
        assert_eq!(
            public_version("codex-cli 0.149.1"),
            Some("0.149.1".to_string())
        );
        assert_eq!(public_version("PRIVATE_VERSION"), None);
    }

    #[tokio::test]
    async fn report_file_is_bounded_before_json_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("oversized.json");
        std::fs::write(&path, vec![b' '; MAX_REPORT_BYTES + 1]).unwrap();
        let error = match load(Some(path)).await {
            Ok(_) => panic!("must reject"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "probe_report_too_large");
    }
}
