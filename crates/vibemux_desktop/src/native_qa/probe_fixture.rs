//! Fixed in-process diagnostics for developer QA. Never inspect the host.
use crate::{
    app_state::AppState,
    event_sink::EventSink,
    probe_registry::{ProbeOutcome, ProbeRunner},
};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

const AGENTS: [&str; 11] = [
    "claude",
    "codex",
    "open_code",
    "copilot",
    "grok",
    "qwen",
    "iflow",
    "trae",
    "codebuddy",
    "kimi",
    "dsh",
];
#[cfg(not(test))]
const DURATION: Duration = Duration::from_secs(8);
#[cfg(test)]
const DURATION: Duration = Duration::from_millis(100);
const POLL: Duration = Duration::from_millis(5);

/// Used by actual QA setup and its non-GUI AppState observations. Renderer
/// input cannot select a runner; normal launches keep AppState's default.
pub(crate) fn app_state(settings_path: PathBuf, sink: Arc<dyn EventSink>) -> AppState {
    AppState::new(settings_path, sink).with_probe_runner(Arc::new(FixtureProbe))
}

struct FixtureProbe;
impl ProbeRunner for FixtureProbe {
    fn run(&self, cancel: watch::Receiver<bool>) -> ProbeOutcome {
        let deadline = Instant::now() + DURATION;
        loop {
            if *cancel.borrow() {
                return ProbeOutcome {
                    report: None,
                    error_code: Some("local_probe_cancelled".into()),
                    quiescent: true,
                };
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(POLL);
        }
        let observed = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(time) => time.as_secs(),
            Err(_) => {
                return ProbeOutcome {
                    report: None,
                    error_code: Some("native_qa_probe_clock_unavailable".into()),
                    quiescent: true,
                };
            }
        };
        // Every check is explicitly NotRun. Completing the owned fixture job
        // is not a claim that any installed tool, gateway or model was checked.
        ProbeOutcome {
            report: Some(json!({
                "schema_version":1,"source":"local_probe","observed_at_epoch_seconds":observed,
                "platform":"windows","endpoint_details_omitted":true,
                "agents":AGENTS.map(|agent| json!({"agent":agent,"launcher_state":"not_run","authentication_state":"not_run",
                    "inference_state":"not_run","launcher":"unavailable","version":null,"route":"unknown","code":"not_run"})),
                "gateway":{"state":"not_run","tcp_reachable":false,"health_status":null,"telemetry_state":"not_run","telemetry":[],"code":"not_run"},
                "a2a":{"state":"not_run","correlation_preserved":false,"listener_closed":false,"code":"not_run"}
            })),
            error_code: None,
            quiescent: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event_sink::RecordingEventSink,
        probe_contract::{ProbePhase, ProbeRequest},
    };
    use uuid::Uuid;

    #[test]
    fn qa_composition_completes_not_checked_data_and_cancels_without_an_installed_probe() {
        let temporary = tempfile::tempdir().expect("owned folder");
        let project = temporary.path().join("project");
        std::fs::create_dir_all(project.join(".git")).expect("repository marker");
        let state = app_state(
            temporary.path().join("desktop_settings.json"),
            Arc::new(RecordingEventSink::default()),
        );
        // Check concrete composition before Start. A mistaken production
        // runner must fail here without executing any sibling helper.
        assert!(state.probes.uses_runner::<FixtureProbe>());
        let key = state
            .register_project(&project)
            .expect("owned registration");
        let first = Uuid::new_v4();
        state
            .probe_request(
                "main",
                &key,
                ProbeRequest::Start {
                    operation_id: first,
                },
            )
            .expect("admit fixture");
        let wait = |operation_id| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let snapshot = state
                    .probe_request(
                        "main",
                        &key,
                        ProbeRequest::Status {
                            operation_id: Some(operation_id),
                        },
                    )
                    .expect("status");
                if snapshot
                    .job
                    .as_ref()
                    .is_some_and(|job| job.phase != ProbePhase::Running)
                {
                    break snapshot;
                }
                assert!(Instant::now() < deadline, "bounded mock worker");
                std::thread::sleep(POLL);
            }
        };
        let completed = wait(first);
        assert_eq!(completed.job.expect("job").phase, ProbePhase::Completed);
        let report = completed.result.expect("fixture result").report;
        assert_eq!(report["endpoint_details_omitted"], true);
        let agents = report["agents"].as_array().expect("agents");
        assert_eq!(agents.len(), AGENTS.len());
        for row in agents {
            for field in ["launcher_state", "authentication_state", "inference_state"] {
                assert_eq!(row[field], "not_run");
            }
        }
        assert_eq!(report["gateway"]["state"], "not_run");
        assert_eq!(report["a2a"]["state"], "not_run");
        assert!(
            !serde_json::to_string(&report)
                .expect("public fixture")
                .contains("project")
        );
        let second = Uuid::new_v4();
        state
            .probe_request(
                "main",
                &key,
                ProbeRequest::Start {
                    operation_id: second,
                },
            )
            .expect("second fixture");
        state
            .probe_request(
                "main",
                &key,
                ProbeRequest::Cancel {
                    operation_id: second,
                },
            )
            .expect("cancel");
        let cancelled = wait(second);
        assert_eq!(cancelled.job.expect("job").phase, ProbePhase::Cancelled);
        assert_eq!(
            cancelled
                .result
                .expect("prior result retained")
                .operation_id,
            first
        );
        state.close_main_view();
        assert_eq!(state.probes.shutdown(), 0);
    }
}
