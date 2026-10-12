#![cfg(feature = "test_helpers")]
//! Decision routing service contract (ADR 034) against the synthetic
//! decision peer and the synthetic native harness: the two external gates,
//! the credential and environment rules of the peer, answer validation and
//! the fallback chain, the peer deadline, and decided dispatch, which
//! re-runs every ADR 029 gate and consumes a decision exactly once.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::time::{sleep, timeout};
use uuid::Uuid;
use vibemux_decision::{DecisionSource, DecisionStatus, ExternalError, ExternalStatus, OptionId};
use vibemux_harness::{
    AgentKind, HarnessDetection,
    dispatch::{DispatchPhase, NativeProtocol, route_config::ROUTE_CONFIG_SCHEMA_VERSION},
};
use vibemux_store::StoredDecision;
use vibemuxd::{
    WriterHandle, WriterWorker,
    control::{
        ControlClient, ControlError, DaemonControlServer, DecisionAdviseArgument, DecisionView,
        ExecutionSettings,
    },
    decision::{
        AdviseRequest, DecisionService, DecisionServiceError, DecisionServiceSettings,
        SHUTDOWN_DEADLINE, decision_config_path,
    },
    harness_dispatch::{
        DISPATCH_CONFIG_FILE_NAME, DispatchServiceSettings, HarnessDispatchService,
    },
};

const TRAMPOLINE: &str = env!("CARGO_BIN_EXE_vibemux_launch_trampoline");
const NATIVE_FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_native_fixture");
const DECISION_FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_decision_fixture");
const MODE_FILE_NAME: &str = "fixture_mode.json";
const DATABASE_FILE_NAME: &str = "vibemux_rust.sqlite3";
const DETACHED_HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const SNAPSHOT_TIME: &str = "2026-10-12T00:00:00Z";
const PROMPT: &str = "synthetic routed prompt that must never reach canonical state";
/// Set by cargo for every test process, so it can stand in for a credential
/// without the test writing its own environment.
const PRESENT_CREDENTIAL: &str = "CARGO_PKG_NAME";
const ABSENT_CREDENTIAL: &str = "DECISION_TEST_ABSENT_CREDENTIAL_1F2E";
/// Long enough that a spawned hanging peer would exceed [`CALL_LIMIT`].
const LONG_PEER_DEADLINE: Duration = Duration::from_secs(60);
const SHORT_PEER_DEADLINE: Duration = Duration::from_millis(1500);
/// Bound on one `advise` call that must not wait for a peer.
const CALL_LIMIT: Duration = Duration::from_secs(20);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(45);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

const ROUTED: [(AgentKind, NativeProtocol); 2] = [
    (AgentKind::Claude, NativeProtocol::ClaudeStreamJson),
    (AgentKind::Codex, NativeProtocol::CodexExec),
];

/// The `rust` label routes to Claude locally; an external answer always
/// picks the last harness option, so the two sources stay distinguishable.
fn decision_config(enabled: bool, request_model: &str, credential_env: &str) -> Value {
    json!({
        "schema_version": 1,
        "labels": ["rust", "docs"],
        "local_rules": [{"label": "rust", "harness": "claude", "priority": 1}],
        "jev": {
            "enabled": enabled,
            "request_model": request_model,
            "pinned_model": "jev-1.13.0",
            "credential_env": credential_env,
        },
    })
}

fn write_json(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec(value).expect("encode")).expect("write json");
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

async fn blocking<T: Send + 'static>(call: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(call).await.expect("join")
}

fn advise(labels: &[&str], allow_external: bool) -> AdviseRequest {
    AdviseRequest {
        prompt: PROMPT.to_string(),
        labels: labels.iter().map(|label| (*label).to_string()).collect(),
        allow_external,
    }
}

/// The harness option the fixture peer answers with.
fn last_harness(decision: &StoredDecision) -> AgentKind {
    decision
        .record
        .options
        .iter()
        .rev()
        .find_map(OptionId::to_harness)
        .expect("a harness option")
}

fn external_error(decision: &StoredDecision) -> Option<ExternalError> {
    decision.record.external.error
}

fn processes_running(executable: &Path) -> usize {
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    system
        .processes()
        .values()
        .filter(|process| process.exe() == Some(executable))
        .filter(|process| process.status() != sysinfo::ProcessStatus::Zombie)
        .count()
}

