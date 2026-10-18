#![cfg(feature = "test_helpers")]

use std::{
    future::Future,
    path::PathBuf,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    sync::{mpsc, watch},
    time::{sleep, timeout},
};
use uuid::Uuid;
use vibemux_harness_native::{
    config::{Config, Limits, Route},
    contract::{
        Harness, Kind, MAX_PROMPT_BYTES, Outcome, Protocol, Request, SCHEMA_VERSION, Status,
    },
    dispatch::Dispatcher,
    native,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const READY_TIMEOUT: Duration = Duration::from_secs(4);
const READY_POLL: Duration = Duration::from_millis(20);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
const CANCEL_DELAY: Duration = Duration::from_millis(100);
const BLOCKED_TIMEOUT_MS: u64 = 30_000;
const CANCEL_SHUTDOWN_MS: u64 = 300;
const LONG_SHUTDOWN_MS: u64 = 3_000;
const PROMPT: &str = "SYNTHETIC_PROMPT_SENTINEL ; echo injected > synthetic_injection_marker & $(synthetic_command) | < > `literal` %SYNTHETIC_ENV% \n你好";
const HARNESSES: [Harness; 5] = [
    Harness::Codex,
    Harness::Claude,
    Harness::Opencode,
    Harness::Copilot,
    Harness::Grok,
];
const DIALECT_HARNESSES: [Harness; 3] = [Harness::Codex, Harness::Claude, Harness::Opencode];

struct Fixture {
    _directory: TempDir,
    config_path: PathBuf,
    working_directory: PathBuf,
    ready_path: PathBuf,
    dispatcher: Dispatcher,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let working_directory = directory.path().join("workspace");
        std::fs::create_dir(&working_directory).unwrap();
        let ready_path = directory.path().join("fixture_ready");
        let config_path = directory.path().join("fixture_config.json");
        let harnesses = HARNESSES
            .iter()
            .map(|harness| {
                let mut arguments = vec!["--fixture_mode".to_string(), mode.to_string()];
                if matches!(mode, "cancel" | "hang" | "descendant") {
                    arguments.extend([
                        "--fixture_ready_path".to_string(),
                        ready_path.to_string_lossy().into_owned(),
                    ]);
                }
                Route {
                    harness: *harness,
                    protocol: protocol(*harness),
                    executable: PathBuf::from(env!("CARGO_BIN_EXE_vibemux_harness_native_fixture")),
                    arguments,
                    working_directory: working_directory.clone(),
                    environment_names: vec![],
                    enabled: true,
                    allow_execution: true,
                    model: None,
                }
            })
            .collect();
        let config = Config {
            schema_version: SCHEMA_VERSION,
            limits: Limits {
                timeout_ms: 2_000,
                shutdown_ms: 500,
                ..Limits::default()
            },
            harnesses,
        };
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let dispatcher = Dispatcher::new(
            &config_path,
            &PathBuf::from(env!("CARGO_BIN_EXE_vibemux_harness_native_plugin")),
        )
        .unwrap();
        Self {
            _directory: directory,
            config_path,
            working_directory,
            ready_path,
            dispatcher,
        }
    }

    /// Rewrites the configuration and reloads the dispatcher, whose
    /// configuration digest the plugin checks.
    fn reconfigure(&mut self, edit: impl FnOnce(&mut Config)) {
        let mut config: Config =
            serde_json::from_slice(&std::fs::read(&self.config_path).unwrap()).unwrap();
        edit(&mut config);
        std::fs::write(&self.config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        self.dispatcher = Dispatcher::new(
            &self.config_path,
            &PathBuf::from(env!("CARGO_BIN_EXE_vibemux_harness_native_plugin")),
        )
        .unwrap();
    }

    fn replace_executable(&mut self, harness: Harness, executable: PathBuf) {
        self.reconfigure(|config| {
            config
                .harnesses
                .iter_mut()
                .find(|route| route.harness == harness)
                .unwrap()
                .executable = executable;
        });
    }

    async fn execute(&self, harness: Harness) -> Outcome {
        let (_sender, receiver) = watch::channel(false);
        self.dispatcher
            .execute(request(harness), receiver)
            .await
            .unwrap()
    }

    async fn cancel_after_ready(&self, sender: watch::Sender<bool>) {
        timeout(READY_TIMEOUT, async {
            while !self.ready_path.exists() {
                sleep(READY_POLL).await;
            }
        })
        .await
        .expect("fixture did not receive the prompt before readiness deadline");
        sender.send(true).unwrap();
    }
}

fn protocol(harness: Harness) -> Protocol {
    match harness {
        Harness::Codex => Protocol::Appserver,
        Harness::Claude => Protocol::Claude,
        Harness::Opencode | Harness::Copilot | Harness::Grok => Protocol::Acp,
    }
}

fn request(harness: Harness) -> Request {
    Request {
        schema_version: SCHEMA_VERSION,
        request_id: Uuid::new_v4(),
        harness,
        prompt: PROMPT.to_string(),
    }
}

async fn bounded(work: impl Future<Output = ()>) {
    timeout(TEST_TIMEOUT, work)
        .await
        .expect("process dispatch test exceeded its outer deadline");
}

fn assert_private_summary(outcome: &Outcome) {
    for public_text in [outcome.summary().to_string(), format!("{outcome:?}")] {
        for sentinel in [
            "SYNTHETIC_PROMPT_SENTINEL",
            "SYNTHETIC_SECRET_TOKEN",
            "SYNTHETIC_PRIVATE_PATH",
            "SYNTHETIC_DIAGNOSTIC_SENTINEL",
        ] {
            assert!(!public_text.contains(sentinel));
        }
    }
}

fn assert_success(outcome: &Outcome) {
    assert_eq!(outcome.completion.status, Status::Completed, "{outcome:?}");
    assert_eq!(outcome.completion.exit_code, Some(0));
    assert!(!outcome.completion.forced_termination);
    assert_eq!(outcome.completion.error_code, None);
    assert_eq!(
        outcome.completion.record_count,
        u64::try_from(outcome.records.len()).unwrap()
    );
    assert_eq!(
        outcome
            .records
            .iter()
            .filter(|record| record.kind == Kind::Text)
            .count(),
        2
    );
    assert!(
        outcome
            .records
            .iter()
            .any(|record| record.kind == Kind::Approval)
    );
    assert!(outcome.records.iter().any(|record| {
        record
            .value()
            .unwrap()
            .pointer("/fixture/permission_denied")
            == Some(&Value::Bool(true))
    }));
    let echo = outcome
        .records
        .iter()
        .find(|record| {
            record
                .value()
                .unwrap()
                .pointer("/fixture/prompt_echo")
                .is_some()
        })
        .unwrap();
    assert_eq!(echo.kind, Kind::Opaque);
    let mut expected = json!({"fixture":{"prompt_echo":PROMPT,
        "token":"SYNTHETIC_SECRET_TOKEN","path":"SYNTHETIC_PRIVATE_PATH"}});
    if outcome.harness == Harness::Claude {
        expected["type"] = json!("fixture_extension");
    } else {
        expected["method"] = json!("fixture/extension");
    }
    if protocol(outcome.harness) == Protocol::Acp {
        expected["jsonrpc"] = json!("2.0");
    }
    assert_eq!(echo.raw_json(), serde_json::to_string(&expected).unwrap());
    assert_private_summary(outcome);
}

#[tokio::test]
async fn all_five_harnesses_dispatch_preserve_prompt_and_deny_permissions() {
    bounded(async {
        let fixture = Fixture::new("normal");
        for harness in HARNESSES {
            let outcome = fixture.execute(harness).await;
            assert_eq!(outcome.harness, harness);
            assert_success(&outcome);
        }
        assert!(
            !fixture
                .working_directory
                .join("synthetic_injection_marker")
                .exists()
        );
    })
    .await;
}

#[tokio::test]
async fn probes_initialize_without_sending_a_prompt() {
    bounded(async {
        let fixture = Fixture::new("normal");
        for harness in HARNESSES {
            let outcome = fixture.dispatcher.probe(harness).await.unwrap();
            assert_eq!(outcome.completion.status, Status::Probed, "{outcome:?}");
            assert_eq!(outcome.completion.exit_code, Some(0));
            assert!(!outcome.completion.forced_termination);
            assert_eq!(outcome.records.len(), 1);
            assert_eq!(outcome.records[0].kind, Kind::Opaque);
            assert!(!outcome.records[0].raw_json().contains("prompt_echo"));
            assert_private_summary(&outcome);
        }
    })
    .await;
}

#[tokio::test]
async fn cancellation_drains_trailing_chunks_and_reaps_each_protocol() {
    bounded(async {
        for harness in DIALECT_HARNESSES {
            let fixture = Fixture::new("cancel");
            let (sender, receiver) = watch::channel(false);
            let (outcome, ()) = tokio::join!(
                fixture.dispatcher.execute(request(harness), receiver),
                fixture.cancel_after_ready(sender)
            );
            let outcome = outcome.unwrap();
            assert_eq!(outcome.completion.status, Status::Cancelled, "{outcome:?}");
            assert_eq!(outcome.completion.exit_code, Some(0));
            assert!(!outcome.completion.forced_termination);
            assert!(outcome.records.iter().any(|record| {
                record.kind == Kind::Text
                    && record
                        .raw_json()
                        .contains("synthetic trailing chunk after cancellation")
            }));
            assert_eq!(outcome.records.last().unwrap().kind, Kind::Cancelled);
            assert_private_summary(&outcome);
        }
    })
    .await;
}

#[tokio::test]
async fn native_deadline_returns_failure_and_reaps_hanging_fixture() {
    bounded(async {
        let fixture = Fixture::new("hang");
        let outcome = fixture.execute(Harness::Codex).await;
        assert_eq!(outcome.completion.status, Status::Failed, "{outcome:?}");
        assert_eq!(
            outcome.completion.error_code.as_deref(),
            Some("harness_deadline_exceeded")
        );
        assert!(outcome.completion.forced_termination);
        assert!(fixture.ready_path.exists());
        assert_private_summary(&outcome);
    })
    .await;
}

#[tokio::test]
async fn vendor_failure_does_not_become_success_for_any_protocol() {
    bounded(async {
        let fixture = Fixture::new("failed");
        for harness in DIALECT_HARNESSES {
            let outcome = fixture.execute(harness).await;
            assert_eq!(outcome.completion.status, Status::Failed, "{outcome:?}");
            assert_eq!(outcome.records.last().unwrap().kind, Kind::Failed);
            assert_private_summary(&outcome);
        }
    })
    .await;
}

#[tokio::test]
async fn malformed_json_is_rejected_without_raw_error_leakage() {
    bounded(async {
        let fixture = Fixture::new("malformed");
        let outcome = fixture.execute(Harness::Claude).await;
        assert_eq!(outcome.completion.status, Status::Failed, "{outcome:?}");
        assert_eq!(
            outcome.completion.error_code.as_deref(),
            Some("harness_invalid_json")
        );
        assert!(!outcome.summary().to_string().contains("invalid_json\n"));
        assert_private_summary(&outcome);
    })
    .await;
}

#[tokio::test]
async fn oversized_json_is_rejected_at_the_native_frame_boundary() {
    bounded(async {
        let fixture = Fixture::new("oversized");
        let outcome = fixture.execute(Harness::Opencode).await;
        assert_eq!(outcome.completion.status, Status::Failed, "{outcome:?}");
        assert_eq!(
            outcome.completion.error_code.as_deref(),
            Some("harness_frame_too_large")
        );
        assert!(
            !outcome
                .records
                .iter()
                .any(|record| record.raw_json().contains("fixture_padding"))
        );
        assert_private_summary(&outcome);
    })
    .await;
}

#[tokio::test]
async fn clean_eof_without_terminal_remains_unverified() {
    bounded(async {
        let fixture = Fixture::new("eof");
        for harness in DIALECT_HARNESSES {
            let outcome = fixture.execute(harness).await;
            assert_eq!(outcome.completion.status, Status::Unverified, "{outcome:?}");
            assert_eq!(outcome.completion.exit_code, Some(0));
            assert_eq!(
                outcome.completion.error_code.as_deref(),
                Some("harness_eof_without_terminal")
            );
            assert!(
                !outcome
                    .records
                    .iter()
                    .any(|record| record.kind == Kind::Completed)
            );
        }
    })
    .await;
}

#[tokio::test]
async fn repeated_request_id_is_rejected_by_one_dispatcher() {
    bounded(async {
        let fixture = Fixture::new("normal");
        let request = request(Harness::Codex);
        let (_first_sender, first_receiver) = watch::channel(false);
        assert_success(
            &fixture
                .dispatcher
                .execute(request.clone(), first_receiver)
                .await
                .unwrap(),
        );
        let (_second_sender, second_receiver) = watch::channel(false);
        let error = fixture
            .dispatcher
            .execute(request, second_receiver)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "harness_duplicate_request");
    })
    .await;
}