async fn wait_for_peer(executable: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while processes_running(executable) == 0 {
        assert!(Instant::now() < deadline, "the peer never started");
        sleep(POLL_INTERVAL).await;
    }
}

/// The peer was reaped before shutdown returned; the bound only absorbs the
/// process table catching up.
async fn assert_peer_gone(executable: &Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while processes_running(executable) > 0 {
        assert!(Instant::now() < deadline, "the peer outlived shutdown");
        sleep(POLL_INTERVAL).await;
    }
}

/// A project with Claude and Codex dispatch routes on the native fixture,
/// both detected, a writer, a dispatch service, and a decision peer copy.
struct RouteFixture {
    decision: Option<DecisionService>,
    dispatch: Option<HarnessDispatchService>,
    writer: Option<WriterWorker>,
    state_dir: PathBuf,
    database: PathBuf,
    vendor_dir: PathBuf,
    peer: PathBuf,
    launches: AtomicUsize,
    snapshots: AtomicUsize,
    /// Dropped last: every process and handle above must be gone first.
    temp: tempfile::TempDir,
}

impl RouteFixture {
    async fn start() -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let project_root = temp.path().join("project");
        let state_dir = project_root.join(".vibemux");
        let vendor_dir = temp.path().join("vendor");
        let peer_dir = temp.path().join("peer");
        std::fs::create_dir_all(project_root.join(".git")).expect("git dir");
        std::fs::write(
            project_root.join(".git").join("HEAD"),
            format!("{DETACHED_HEAD}\n"),
        )
        .expect("head");
        for directory in [&state_dir, &vendor_dir, &peer_dir] {
            std::fs::create_dir_all(directory).expect("directory");
        }
        let routes: Vec<Value> = ROUTED
            .iter()
            .map(|(harness, protocol)| {
                let executable = vendor_dir.join(format!(
                    "{}{}",
                    harness.command_name(),
                    std::env::consts::EXE_SUFFIX
                ));
                std::fs::copy(NATIVE_FIXTURE, &executable).expect("install native fixture");
                json!({
                    "harness": harness,
                    "protocol": protocol,
                    "executable": executable,
                    "enabled": true,
                    "allow_execution": true,
                })
            })
            .collect();
        let dispatch_config_path = state_dir.join(DISPATCH_CONFIG_FILE_NAME);
        write_json(
            &dispatch_config_path,
            &json!({
                "schema_version": ROUTE_CONFIG_SCHEMA_VERSION,
                "limits": {"deadline_ms": 20_000, "shutdown_grace_ms": 2_000},
                "routes": routes,
            }),
        );
        let peer = peer_dir.join(format!(
            "vibemux_decision_peer{}",
            std::env::consts::EXE_SUFFIX
        ));
        std::fs::copy(DECISION_FIXTURE, &peer).expect("install decision fixture");
        let database = state_dir.join(DATABASE_FILE_NAME);
        let writer = blocking({
            let database = database.clone();
            move || WriterWorker::start(&database)
        })
        .await
        .expect("writer");
        let handle = writer.handle().expect("handle");
        let dispatch = HarnessDispatchService::start(
            handle,
            DispatchServiceSettings {
                project_root,
                config_path: dispatch_config_path,
                trampoline: Some(PathBuf::from(TRAMPOLINE)),
            },
        )
        .await;
        let fixture = Self {
            decision: None,
            dispatch: Some(dispatch),
            writer: Some(writer),
            state_dir,
            database,
            vendor_dir,
            peer,
            launches: AtomicUsize::new(0),
            snapshots: AtomicUsize::new(0),
            temp,
        };
        fixture.detect(&[AgentKind::Claude, AgentKind::Codex]).await;
        fixture
    }

    fn handle(&self) -> WriterHandle {
        self.writer
            .as_ref()
            .expect("writer")
            .handle()
            .expect("handle")
    }

    async fn detect(&self, harnesses: &[AgentKind]) {
        let detections: BTreeMap<String, HarnessDetection> = harnesses
            .iter()
            .map(|harness| {
                let detection = HarnessDetection {
                    detected: true,
                    path: None,
                    launcher: None,
                    version: Some("1.0.0".to_string()),
                };
                (harness.command_name().to_string(), detection)
            })
            .collect();
        let snapshot = self.snapshots.fetch_add(1, Ordering::Relaxed);
        let handle = self.handle();
        blocking(move || {
            handle.commit_harness_snapshot(
                detections,
                SNAPSHOT_TIME.to_string(),
                format!("decision_test_snapshot_{snapshot}"),
            )
        })
        .await
        .expect("commit detection snapshot");
    }

    async fn switch_default(&self, harness: AgentKind) {
        let handle = self.handle();
        blocking(move || {
            handle.commit_harness_switch(
                harness.command_name().to_string(),
                "decision_test_switch".to_string(),
            )
        })
        .await
        .expect("switch default");
    }

    /// Writes `config` and (re)starts the decision service, which pins the
    /// config at start.
    async fn configure(&mut self, config: &Value, peer: Option<PathBuf>, deadline: Duration) {
        let config_path = decision_config_path(&self.state_dir);
        write_json(&config_path, config);
        let settings = DecisionServiceSettings {
            config_path,
            peer,
            peer_deadline: deadline,
        };
        let dispatch = self.dispatch.clone().expect("dispatch");
        self.decision = Some(DecisionService::start(self.handle(), dispatch, settings).await);
    }

    /// Configures the fixture peer in `request_model` mode with the
    /// credential present and the long deadline, then decides once.
    async fn decide_external(&mut self, request_model: &str, labels: &[&str]) -> StoredDecision {
        let config = decision_config(true, request_model, PRESENT_CREDENTIAL);
        self.configure(&config, Some(self.peer.clone()), LONG_PEER_DEADLINE)
            .await;
        self.decide(labels, true).await
    }

    fn service(&self) -> &DecisionService {
        self.decision.as_ref().expect("decision service")
    }

    async fn decide(&self, labels: &[&str], allow_external: bool) -> StoredDecision {
        timeout(
            CALL_LIMIT,
            self.service().advise(advise(labels, allow_external)),
        )
        .await
        .expect("advise returned in time")
        .expect("advise")
    }

    /// Selects the native fixture's mode for the next launch.
    fn set_native_mode(&self) {
        let launch = self.launches.fetch_add(1, Ordering::Relaxed);
        let ready_path = self.temp.path().join(format!("ready_{launch}"));
        write_json(
            &self.vendor_dir.join(MODE_FILE_NAME),
            &json!({"mode": "normal", "ready_path": ready_path}),
        );
    }

    async fn settle(&self, request_id: Uuid) {
        let dispatch = self.dispatch.as_ref().expect("dispatch");
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            let record = dispatch.status(request_id).await.expect("status");
            if record.phase.is_terminal() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the attempt stayed {:?}",
                record.phase
            );
            sleep(POLL_INTERVAL).await;
        }
    }

    async fn close(&mut self) {
        if let Some(decision) = self.decision.take() {
            decision.shutdown().await;
        }
        if let Some(dispatch) = self.dispatch.take() {
            dispatch.shutdown().await;
        }
        if let Some(writer) = self.writer.take() {
            blocking(move || writer.shutdown())
                .await
                .expect("writer shutdown");
        }
    }

    /// The prompt and this layout's paths never reach canonical state.
    fn assert_content_free(&self) {
        let mut bytes = std::fs::read(&self.database).expect("database");
        let mut wal = self.database.clone().into_os_string();
        wal.push("-wal");
        if let Ok(extra) = std::fs::read(wal) {
            bytes.extend(extra);
        }
        assert!(contains(&bytes, "decision_recorded"));
        let layout = self
            .temp
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .expect("temp name");
        for needle in [PROMPT, layout] {
            assert!(!contains(&bytes, needle), "canonical state holds {needle}");
        }
    }
}

fn code<T: std::fmt::Debug>(result: Result<T, DecisionServiceError>) -> String {
    result
        .expect_err("the call was accepted")
        .code()
        .to_string()
}

#[tokio::test]
async fn local_rules_the_project_default_and_escalation_decide_without_a_backend() {
    let mut fixture = RouteFixture::start().await;
    let config = decision_config(false, "jev-latest", PRESENT_CREDENTIAL);
    fixture
        .configure(&config, Some(fixture.peer.clone()), LONG_PEER_DEADLINE)
        .await;

    let ruled = fixture.decide(&["rust"], false).await;
    assert_eq!(ruled.record.status, DecisionStatus::Resolved);
    assert_eq!(ruled.record.source, DecisionSource::LocalRule);
    assert_eq!(ruled.record.chosen_harness, Some(AgentKind::Claude));
    assert_eq!(ruled.record.external.status, ExternalStatus::NotRequested);

    let escalated = fixture.decide(&["docs"], false).await;
    assert_eq!(escalated.record.status, DecisionStatus::Escalated);
    assert_eq!(escalated.record.chosen_harness, None);

    fixture.switch_default(AgentKind::Codex).await;
    let defaulted = fixture.decide(&[], false).await;
    assert_eq!(defaulted.record.source, DecisionSource::ProjectDefault);
    assert_eq!(defaulted.record.chosen_harness, Some(AgentKind::Codex));

    let unknown = fixture.service().advise(advise(&["unlisted"], false)).await;
    assert_eq!(code(unknown), "decision_unknown_label");
    let mut empty = advise(&[], false);
    empty.prompt = " ".to_string();
    assert_eq!(
        code(fixture.service().advise(empty).await),
        "decision_invalid_request"
    );

    let listed = fixture.service().list(0, 10).await.expect("list");
    let ids: Vec<Uuid> = listed
        .iter()
        .map(|stored| stored.record.decision_id)
        .collect();
    assert_eq!(
        ids,
        [
            ruled.record.decision_id,
            escalated.record.decision_id,
            defaulted.record.decision_id
        ]
    );
    let shown = fixture
        .service()
        .show(ruled.record.decision_id)
        .await
        .expect("show");
    assert_eq!(shown, ruled);
    assert_eq!(
        code(fixture.service().show(Uuid::new_v4()).await),
        "decision_not_found"
    );
    assert_eq!(
        code(fixture.service().list(0, 0).await),
        "decision_invalid_request"
    );
    fixture.close().await;
    fixture.assert_content_free();
}