#[tokio::test]
async fn simultaneous_requests_cannot_share_a_working_directory() {
    bounded(async {
        let fixture = Fixture::new("hang");
        let (_first_sender, first_receiver) = watch::channel(false);
        let (_second_sender, second_receiver) = watch::channel(false);
        let (first, second) = tokio::join!(
            fixture
                .dispatcher
                .execute(request(Harness::Codex), first_receiver),
            fixture
                .dispatcher
                .execute(request(Harness::Claude), second_receiver),
        );
        assert_eq!(second.unwrap_err().code(), "harness_busy");
        assert_eq!(
            first.unwrap().completion.error_code.as_deref(),
            Some("harness_deadline_exceeded")
        );
    })
    .await;
}

#[tokio::test]
async fn large_stderr_is_drained_without_polluting_public_reports() {
    bounded(async {
        let fixture = Fixture::new("stderr");
        let outcome = fixture.execute(Harness::Codex).await;
        assert_success(&outcome);
        assert!(outcome.completion.stderr_bytes > 64 * 1024);
        assert!(
            !outcome
                .records
                .iter()
                .any(|record| record.raw_json().contains("SYNTHETIC_DIAGNOSTIC_SENTINEL"))
        );
    })
    .await;
}

#[tokio::test]
async fn incorrect_rpc_ids_fail_before_any_model_prompt() {
    bounded(async {
        let fixture = Fixture::new("wrong_id");
        for harness in DIALECT_HARNESSES {
            let outcome = fixture.execute(harness).await;
            let expected = if harness == Harness::Claude {
                "harness_invalid_initialize"
            } else {
                "harness_uncorrelated_response"
            };
            assert_eq!(outcome.completion.status, Status::Failed, "{outcome:?}");
            assert_eq!(outcome.completion.error_code.as_deref(), Some(expected));
            assert!(
                !outcome
                    .records
                    .iter()
                    .any(|record| record.raw_json().contains("prompt_echo"))
            );
        }
    })
    .await;
}