#[tokio::test]
async fn either_gate_off_or_a_missing_credential_never_spawns_the_peer() {
    let mut fixture = RouteFixture::start().await;
    // A spawned peer in this mode would hold the call past `CALL_LIMIT`.
    let peer = Some(fixture.peer.clone());

    let disabled = decision_config(false, "fixture_hang", PRESENT_CREDENTIAL);
    fixture
        .configure(&disabled, peer.clone(), LONG_PEER_DEADLINE)
        .await;
    let off = fixture.decide(&["rust"], true).await;
    assert_eq!(off.record.external.status, ExternalStatus::Disabled);
    assert_eq!(off.record.source, DecisionSource::LocalRule);

    let enabled = decision_config(true, "fixture_hang", PRESENT_CREDENTIAL);
    fixture
        .configure(&enabled, peer.clone(), LONG_PEER_DEADLINE)
        .await;
    let not_requested = fixture.decide(&["rust"], false).await;
    assert_eq!(
        not_requested.record.external.status,
        ExternalStatus::NotRequested
    );

    let absent = decision_config(true, "fixture_hang", ABSENT_CREDENTIAL);
    fixture.configure(&absent, peer, LONG_PEER_DEADLINE).await;
    let missing = fixture.decide(&["rust"], true).await;
    assert_eq!(missing.record.external.status, ExternalStatus::Failed);
    assert_eq!(
        external_error(&missing),
        Some(ExternalError::CredentialUnavailable)
    );
    assert_eq!(missing.record.chosen_harness, Some(AgentKind::Claude));
    assert_eq!(processes_running(&fixture.peer), 0);
    fixture.close().await;
}

#[tokio::test]
async fn only_the_system_variables_and_the_credential_reach_the_peer() {
    let mut fixture = RouteFixture::start().await;
    let decided = fixture.decide_external("fixture_env", &[]).await;
    assert_eq!(
        decided.record.external.status,
        ExternalStatus::Accepted,
        "the peer saw an unexpected environment: {:?}",
        external_error(&decided)
    );
    assert_eq!(decided.record.source, DecisionSource::External);
    fixture.close().await;
}

#[tokio::test]
async fn a_confident_valid_answer_applies_and_any_other_falls_back() {
    let mut fixture = RouteFixture::start().await;

    let accepted = fixture.decide_external("fixture_valid", &["rust"]).await;
    assert_eq!(accepted.record.external.status, ExternalStatus::Accepted);
    assert_eq!(accepted.record.source, DecisionSource::External);
    assert_eq!(
        accepted.record.chosen_harness,
        Some(last_harness(&accepted))
    );
    let answer = accepted.record.external.answer.as_ref().expect("evidence");
    assert_eq!(answer.model, "jev-1.13.0");

    let escalated = fixture.decide_external("fixture_escalate", &["rust"]).await;
    assert_eq!(escalated.record.external.status, ExternalStatus::Accepted);
    assert_eq!(escalated.record.status, DecisionStatus::Escalated);

    let low = fixture.decide_external("fixture_low", &["rust"]).await;
    assert_eq!(low.record.external.status, ExternalStatus::BelowThreshold);
    assert_eq!(low.record.source, DecisionSource::LocalRule);
    assert!(low.record.external.answer.is_some(), "evidence is kept");

    let cases = [
        (
            "fixture_model",
            ExternalStatus::Rejected,
            ExternalError::AnswerModelMismatch,
        ),
        (
            "fixture_failed",
            ExternalStatus::Failed,
            ExternalError::RateLimited,
        ),
        (
            "fixture_malformed",
            ExternalStatus::Failed,
            ExternalError::PeerProtocol,
        ),
        (
            "fixture_oversized",
            ExternalStatus::Failed,
            ExternalError::PeerProtocol,
        ),
    ];
    for (mode, status, error) in cases {
        let decided = fixture.decide_external(mode, &["rust"]).await;
        assert_eq!(decided.record.external.status, status, "{mode}");
        assert_eq!(external_error(&decided), Some(error), "{mode}");
        assert_eq!(decided.record.source, DecisionSource::LocalRule, "{mode}");
        assert!(decided.record.external.answer.is_none(), "{mode}");
    }
    fixture.close().await;
    fixture.assert_content_free();
}

#[tokio::test]
async fn a_hung_peer_is_killed_at_the_deadline() {
    let mut fixture = RouteFixture::start().await;
    let config = decision_config(true, "fixture_hang", PRESENT_CREDENTIAL);
    fixture
        .configure(&config, Some(fixture.peer.clone()), SHORT_PEER_DEADLINE)
        .await;
    let started = Instant::now();
    let decided = fixture.decide(&["rust"], true).await;
    assert!(started.elapsed() >= SHORT_PEER_DEADLINE);
    assert_eq!(external_error(&decided), Some(ExternalError::PeerTimeout));
    assert_eq!(decided.record.source, DecisionSource::LocalRule);
    let deadline = Instant::now() + Duration::from_secs(10);
    while processes_running(&fixture.peer) > 0 {
        assert!(Instant::now() < deadline, "the hung peer is still running");
        sleep(Duration::from_millis(50)).await;
    }
    fixture.close().await;
}

#[tokio::test]
async fn a_second_external_request_falls_back_at_once_instead_of_queueing() {
    let mut fixture = RouteFixture::start().await;
    let config = decision_config(true, "fixture_hang", PRESENT_CREDENTIAL);
    fixture
        .configure(&config, Some(fixture.peer.clone()), SHORT_PEER_DEADLINE)
        .await;
    let service = fixture.service().clone();
    let first = tokio::spawn(async move { service.advise(advise(&["rust"], true)).await });
    let deadline = Instant::now() + Duration::from_secs(10);
    while processes_running(&fixture.peer) == 0 {
        assert!(Instant::now() < deadline, "the first peer never started");
        sleep(POLL_INTERVAL).await;
    }

    let started = Instant::now();
    let second = fixture.decide(&["rust"], true).await;
    assert!(
        started.elapsed() < SHORT_PEER_DEADLINE,
        "the second request waited for the first exchange"
    );
    assert_eq!(external_error(&second), Some(ExternalError::PeerBusy));
    assert_eq!(second.record.external.status, ExternalStatus::Failed);
    assert_eq!(second.record.source, DecisionSource::LocalRule);
    assert!(
        processes_running(&fixture.peer) <= 1,
        "a second peer started"
    );

    let first = first.await.expect("join").expect("first advice");
    assert_eq!(external_error(&first), Some(ExternalError::PeerTimeout));
    fixture.close().await;
}