#[tokio::test]
async fn changed_config_is_rejected_before_probe_or_prompt() {
    bounded(async {
        let fixture = Fixture::new("hang");
        let mut config: Config =
            serde_json::from_slice(&std::fs::read(&fixture.config_path).unwrap()).unwrap();
        config.limits.record_count -= 1;
        config.validate().unwrap();
        std::fs::write(&fixture.config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let (_sender, receiver) = watch::channel(false);
        let execute_error = fixture
            .dispatcher
            .execute(request(Harness::Codex), receiver)
            .await
            .unwrap_err();
        let probe_error = fixture.dispatcher.probe(Harness::Codex).await.unwrap_err();
        for error in [execute_error, probe_error] {
            assert!(matches!(
                error.code(),
                "harness_config_changed" | "harness_plugin_start_failed"
            ));
        }
        assert!(!fixture.ready_path.exists());
    })
    .await;
}

/// A vendor executable that cannot launch fails in the plugin's runner,
/// before any completion exists. The report names that launch failure
/// rather than `harness_plugin_rejected`, for a probe and an execution. It
/// claims no forced kill, and it returns without waiting out the shutdown
/// grace period for an answer the plugin has already given.
#[tokio::test]
async fn launch_failures_keep_the_runner_code() {
    bounded(async {
        let mut fixture = Fixture::new("normal");
        fixture.reconfigure(|config| config.limits.shutdown_ms = LONG_SHUTDOWN_MS);
        let missing = fixture.working_directory.join("missing_vendor_executable");
        fixture.replace_executable(Harness::Codex, missing);
        let started = Instant::now();
        let probe = fixture.dispatcher.probe(Harness::Codex).await.unwrap();
        let probe_elapsed = started.elapsed();
        let started = Instant::now();
        let execution = fixture.execute(Harness::Codex).await;
        for (outcome, elapsed) in [(probe, probe_elapsed), (execution, started.elapsed())] {
            assert_eq!(outcome.completion.status, Status::Failed, "{outcome:?}");
            assert_eq!(
                outcome.completion.error_code.as_deref(),
                Some("harness_invalid_launch")
            );
            assert!(outcome.records.is_empty());
            assert_eq!(outcome.completion.exit_code, None);
            assert!(!outcome.completion.forced_termination);
            assert!(
                elapsed < Duration::from_millis(LONG_SHUTDOWN_MS),
                "{elapsed:?}"
            );
        }
    })
    .await;
}

#[cfg(windows)]
#[tokio::test]
async fn windows_batch_routes_are_rejected_without_launching_a_shell() {
    bounded(async {
        for extension in ["cmd", "bat"] {
            let mut fixture = Fixture::new("normal");
            let executable = fixture
                .working_directory
                .join(format!("synthetic_launcher.{extension}"));
            std::fs::write(
                &executable,
                b"@echo off\r\necho executed>synthetic_script_executed\r\n",
            )
            .unwrap();
            fixture.replace_executable(Harness::Codex, executable);
            let outcome = fixture.execute(Harness::Codex).await;
            assert_eq!(outcome.completion.status, Status::Failed, "{outcome:?}");
            assert_eq!(
                outcome.completion.error_code.as_deref(),
                Some("harness_native_executable_required")
            );
            assert!(outcome.records.is_empty());
            assert_eq!(outcome.completion.exit_code, None);
            assert!(
                !fixture
                    .working_directory
                    .join("synthetic_script_executed")
                    .exists()
            );
        }
    })
    .await;
}

#[tokio::test]
async fn native_cancellation_interrupts_an_undrained_record_queue() {
    timeout(CANCEL_TIMEOUT, async {
        let fixture = Fixture::new("normal");
        let route = Config::load(&fixture.config_path)
            .unwrap()
            .route(Harness::Codex)
            .unwrap()
            .clone();
        let limits = Limits {
            timeout_ms: BLOCKED_TIMEOUT_MS,
            shutdown_ms: CANCEL_SHUTDOWN_MS,
            ..Limits::default()
        };
        let (record_sender, record_receiver) = mpsc::channel(1);
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        let cancel = async {
            // Observe saturation without consuming the queued record.
            while record_receiver.is_empty() {
                sleep(READY_POLL).await;
            }
            sleep(CANCEL_DELAY).await;
            cancel_sender.send(true).unwrap();
        };
        let (completion, ()) = tokio::join!(
            native::run(
                route,
                limits,
                request(Harness::Codex),
                false,
                record_sender,
                cancel_receiver
            ),
            cancel,
        );
        let completion = completion.unwrap();
        assert_eq!(completion.status, Status::Cancelled);
        assert_eq!(completion.error_code.as_deref(), Some("harness_cancelled"));
        assert!(completion.forced_termination);
        assert_eq!(completion.record_count, 1);
        assert_eq!(record_receiver.len(), 1);
        #[cfg(windows)]
        assert!(
            completion.exit_code.is_some(),
            "the native child was not reaped"
        );
    })
    .await
    .expect("cancellation did not interrupt the saturated record queue");
}

#[tokio::test]
async fn native_cancellation_interrupts_a_blocked_prompt_write() {
    timeout(CANCEL_TIMEOUT, async {
        let fixture = Fixture::new("silent_input");
        let mut route = Config::load(&fixture.config_path)
            .unwrap()
            .route(Harness::Codex)
            .unwrap()
            .clone();
        route.protocol = Protocol::Codex;
        let limits = Limits {
            timeout_ms: BLOCKED_TIMEOUT_MS,
            shutdown_ms: CANCEL_SHUTDOWN_MS,
            ..Limits::default()
        };
        let mut request = request(Harness::Codex);
        request.prompt = "x".repeat(MAX_PROMPT_BYTES);
        let (record_sender, record_receiver) = mpsc::channel(1);
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        let cancel = async {
            sleep(CANCEL_DELAY).await;
            cancel_sender.send(true).unwrap();
        };
        let (completion, ()) = tokio::join!(
            native::run(
                route,
                limits,
                request,
                false,
                record_sender,
                cancel_receiver
            ),
            cancel,
        );
        let completion = completion.unwrap();
        assert_eq!(completion.status, Status::Cancelled);
        assert_eq!(completion.error_code.as_deref(), Some("harness_cancelled"));
        assert!(completion.forced_termination);
        assert_eq!(completion.record_count, 0);
        assert!(record_receiver.is_empty());
        #[cfg(windows)]
        assert!(
            completion.exit_code.is_some(),
            "the native child was not reaped"
        );
    })
    .await
    .expect("cancellation did not interrupt the blocked stdin write");
}

#[tokio::test]
async fn plugin_success_without_terminal_evidence_is_rejected() {
    bounded(async {
        let fixture = Fixture::new("spoof_empty");
        let dispatcher = Dispatcher::new(
            &fixture.config_path,
            &PathBuf::from(env!("CARGO_BIN_EXE_vibemux_harness_native_fixture")),
        )
        .unwrap();
        let (_sender, receiver) = watch::channel(false);
        let outcome = dispatcher
            .execute(request(Harness::Codex), receiver)
            .await
            .unwrap();
        assert_eq!(outcome.completion.status, Status::Failed);
        assert_eq!(
            outcome.completion.error_code.as_deref(),
            Some("harness_invalid_completion")
        );
        assert!(outcome.records.is_empty());
    })
    .await;
}

#[tokio::test]
async fn plugin_success_contradicting_failed_vendor_terminal_is_rejected() {
    bounded(async {
        let fixture = Fixture::new("spoof_failed");
        let dispatcher = Dispatcher::new(
            &fixture.config_path,
            &PathBuf::from(env!("CARGO_BIN_EXE_vibemux_harness_native_fixture")),
        )
        .unwrap();
        let (_sender, receiver) = watch::channel(false);
        let outcome = dispatcher
            .execute(request(Harness::Codex), receiver)
            .await
            .unwrap();
        assert_eq!(outcome.completion.status, Status::Failed);
        assert_eq!(
            outcome.completion.error_code.as_deref(),
            Some("harness_invalid_completion")
        );
        assert_eq!(outcome.records.len(), 5);
        assert_eq!(outcome.records.last().unwrap().kind, Kind::Failed);
    })
    .await;
}

/// `enabled` and `allow_execution` are the operator's only authorization, and
/// both the dispatcher and the plugin enforce them before anything starts.
/// Every route names a missing executable and working directory, so a request
/// that slipped past a gate would fail later with a different code; the
/// control rows show that an admitted probe does reach that later step.
#[tokio::test]
async fn route_gates_refuse_before_any_plugin_or_vendor_process_starts() {
    bounded(async {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");
        let route = |harness: Harness, enabled: bool, allow_execution: bool| Route {
            harness,
            protocol: protocol(harness),
            executable: missing.join("vendor.exe"),
            arguments: vec![],
            working_directory: missing.clone(),
            environment_names: vec![],
            enabled,
            allow_execution,
            model: None,
        };
        let config = Config {
            schema_version: SCHEMA_VERSION,
            limits: Limits::default(),
            harnesses: vec![
                route(Harness::Codex, true, false),
                route(Harness::Claude, false, true),
            ],
        };
        let config_path = directory.path().join("gate_config.json");
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let dispatcher = Dispatcher::new(
            &config_path,
            &PathBuf::from(env!("CARGO_BIN_EXE_vibemux_harness_native_plugin")),
        )
        .unwrap();
        let (_sender, cancellation) = watch::channel(false);
        let refused = |error: vibemux_harness_native::Error| error.code();
        let execute = |harness| dispatcher.execute(request(harness), cancellation.clone());
        assert_eq!(
            execute(Harness::Codex).await.map_err(refused).err(),
            Some("harness_execution_disabled")
        );
        for outcome in [
            execute(Harness::Claude).await,
            dispatcher.probe(Harness::Claude).await,
            // No route is configured for Grok at all.
            dispatcher.probe(Harness::Grok).await,
        ] {
            assert_eq!(
                outcome.map_err(refused).err(),
                Some("harness_route_unavailable")
            );
        }
        // Control: a probe needs no execution grant, so it passes both gates.
        assert_eq!(
            dispatcher
                .probe(Harness::Codex)
                .await
                .map_err(refused)
                .err(),
            Some("harness_invalid_cwd")
        );

        // The plugin re-checks the route it was handed, including a route
        // that belongs to another harness.
        for (route, harness, probe, expected) in [
            (
                route(Harness::Codex, true, false),
                Harness::Codex,
                false,
                "harness_execution_disabled",
            ),
            (
                route(Harness::Claude, false, true),
                Harness::Claude,
                false,
                "harness_route_unavailable",
            ),
            (
                route(Harness::Claude, false, true),
                Harness::Claude,
                true,
                "harness_route_unavailable",
            ),
            (
                route(Harness::Codex, true, true),
                Harness::Claude,
                true,
                "harness_route_unavailable",
            ),
            (
                route(Harness::Codex, true, false),
                Harness::Codex,
                true,
                "harness_invalid_cwd",
            ),
        ] {
            let (records, _receiver) = mpsc::channel(1);
            let result = native::run(
                route,
                Limits::default(),
                request(harness),
                probe,
                records,
                cancellation.clone(),
            )
            .await;
            assert_eq!(result.map_err(refused).err(), Some(expected));
        }
    })
    .await;
}

#[cfg(windows)]
#[tokio::test]
async fn windows_timeout_terminates_the_owned_descendant_process() {
    bounded(async {
        let fixture = Fixture::new("descendant");
        let observe_child = async {
            let child_id = timeout(READY_TIMEOUT, async {
                loop {
                    let child_id = std::fs::read_to_string(&fixture.ready_path)
                        .ok()
                        .and_then(|text| text.parse::<u32>().ok());
                    if let Some(child_id) = child_id {
                        break child_id;
                    }
                    sleep(READY_POLL).await;
                }
            })
            .await
            .expect("fixture did not publish its child PID");
            let child = windows_descendant::Held::open_fixture_child(child_id).unwrap();
            assert!(
                !child.exited().unwrap(),
                "the child must be alive before timeout"
            );
            (child_id, child)
        };
        let (outcome, (child_id, child)) =
            tokio::join!(fixture.execute(Harness::Codex), observe_child);
        assert_eq!(outcome.completion.status, Status::Failed, "{outcome:?}");
        assert_eq!(
            outcome.completion.error_code.as_deref(),
            Some("harness_deadline_exceeded")
        );
        assert!(outcome.completion.forced_termination);
        assert!(
            outcome
                .records
                .iter()
                .any(|record| record.kind == Kind::Opaque
                    && record.value().unwrap().pointer("/fixture/descendant_pid")
                        == Some(&json!(child_id)))
        );
        // The retained handle identifies the same process even if its PID is reused.
        assert!(
            child.exited().unwrap(),
            "the owned descendant survived parent cleanup"
        );
    })
    .await;
}

#[cfg(windows)]
mod windows_descendant {
    use std::{
        ffi::c_void,
        io,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };

    const PROCESS_ACCESS: u32 = 0x0010_0001;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 258;
    const CLEANUP_TIMEOUT_MS: u32 = 1_000;

    pub struct Held(OwnedHandle);

    impl Held {
        pub fn open_fixture_child(child_id: u32) -> io::Result<Self> {
            // SAFETY: the test fixture supplies this child ID; Win32 validates it and returns an owned handle.
            let handle = unsafe { OpenProcess(PROCESS_ACCESS, 0, child_id) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: OpenProcess returned one non-null owned handle; ownership transfers exactly once.
            Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle) }))
        }

        pub fn exited(&self) -> io::Result<bool> {
            // SAFETY: this retained handle is live, and a zero-time wait cannot block the test runtime.
            match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            if !matches!(self.exited(), Ok(true)) {
                // SAFETY: cleanup uses the retained fixture handle, never a fresh PID lookup or name scan.
                unsafe {
                    TerminateProcess(self.0.as_raw_handle(), 1);
                    WaitForSingleObject(self.0.as_raw_handle(), CLEANUP_TIMEOUT_MS);
                }
            }
        }
    }

    // SAFETY: declarations match kernel32; their call sites retain ownership of each handle.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn TerminateProcess(handle: *mut c_void, exit_code: u32) -> i32;
    }
}