#[tokio::test]
async fn shutdown_kills_and_reaps_a_hung_peer_and_refuses_new_exchanges() {
    let mut fixture = RouteFixture::start().await;
    let config = decision_config(true, "fixture_hang", PRESENT_CREDENTIAL);
    fixture
        .configure(&config, Some(fixture.peer.clone()), LONG_PEER_DEADLINE)
        .await;
    let service = fixture.service().clone();
    let pending = tokio::spawn(async move { service.advise(advise(&["rust"], true)).await });
    wait_for_peer(&fixture.peer).await;

    let started = Instant::now();
    fixture.service().shutdown().await;
    assert!(
        started.elapsed() < SHUTDOWN_DEADLINE,
        "shutdown waited for the peer deadline"
    );
    assert_peer_gone(&fixture.peer).await;
    let cancelled = timeout(CALL_LIMIT, pending)
        .await
        .expect("advice returned in time")
        .expect("join")
        .expect("advice");
    assert_eq!(
        external_error(&cancelled),
        Some(ExternalError::PeerCancelled)
    );
    assert_eq!(cancelled.record.external.status, ExternalStatus::Failed);
    assert_eq!(cancelled.record.source, DecisionSource::LocalRule);

    // After shutdown an external request falls back without a peer.
    let refused = fixture.decide(&["rust"], true).await;
    assert_eq!(external_error(&refused), Some(ExternalError::PeerCancelled));
    assert_eq!(refused.record.source, DecisionSource::LocalRule);
    assert_eq!(processes_running(&fixture.peer), 0);
    fixture.close().await;
    fixture.assert_content_free();
}

/// Hands the fixture's database to a control server whose decision config
/// enables the hanging peer; the fixture's own writer is released first.
async fn hanging_control_server(
    fixture: &mut RouteFixture,
    runtime: &str,
) -> (DaemonControlServer, ControlClient) {
    let config_path = decision_config_path(&fixture.state_dir);
    write_json(
        &config_path,
        &decision_config(true, "fixture_hang", PRESENT_CREDENTIAL),
    );
    fixture.close().await;
    let runtime_dir = fixture.temp.path().join(runtime);
    std::fs::create_dir_all(&runtime_dir).expect("runtime dir");
    let settings = ExecutionSettings {
        dispatch: DispatchServiceSettings {
            project_root: fixture
                .state_dir
                .parent()
                .expect("project root")
                .to_path_buf(),
            config_path: fixture.state_dir.join(DISPATCH_CONFIG_FILE_NAME),
            trampoline: Some(PathBuf::from(TRAMPOLINE)),
        },
        workflow: None,
        decision: Some(DecisionServiceSettings {
            config_path,
            peer: Some(fixture.peer.clone()),
            peer_deadline: LONG_PEER_DEADLINE,
        }),
    };
    let server =
        DaemonControlServer::start_with_execution(&fixture.database, &runtime_dir, settings)
            .await
            .expect("control server");
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    (server, client)
}

/// Starts an external advice over control IPC and waits for its peer.
async fn advise_over_control(
    client: &ControlClient,
    peer: &Path,
) -> tokio::task::JoinHandle<Result<DecisionView, ControlError>> {
    let client = client.clone();
    let advising = tokio::spawn(async move {
        client
            .decision_advise(&DecisionAdviseArgument {
                prompt: PROMPT.to_string(),
                labels: vec!["rust".to_string()],
                allow_external: true,
            })
            .await
    });
    wait_for_peer(peer).await;
    advising
}

#[tokio::test]
async fn daemon_shutdown_cancels_an_external_decision_in_flight() {
    let mut fixture = RouteFixture::start().await;

    // A control shutdown cancels the peer, so the drained advice falls back
    // instead of holding the shutdown for the peer deadline.
    let (server, client) = hanging_control_server(&mut fixture, "runtime_control").await;
    let advising = advise_over_control(&client, &fixture.peer).await;
    let started = Instant::now();
    client.shutdown().await.expect("shutdown accepted");
    let view = timeout(CALL_LIMIT, advising)
        .await
        .expect("advice returned in time")
        .expect("join")
        .expect("advice");
    server.wait().await.expect("server stopped");
    assert!(
        started.elapsed() < CALL_LIMIT,
        "shutdown waited for the peer deadline"
    );
    assert_eq!(view.external.error, Some(ExternalError::PeerCancelled));
    assert_eq!(view.source, DecisionSource::LocalRule);
    assert_peer_gone(&fixture.peer).await;

    // Stopping the daemon reaps the peer before it aborts the connection.
    let (server, client) = hanging_control_server(&mut fixture, "runtime_stop").await;
    let advising = advise_over_control(&client, &fixture.peer).await;
    let started = Instant::now();
    server.shutdown().await.expect("server stopped");
    assert!(
        started.elapsed() < CALL_LIMIT,
        "the stop waited for the peer deadline"
    );
    assert_peer_gone(&fixture.peer).await;
    // The aborted connection ends the client call either way.
    let _ = timeout(CALL_LIMIT, advising)
        .await
        .expect("the client call ended");
    fixture.assert_content_free();
}

#[tokio::test]
async fn a_missing_peer_is_peer_unavailable_and_falls_back() {
    let mut fixture = RouteFixture::start().await;
    let config = decision_config(true, "fixture_valid", PRESENT_CREDENTIAL);
    let missing = fixture.temp.path().join("absent_peer");
    fixture
        .configure(&config, Some(missing), LONG_PEER_DEADLINE)
        .await;
    let decided = fixture.decide(&["rust"], true).await;
    assert_eq!(decided.record.external.status, ExternalStatus::Failed);
    assert_eq!(
        external_error(&decided),
        Some(ExternalError::PeerUnavailable)
    );
    assert_eq!(decided.record.chosen_harness, Some(AgentKind::Claude));
    fixture.close().await;
}

#[tokio::test]
async fn a_decided_dispatch_rechecks_every_gate_and_consumes_the_decision_once() {
    let mut fixture = RouteFixture::start().await;
    let config = decision_config(false, "jev-latest", PRESENT_CREDENTIAL);
    fixture
        .configure(&config, Some(fixture.peer.clone()), LONG_PEER_DEADLINE)
        .await;
    let decided = fixture.decide(&["rust"], false).await;
    let decision_id = decided.record.decision_id;
    let service = fixture.service().clone();

    let changed_prompt = service
        .dispatch(decision_id, Uuid::new_v4(), format!("{PROMPT} changed"))
        .await;
    assert_eq!(code(changed_prompt), "decision_prompt_mismatch");

    fixture.detect(&[AgentKind::Codex]).await;
    let undetected = service
        .dispatch(decision_id, Uuid::new_v4(), PROMPT.to_string())
        .await;
    assert_eq!(code(undetected), "harness_dispatch_not_detected");
    fixture.detect(&[AgentKind::Claude, AgentKind::Codex]).await;
    let unconsumed = service.show(decision_id).await.expect("show");
    assert!(unconsumed.record.consumption.is_none());

    fixture.set_native_mode();
    let request_id = Uuid::new_v4();
    let receipt = service
        .dispatch(decision_id, request_id, PROMPT.to_string())
        .await
        .expect("decided dispatch");
    assert!(!receipt.duplicate);
    assert_eq!(receipt.record.harness, AgentKind::Claude);
    assert_eq!(receipt.record.phase, DispatchPhase::Admitted);
    fixture.settle(request_id).await;

    let consumed = service.show(decision_id).await.expect("show");
    let consumption = consumed.record.consumption.expect("consumed");
    assert_eq!(consumption.request_id, request_id);
    let replay = service
        .dispatch(decision_id, request_id, PROMPT.to_string())
        .await
        .expect("replay");
    assert!(replay.duplicate);
    let reused = service
        .dispatch(decision_id, Uuid::new_v4(), PROMPT.to_string())
        .await;
    assert_eq!(code(reused), "decision_already_consumed");

    let escalated = fixture.decide(&["docs"], false).await;
    let refused = service
        .dispatch(
            escalated.record.decision_id,
            Uuid::new_v4(),
            PROMPT.to_string(),
        )
        .await;
    assert_eq!(code(refused), "decision_escalated");
    assert_eq!(
        code(
            service
                .dispatch(Uuid::new_v4(), Uuid::new_v4(), PROMPT.to_string())
                .await
        ),
        "decision_not_found"
    );
    fixture.close().await;
    fixture.assert_content_free();
}
