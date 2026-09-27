#![cfg(feature = "test_helpers")]
//! Harness dispatch service contract (ADR 029 §§4–7) against the synthetic
//! native fixture: admission gates, execution and capture for every
//! execution protocol, initialize-only probes, cancellation with and
//! without a protocol message, deadlines, process-tree teardown, shutdown,
//! restart recovery, the executable trust rules, and the rule that no
//! prompt, vendor output, or path reaches canonical state.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::time::{sleep, timeout};
use uuid::Uuid;
use vibemux_harness::{
    AgentKind, HarnessDetection,
    dispatch::{
        AttemptOutcome, DispatchError, DispatchPhase, DispatchRequest, NativeProtocol,
        events::{HARNESS_DISPATCH_ADMITTED_EVENT, HARNESS_DISPATCH_FINISHED_EVENT},
        request::{DISPATCH_REQUEST_SCHEMA_VERSION, MAX_PROMPT_BYTES},
        route_config::{MAX_ROUTE_CONFIG_BYTES, ROUTE_CONFIG_SCHEMA_VERSION},
    },
};
use vibemux_store::HarnessDispatchRecord;
use vibemuxd::{
    WriterHandle, WriterWorker,
    harness_dispatch::{
        DISPATCH_CONFIG_FILE_NAME, DispatchServiceError, DispatchServiceSettings,
        HarnessDispatchService, load_dispatch_config,
    },
};

const TRAMPOLINE: &str = env!("CARGO_BIN_EXE_vibemux_launch_trampoline");
const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_native_fixture");
/// The fixture reads its mode from this file next to its executable.
const MODE_FILE_NAME: &str = "fixture_mode.json";
const DATABASE_FILE_NAME: &str = "vibemux_rust.sqlite3";
const DETACHED_HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const SNAPSHOT_TIME: &str = "2026-09-27T00:00:00Z";
const PROMPT: &str = "synthetic prompt that must never reach canonical state";
/// Values the fixture prints inside its vendor records.
const VENDOR_SENTINELS: [&str; 2] = ["SYNTHETIC_SECRET_TOKEN", "SYNTHETIC_PRIVATE_PATH"];
const STDERR_SENTINEL: &str = "SYNTHETIC_DIAGNOSTIC_SENTINEL";
/// Lines of the sentinel the fixture's `stderr` mode prints.
const STDERR_LINES: u64 = 4096;
const DEADLINE_MS: u64 = 20_000;
const SHUTDOWN_GRACE_MS: u64 = 2_000;
/// Longer than any single attempt may take under the limits above.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(45);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One configured route.
#[derive(Clone, Copy)]
struct RouteSpec {
    harness: AgentKind,
    protocol: NativeProtocol,
    allow_execution: bool,
}

impl RouteSpec {
    const fn executing(harness: AgentKind, protocol: NativeProtocol) -> Self {
        Self {
            harness,
            protocol,
            allow_execution: true,
        }
    }
}

const CODEX_EXEC: RouteSpec = RouteSpec::executing(AgentKind::Codex, NativeProtocol::CodexExec);
const CODEX_APP_SERVER: RouteSpec =
    RouteSpec::executing(AgentKind::Codex, NativeProtocol::CodexAppServer);
const CLAUDE: RouteSpec = RouteSpec::executing(AgentKind::Claude, NativeProtocol::ClaudeStreamJson);
/// ACP routes are probe-only, so a config may not grant them execution.
const OPENCODE: RouteSpec = RouteSpec {
    harness: AgentKind::OpenCode,
    protocol: NativeProtocol::Acp,
    allow_execution: false,
};

fn default_limits() -> Value {
    json!({"deadline_ms": DEADLINE_MS, "shutdown_grace_ms": SHUTDOWN_GRACE_MS})
}

fn request(harness: AgentKind, prompt: &str) -> DispatchRequest {
    DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id: Uuid::new_v4(),
        harness,
        prompt: prompt.to_string(),
    }
}

/// The dispatch code of a refused call.
fn rejection<T: std::fmt::Debug>(result: Result<T, DispatchServiceError>) -> DispatchError {
    match result.expect_err("the service accepted the call") {
        DispatchServiceError::Dispatch(error) => error,
        other => panic!("unexpected writer error: {other}"),
    }
}

fn write_json(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec(value).expect("encode")).expect("write json");
}

/// Copies the fixture to `<name><EXE_SUFFIX>` in `directory`.
fn install_executable(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(FIXTURE, &path).expect("install fixture");
    path
}

fn config_json(routes: &[(RouteSpec, PathBuf)], limits: &Value) -> Value {
    let routes: Vec<Value> = routes
        .iter()
        .map(|(route, executable)| {
            json!({
                "harness": route.harness,
                "protocol": route.protocol,
                "executable": executable,
                "enabled": true,
                "allow_execution": route.allow_execution,
            })
        })
        .collect();
    json!({"schema_version": ROUTE_CONFIG_SCHEMA_VERSION, "limits": limits, "routes": routes})
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

async fn blocking<T: Send + 'static>(call: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(call).await.expect("join")
}

async fn detect(handle: &WriterHandle, harnesses: &[AgentKind]) {
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
    let handle = handle.clone();
    blocking(move || {
        handle.commit_harness_snapshot(
            detections,
            SNAPSHOT_TIME.to_string(),
            "dispatch_test_snapshot".to_string(),
        )
    })
    .await
    .expect("commit detection snapshot");
}

async fn wait_for_ready(path: &Path) -> String {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        // The fixture creates the file, then writes it in one call.
        if let Some(text) = std::fs::read_to_string(path)
            .ok()
            .filter(|text| !text.is_empty())
        {
            return text;
        }
        assert!(Instant::now() < deadline, "the fixture never became ready");
        sleep(POLL_INTERVAL).await;
    }
}

async fn process_gone_within(process_id: u32, limit: Duration) -> bool {
    let pid = sysinfo::Pid::from_u32(process_id);
    let deadline = Instant::now() + limit;
    let mut system = sysinfo::System::new();
    loop {
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        // An orphan the init process has not reaped yet is already dead.
        let gone = system
            .process(pid)
            .is_none_or(|process| process.status() == sysinfo::ProcessStatus::Zombie);
        if gone {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(POLL_INTERVAL).await;
    }
}

/// A project with a detached HEAD, a route config in its state directory,
/// fixture copies outside the root, a writer, and a service.
struct DispatchFixture {
    service: Option<HarnessDispatchService>,
    writer: Option<WriterWorker>,
    project_root: PathBuf,
    config_path: PathBuf,
    database: PathBuf,
    vendor_dir: PathBuf,
    launches: AtomicUsize,
    /// Dropped last: every process and handle above must be gone first.
    temp: tempfile::TempDir,
}

impl DispatchFixture {
    async fn start(routes: &[RouteSpec], detected: &[AgentKind], limits: &Value) -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let project_root = temp.path().join("project");
        let state_dir = project_root.join(".vibemux");
        let vendor_dir = temp.path().join("vendor");
        std::fs::create_dir_all(project_root.join(".git")).expect("git dir");
        std::fs::write(
            project_root.join(".git").join("HEAD"),
            format!("{DETACHED_HEAD}\n"),
        )
        .expect("head");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        std::fs::create_dir_all(&vendor_dir).expect("vendor dir");
        let installed: Vec<(RouteSpec, PathBuf)> = routes
            .iter()
            .map(|route| {
                let executable = install_executable(&vendor_dir, route.harness.command_name());
                (*route, executable)
            })
            .collect();
        let config_path = state_dir.join(DISPATCH_CONFIG_FILE_NAME);
        write_json(&config_path, &config_json(&installed, limits));
        let mut fixture = Self {
            service: None,
            writer: None,
            project_root,
            config_path,
            database: state_dir.join(DATABASE_FILE_NAME),
            vendor_dir,
            launches: AtomicUsize::new(0),
            temp,
        };
        fixture.set_mode("normal");
        let handle = fixture.open_writer().await;
        detect(&handle, detected).await;
        fixture.open_service(handle).await;
        fixture
    }

    async fn open_writer(&mut self) -> WriterHandle {
        let database = self.database.clone();
        let writer = blocking(move || WriterWorker::start(&database))
            .await
            .expect("writer");
        let handle = writer.handle().expect("handle");
        self.writer = Some(writer);
        handle
    }

    fn settings(&self) -> DispatchServiceSettings {
        DispatchServiceSettings {
            project_root: self.project_root.clone(),
            config_path: self.config_path.clone(),
            trampoline: Some(PathBuf::from(TRAMPOLINE)),
        }
    }

    async fn open_service(&mut self, handle: WriterHandle) {
        self.service = Some(HarnessDispatchService::start(handle, self.settings()).await);
    }

    fn service(&self) -> &HarnessDispatchService {
        self.service.as_ref().expect("service")
    }

    fn writer_handle(&self) -> WriterHandle {
        self.writer
            .as_ref()
            .expect("writer")
            .handle()
            .expect("handle")
    }

    /// Selects the fixture mode for the next launch and returns a fresh
    /// ready path, since the fixture creates that file exclusively.
    fn set_mode(&self, mode: &str) -> PathBuf {
        let launch = self.launches.fetch_add(1, Ordering::Relaxed);
        let ready_path = self.temp.path().join(format!("ready_{launch}"));
        write_json(
            &self.vendor_dir.join(MODE_FILE_NAME),
            &json!({"mode": mode, "ready_path": ready_path}),
        );
        ready_path
    }

    async fn submit(&self, request: &DispatchRequest) -> HarnessDispatchRecord {
        let receipt = self
            .service()
            .submit(request.clone())
            .await
            .expect("submit");
        assert!(!receipt.duplicate);
        assert_eq!(receipt.record.phase, DispatchPhase::Admitted);
        receipt.record
    }

    async fn wait_for_phase(&self, request_id: Uuid, phase: DispatchPhase) {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            let record = self.service().status(request_id).await.expect("status");
            if record.phase == phase {
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

    async fn settle(&self, request_id: Uuid) -> HarnessDispatchRecord {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            let record = self.service().status(request_id).await.expect("status");
            if record.phase.is_terminal() {
                return record;
            }
            assert!(
                Instant::now() < deadline,
                "the attempt stayed {:?}",
                record.phase
            );
            sleep(POLL_INTERVAL).await;
        }
    }

    /// Submits `mode` and waits for the terminal record.
    async fn run(&self, harness: AgentKind, mode: &str) -> HarnessDispatchRecord {
        self.set_mode(mode);
        let request = request(harness, PROMPT);
        self.submit(&request).await;
        self.settle(request.request_id).await
    }

    async fn close(&mut self) {
        if let Some(service) = self.service.take() {
            service.shutdown().await;
        }
        if let Some(writer) = self.writer.take() {
            blocking(move || writer.shutdown())
                .await
                .expect("writer shutdown");
        }
    }

    /// Raw bytes of the canonical database after [`DispatchFixture::close`].
    fn canonical_state(&self) -> Vec<u8> {
        let mut bytes = std::fs::read(&self.database).expect("database");
        let mut wal = self.database.clone().into_os_string();
        wal.push("-wal");
        if let Ok(extra) = std::fs::read(wal) {
            bytes.extend(extra);
        }
        bytes
    }

    /// No prompt, vendor record, diagnostic, or path of this layout reaches
    /// canonical state, while the dispatch events themselves are there.
    fn assert_content_free(&self) {
        let bytes = self.canonical_state();
        assert!(contains(&bytes, HARNESS_DISPATCH_FINISHED_EVENT));
        let layout = self
            .temp
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .expect("temp name");
        for needle in [PROMPT, STDERR_SENTINEL, layout]
            .into_iter()
            .chain(VENDOR_SENTINELS)
        {
            assert!(!contains(&bytes, needle), "canonical state holds {needle}");
        }
    }
}

async fn assert_completes(route: RouteSpec) {
    let mut fixture = DispatchFixture::start(&[route], &[route.harness], &default_limits()).await;
    let request = request(route.harness, PROMPT);
    fixture.submit(&request).await;
    let record = fixture.settle(request.request_id).await;
    assert_eq!(record.phase, DispatchPhase::Completed);
    assert_eq!(record.outcome, Some(AttemptOutcome::Completed));
    assert_eq!(record.error_code, None);
    assert_eq!(record.protocol, route.protocol);
    assert_eq!(record.base_commit, DETACHED_HEAD);
    let process = record.process.expect("process summary");
    assert_eq!(process.exit_code, Some(0));
    assert!(!process.forced_termination);
    let capture = record.capture.expect("capture summary");
    assert!(capture.record_count > 0);

    let service = fixture.service();
    let page = service
        .transcript(request.request_id, 0, usize::MAX)
        .await
        .expect("transcript");
    assert!(page.complete);
    assert_eq!(page.records.len() as u64, capture.record_count);
    assert!(
        page.records
            .windows(2)
            .all(|pair| pair[0].sequence() < pair[1].sequence())
    );
    // The transcript is the vendor's own output, prompt echo included.
    assert!(
        page.records
            .iter()
            .any(|record| record.raw_json().contains(PROMPT))
    );
    let first = service
        .transcript(request.request_id, 0, 1)
        .await
        .expect("first page");
    assert_eq!(first.records.len(), 1);
    let last = page.records.last().expect("records").sequence();
    let tail = service
        .transcript(request.request_id, last, 8)
        .await
        .expect("tail");
    assert!(tail.records.is_empty() && tail.complete);

    fixture.close().await;
    fixture.assert_content_free();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_exec_completes_and_keeps_its_transcript_out_of_canonical_state() {
    assert_completes(CODEX_EXEC).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_app_server_completes_after_denying_the_approval() {
    assert_completes(CODEX_APP_SERVER).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_stream_json_completes_after_denying_the_tool() {
    assert_completes(CLAUDE).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_probe_handshakes_without_a_prompt_or_canonical_state() {
    for route in [OPENCODE, CODEX_APP_SERVER, CLAUDE] {
        let mut fixture =
            DispatchFixture::start(&[route], &[route.harness], &default_limits()).await;
        let report = fixture.service().probe(route.harness).await.expect("probe");
        assert_eq!(report.harness, route.harness);
        assert_eq!(report.protocol, route.protocol);
        assert_eq!(report.outcome, AttemptOutcome::Probed);
        assert_eq!(report.error_code, None);
        assert_eq!(report.process.exit_code, Some(0));
        assert!(!report.process.forced_termination);
        assert!(report.capture.record_count >= 1);
        fixture.close().await;
        assert!(!contains(
            &fixture.canonical_state(),
            HARNESS_DISPATCH_ADMITTED_EVENT
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_acp_route_is_probe_only() {
    let mut fixture =
        DispatchFixture::start(&[OPENCODE], &[AgentKind::OpenCode], &default_limits()).await;
    let request = request(AgentKind::OpenCode, PROMPT);
    assert_eq!(
        rejection(fixture.service().submit(request.clone()).await),
        DispatchError::ExecutionDisabled
    );
    let catalog = fixture.service().catalog().expect("catalog");
    assert_eq!(catalog.len(), 1);
    assert!(catalog[0].probe_supported && !catalog[0].allow_execution);

    // A config that grants an ACP route execution is refused as a whole.
    let granting_path = fixture.config_path.with_file_name("granting_dispatch.json");
    let executable = fixture
        .vendor_dir
        .join(format!("opencode{}", std::env::consts::EXE_SUFFIX));
    let granting = RouteSpec {
        allow_execution: true,
        ..OPENCODE
    };
    write_json(
        &granting_path,
        &config_json(&[(granting, executable)], &default_limits()),
    );
    let settings = DispatchServiceSettings {
        config_path: granting_path,
        ..fixture.settings()
    };
    let service = HarnessDispatchService::start(fixture.writer_handle(), settings).await;
    assert_eq!(
        rejection(service.submit(request.clone()).await),
        DispatchError::ConfigRouteInvalid
    );
    assert_eq!(
        rejection(service.probe(AgentKind::OpenCode).await),
        DispatchError::ConfigRouteInvalid
    );
    service.shutdown().await;
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admission_gates_refuse_before_any_state_change() {
    let routes = [
        CODEX_EXEC,
        RouteSpec {
            allow_execution: false,
            ..CLAUDE
        },
    ];
    let mut fixture =
        DispatchFixture::start(&routes, &[AgentKind::Claude], &default_limits()).await;
    let service = fixture.service();
    let nil_id = DispatchRequest {
        request_id: Uuid::nil(),
        ..request(AgentKind::Codex, PROMPT)
    };
    let wrong_schema = DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION + 1,
        ..request(AgentKind::Codex, PROMPT)
    };
    let cases = [
        (
            request(AgentKind::Codex, PROMPT),
            DispatchError::NotDetected,
        ),
        (
            request(AgentKind::Claude, PROMPT),
            DispatchError::ExecutionDisabled,
        ),
        (
            request(AgentKind::Grok, PROMPT),
            DispatchError::RouteUnavailable,
        ),
        (
            request(AgentKind::Codex, " \n"),
            DispatchError::InvalidPrompt,
        ),
        (
            request(AgentKind::Codex, &"x".repeat(MAX_PROMPT_BYTES + 1)),
            DispatchError::InvalidPrompt,
        ),
        (nil_id, DispatchError::InvalidRequest),
        (wrong_schema, DispatchError::InvalidRequest),
    ];
    for (request, expected) in cases {
        let request_id = request.request_id;
        assert_eq!(rejection(service.submit(request).await), expected);
        assert_eq!(
            rejection(service.status(request_id).await),
            DispatchError::NotFound
        );
    }
    assert_eq!(
        rejection(service.probe(AgentKind::Codex).await),
        DispatchError::ProbeUnsupported
    );
    assert_eq!(
        rejection(service.probe(AgentKind::Grok).await),
        DispatchError::RouteUnavailable
    );
    assert_eq!(
        rejection(service.transcript(Uuid::new_v4(), 0, 8).await),
        DispatchError::NotFound
    );
    assert_eq!(
        rejection(service.cancel(Uuid::new_v4()).await),
        DispatchError::NotFound
    );
    let catalog = service.catalog().expect("catalog");
    let codex = catalog
        .iter()
        .find(|entry| entry.harness == AgentKind::Codex)
        .expect("codex entry");
    assert!(codex.allow_execution && !codex.probe_supported);
    let claude = catalog
        .iter()
        .find(|entry| entry.harness == AgentKind::Claude)
        .expect("claude entry");
    assert!(!claude.allow_execution && claude.probe_supported);
    fixture.close().await;
    assert!(!contains(
        &fixture.canonical_state(),
        HARNESS_DISPATCH_ADMITTED_EVENT
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_service_without_a_usable_config_refuses_dispatch_but_answers_status() {
    let mut fixture =
        DispatchFixture::start(&[CODEX_EXEC], &[AgentKind::Codex], &default_limits()).await;
    let missing = DispatchServiceSettings {
        config_path: fixture.config_path.with_file_name("missing_dispatch.json"),
        ..fixture.settings()
    };
    let invalid_path = fixture.config_path.with_file_name("invalid_dispatch.json");
    std::fs::write(&invalid_path, b"{not json").expect("invalid config");
    let invalid = DispatchServiceSettings {
        config_path: invalid_path,
        ..fixture.settings()
    };
    let services = [
        (
            HarnessDispatchService::unconfigured(fixture.writer_handle()),
            DispatchError::Unconfigured,
        ),
        (
            HarnessDispatchService::start(fixture.writer_handle(), missing).await,
            DispatchError::Unconfigured,
        ),
        (
            HarnessDispatchService::start(fixture.writer_handle(), invalid).await,
            DispatchError::ConfigInvalid,
        ),
    ];
    for (service, expected) in services {
        assert_eq!(
            rejection(service.submit(request(AgentKind::Codex, PROMPT)).await),
            expected
        );
        assert_eq!(rejection(service.probe(AgentKind::Codex).await), expected);
        assert_eq!(rejection(service.catalog()), expected);
        assert_eq!(
            rejection(service.status(Uuid::new_v4()).await),
            DispatchError::NotFound
        );
        service.shutdown().await;
    }
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repeated_request_is_a_duplicate_and_a_changed_one_conflicts() {
    let mut fixture =
        DispatchFixture::start(&[CODEX_EXEC], &[AgentKind::Codex], &default_limits()).await;
    let request = request(AgentKind::Codex, PROMPT);
    fixture.submit(&request).await;
    let settled = fixture.settle(request.request_id).await;
    assert_eq!(settled.phase, DispatchPhase::Completed);

    let repeat = fixture
        .service()
        .submit(request.clone())
        .await
        .expect("repeat");
    assert!(repeat.duplicate);
    assert_eq!(repeat.record, settled);
    let changed = DispatchRequest {
        prompt: "a different synthetic prompt".to_string(),
        ..request.clone()
    };
    assert_eq!(
        rejection(fixture.service().submit(changed).await),
        DispatchError::Conflict
    );
    assert_eq!(
        fixture
            .service()
            .status(request.request_id)
            .await
            .expect("status"),
        settled
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_repeats_of_one_request_launch_one_attempt() {
    let mut fixture =
        DispatchFixture::start(&[CODEX_EXEC], &[AgentKind::Codex], &default_limits()).await;
    let ready_path = fixture.set_mode("hang");
    let request = request(AgentKind::Codex, PROMPT);
    let service = fixture.service();
    let (first, second) = tokio::join!(
        service.submit(request.clone()),
        service.submit(request.clone())
    );
    let (first, second) = (first.expect("first"), second.expect("second"));
    assert_ne!(first.duplicate, second.duplicate);
    // A second launch would fail to create the exclusive ready file and
    // end the attempt; the one attempt keeps running instead.
    wait_for_ready(&ready_path).await;
    sleep(Duration::from_millis(200)).await;
    let record = service.status(request.request_id).await.expect("status");
    assert_eq!(record.phase, DispatchPhase::Running);
    service.cancel(request.request_id).await.expect("cancel");
    let record = fixture.settle(request.request_id).await;
    assert_eq!(record.phase, DispatchPhase::Cancelled);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_request_is_busy_and_a_cancel_without_a_protocol_message_kills_the_tree() {
    let mut fixture =
        DispatchFixture::start(&[CODEX_EXEC], &[AgentKind::Codex], &default_limits()).await;
    let ready_path = fixture.set_mode("cancel");
    let first = request(AgentKind::Codex, PROMPT);
    fixture.submit(&first).await;
    wait_for_ready(&ready_path).await;
    let service = fixture.service();
    let second = request(AgentKind::Codex, PROMPT);
    assert_eq!(
        rejection(service.submit(second.clone()).await),
        DispatchError::Busy
    );
    assert_eq!(
        rejection(service.status(second.request_id).await),
        DispatchError::NotFound
    );
    let requested = service.cancel(first.request_id).await.expect("cancel");
    assert_eq!(requested.phase, DispatchPhase::CancelRequested);
    let record = fixture.settle(first.request_id).await;
    assert_eq!(record.phase, DispatchPhase::Cancelled);
    assert!(record.process.expect("process").forced_termination);
    // Cancelling again is an idempotent repeat.
    let repeat = service.cancel(first.request_id).await.expect("repeat");
    assert_eq!(repeat, record);

    // The reservation is released, so the next request runs.
    let next = fixture.run(AgentKind::Codex, "normal").await;
    assert_eq!(next.phase, DispatchPhase::Completed);
    assert_eq!(
        rejection(service.cancel(next.request_id).await),
        DispatchError::Terminal
    );
    fixture.close().await;
}

async fn assert_confirmed_cancel(route: RouteSpec) {
    let mut fixture = DispatchFixture::start(&[route], &[route.harness], &default_limits()).await;
    let ready_path = fixture.set_mode("cancel");
    let request = request(route.harness, PROMPT);
    fixture.submit(&request).await;
    wait_for_ready(&ready_path).await;
    let requested = fixture
        .service()
        .cancel(request.request_id)
        .await
        .expect("cancel");
    assert_eq!(requested.phase, DispatchPhase::CancelRequested);
    let record = fixture.settle(request.request_id).await;
    assert_eq!(record.phase, DispatchPhase::Cancelled);
    assert_eq!(record.outcome, Some(AttemptOutcome::Cancelled));
    let process = record.process.expect("process");
    assert!(
        !process.forced_termination,
        "the vendor confirmed the cancel"
    );
    assert_eq!(process.exit_code, Some(0));
    let page = fixture
        .service()
        .transcript(request.request_id, 0, usize::MAX)
        .await
        .expect("transcript");
    assert!(
        page.records
            .iter()
            .any(|record| record.raw_json().contains("after cancellation"))
    );
    fixture.close().await;
    fixture.assert_content_free();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_confirms_an_interrupt_without_a_kill() {
    assert_confirmed_cancel(CLAUDE).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_app_server_confirms_an_interrupt_without_a_kill() {
    assert_confirmed_cancel(CODEX_APP_SERVER).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_deadline_kills_a_silent_vendor() {
    let limits = json!({"deadline_ms": 1_000, "shutdown_grace_ms": 100});
    let mut fixture =
        DispatchFixture::start(&[CODEX_APP_SERVER], &[AgentKind::Codex], &limits).await;
    let started = Instant::now();
    let record = fixture.run(AgentKind::Codex, "hang").await;
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(record.phase, DispatchPhase::Failed);
    assert_eq!(record.error_code, Some(DispatchError::DeadlineExceeded));
    assert!(record.process.expect("process").forced_termination);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protocol_violations_end_the_attempt_with_their_code() {
    let mut fixture =
        DispatchFixture::start(&[CODEX_EXEC], &[AgentKind::Codex], &default_limits()).await;
    let cases = [
        ("failed", DispatchPhase::Failed, DispatchError::TurnFailed),
        (
            "malformed",
            DispatchPhase::Failed,
            DispatchError::InvalidJson,
        ),
        (
            "oversized",
            DispatchPhase::Failed,
            DispatchError::FrameTooLarge,
        ),
        (
            "eof",
            DispatchPhase::Unverified,
            DispatchError::EofWithoutTerminal,
        ),
    ];
    for (mode, phase, code) in cases {
        let record = fixture.run(AgentKind::Codex, mode).await;
        assert_eq!(record.phase, phase, "{mode}");
        assert_eq!(record.error_code, Some(code), "{mode}");
    }
    fixture.close().await;
    fixture.assert_content_free();

    let mut fixture =
        DispatchFixture::start(&[CODEX_APP_SERVER], &[AgentKind::Codex], &default_limits()).await;
    let record = fixture.run(AgentKind::Codex, "wrong_id").await;
    assert_eq!(record.phase, DispatchPhase::Failed);
    assert_eq!(record.error_code, Some(DispatchError::UncorrelatedResponse));
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stderr_is_counted_but_never_kept() {
    let mut fixture =
        DispatchFixture::start(&[CODEX_EXEC], &[AgentKind::Codex], &default_limits()).await;
    let record = fixture.run(AgentKind::Codex, "stderr").await;
    assert_eq!(record.phase, DispatchPhase::Completed);
    let process = record.process.expect("process");
    assert!(process.stderr_bytes >= STDERR_LINES * (STDERR_SENTINEL.len() as u64 + 1));
    let page = fixture
        .service()
        .transcript(record.request_id, 0, usize::MAX)
        .await
        .expect("transcript");
    assert!(
        page.records
            .iter()
            .all(|record| !record.raw_json().contains(STDERR_SENTINEL))
    );
    fixture.close().await;
    fixture.assert_content_free();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_vendor_that_never_reads_its_prompt_still_cancels() {
    let mut fixture =
        DispatchFixture::start(&[CODEX_EXEC], &[AgentKind::Codex], &default_limits()).await;
    fixture.set_mode("silent_input");
    // Larger than a Windows pipe buffer, so the prompt write blocks.
    let request = request(AgentKind::Codex, &"p".repeat(MAX_PROMPT_BYTES));
    fixture.submit(&request).await;
    fixture
        .wait_for_phase(request.request_id, DispatchPhase::Running)
        .await;
    sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    fixture
        .service()
        .cancel(request.request_id)
        .await
        .expect("cancel");
    let record = fixture.settle(request.request_id).await;
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(record.phase, DispatchPhase::Cancelled);
    assert!(record.process.expect("process").forced_termination);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_descendant_started_by_the_vendor_ends_with_the_attempt() {
    let mut fixture =
        DispatchFixture::start(&[CLAUDE], &[AgentKind::Claude], &default_limits()).await;
    let ready_path = fixture.set_mode("descendant");
    let request = request(AgentKind::Claude, PROMPT);
    fixture.submit(&request).await;
    let descendant: u32 = wait_for_ready(&ready_path)
        .await
        .trim()
        .parse()
        .expect("descendant pid");
    fixture
        .service()
        .cancel(request.request_id)
        .await
        .expect("cancel");
    let record = fixture.settle(request.request_id).await;
    assert_eq!(record.phase, DispatchPhase::Cancelled);
    // The vendor sleeps through the interrupt, so the grace ends in a kill.
    assert!(record.process.expect("process").forced_termination);
    assert!(process_gone_within(descendant, Duration::from_secs(10)).await);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cancels_running_attempts_and_refuses_new_work() {
    let mut fixture =
        DispatchFixture::start(&[CODEX_EXEC], &[AgentKind::Codex], &default_limits()).await;
    let ready_path = fixture.set_mode("hang");
    let request = request(AgentKind::Codex, PROMPT);
    fixture.submit(&request).await;
    wait_for_ready(&ready_path).await;
    let service = fixture.service();
    timeout(Duration::from_secs(30), service.shutdown())
        .await
        .expect("shutdown within its bound");
    let record = service.status(request.request_id).await.expect("status");
    assert_eq!(record.phase, DispatchPhase::Cancelled);
    assert!(record.process.expect("process").forced_termination);
    assert_eq!(
        rejection(
            service
                .submit(self::request(AgentKind::Codex, PROMPT))
                .await
        ),
        DispatchError::Internal
    );
    assert_eq!(
        rejection(service.probe(AgentKind::Codex).await),
        DispatchError::Internal
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_leaves_a_running_attempt_pending_until_acknowledged() {
    let mut fixture =
        DispatchFixture::start(&[CODEX_EXEC], &[AgentKind::Codex], &default_limits()).await;
    let ready_path = fixture.set_mode("hang");
    let lost = request(AgentKind::Codex, PROMPT);
    fixture.submit(&lost).await;
    wait_for_ready(&ready_path).await;

    // A crash: the service vanishes without reporting the attempt, which
    // also kills its process tree.
    drop(fixture.service.take());
    let writer = fixture.writer.take().expect("writer");
    blocking(move || writer.shutdown())
        .await
        .expect("writer shutdown");

    let handle = fixture.open_writer().await;
    let recovery_handle = handle.clone();
    let report = blocking(move || recovery_handle.harness_dispatch_recovery())
        .await
        .expect("recovery report");
    assert_eq!(report.recovered, vec![lost.request_id]);
    assert!(report.quarantined.is_empty());
    fixture.open_service(handle).await;
    let service = fixture.service();
    let record = service.status(lost.request_id).await.expect("status");
    assert_eq!(record.phase, DispatchPhase::RecoveryPending);
    assert_eq!(record.error_code, Some(DispatchError::Interrupted));
    // Transcripts do not survive a restart.
    assert_eq!(
        rejection(service.transcript(lost.request_id, 0, 8).await),
        DispatchError::OutputUnavailable
    );
    // The pending attempt still holds the working directory.
    fixture.set_mode("normal");
    assert_eq!(
        rejection(service.submit(request(AgentKind::Codex, PROMPT)).await),
        DispatchError::Busy
    );
    let acknowledged = service.cancel(lost.request_id).await.expect("acknowledge");
    assert_eq!(acknowledged.phase, DispatchPhase::Cancelled);
    let next = fixture.run(AgentKind::Codex, "normal").await;
    assert_eq!(next.phase, DispatchPhase::Completed);
    fixture.close().await;
}

/// A project root, a vendor directory outside it, and a config path.
struct LoaderLayout {
    canonical_root: PathBuf,
    config_path: PathBuf,
    vendor_dir: PathBuf,
    temp: tempfile::TempDir,
}

impl LoaderLayout {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let project_root = temp.path().join("project");
        let state_dir = project_root.join(".vibemux");
        let vendor_dir = temp.path().join("vendor");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        std::fs::create_dir_all(&vendor_dir).expect("vendor dir");
        Self {
            canonical_root: std::fs::canonicalize(&project_root).expect("canonical root"),
            config_path: state_dir.join(DISPATCH_CONFIG_FILE_NAME),
            vendor_dir,
            temp,
        }
    }

    /// Loads a config with one codex route at `executable`.
    fn load_codex(&self, executable: &Path) -> Result<PathBuf, DispatchError> {
        write_json(
            &self.config_path,
            &config_json(&[(CODEX_EXEC, executable.to_path_buf())], &default_limits()),
        );
        let loaded = load_dispatch_config(&self.config_path, &self.canonical_root)?;
        loaded.executable(AgentKind::Codex).map(Path::to_path_buf)
    }
}

fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
}

#[test]
fn the_loader_pins_the_canonical_executable_named_for_its_harness() {
    let layout = LoaderLayout::new();
    let executable = install_executable(&layout.vendor_dir, "codex");
    let pinned = layout.load_codex(&executable).expect("trusted route");
    assert_eq!(
        pinned,
        std::fs::canonicalize(&executable).expect("canonical")
    );
    let loaded = load_dispatch_config(&layout.config_path, &layout.canonical_root).expect("config");
    assert_eq!(
        loaded.executable(AgentKind::Claude).err(),
        Some(DispatchError::ExecutableUnavailable)
    );
}

#[test]
fn the_loader_refuses_untrusted_executables() {
    let layout = LoaderLayout::new();
    let inside = layout.canonical_root.join("tools");
    std::fs::create_dir_all(&inside).expect("tools dir");
    let shim = layout.vendor_dir.join("codex.cmd");
    std::fs::copy(FIXTURE, &shim).expect("shim");
    let untrusted = [
        // Inside the project root, where a checkout could plant it.
        install_executable(&inside, "codex"),
        // Named for another program.
        install_executable(&layout.vendor_dir, "claude"),
        install_executable(&layout.vendor_dir, "codex_wrapper"),
        // A command-interpreter shim.
        shim,
        // Missing.
        layout
            .vendor_dir
            .join(format!("absent{}", std::env::consts::EXE_SUFFIX)),
    ];
    for executable in untrusted {
        assert_eq!(
            layout.load_codex(&executable).err(),
            Some(DispatchError::ConfigExecutableInvalid),
            "{}",
            executable.display()
        );
    }
}

#[test]
fn the_loader_checks_the_target_a_link_resolves_to() {
    let layout = LoaderLayout::new();
    let elsewhere = layout.temp.path().join("elsewhere");
    let inside = layout.canonical_root.join("tools");
    std::fs::create_dir_all(&elsewhere).expect("elsewhere");
    std::fs::create_dir_all(&inside).expect("tools dir");
    let suffix = std::env::consts::EXE_SUFFIX;
    let link = layout.vendor_dir.join(format!("codex{suffix}"));
    let trusted_target = install_executable(&elsewhere, "codex");
    if let Err(error) = symlink_file(&trusted_target, &link) {
        // Windows needs Developer Mode or a privilege to create links.
        eprintln!("skipped: cannot create a file symlink here ({error})");
        return;
    }
    assert_eq!(
        layout.load_codex(&link).expect("linked route"),
        std::fs::canonicalize(&trusted_target).expect("canonical target")
    );
    // A link named for the harness, pointing at another program or into
    // the project root.
    for (index, target) in [
        install_executable(&elsewhere, "vibemux_native_fixture"),
        install_executable(&inside, "codex"),
    ]
    .into_iter()
    .enumerate()
    {
        let link_dir = layout.vendor_dir.join(format!("link_{index}"));
        std::fs::create_dir_all(&link_dir).expect("link dir");
        let link = link_dir.join(format!("codex{suffix}"));
        symlink_file(&target, &link).expect("link");
        assert_eq!(
            layout.load_codex(&link).err(),
            Some(DispatchError::ConfigExecutableInvalid)
        );
    }
}

#[test]
fn the_loader_refuses_a_config_that_is_missing_oversized_or_not_a_regular_file() {
    let layout = LoaderLayout::new();
    let load = || load_dispatch_config(&layout.config_path, &layout.canonical_root).err();
    assert_eq!(load(), Some(DispatchError::Unconfigured));
    std::fs::write(&layout.config_path, vec![b' '; MAX_ROUTE_CONFIG_BYTES + 1]).expect("large");
    assert_eq!(load(), Some(DispatchError::ConfigTooLarge));
    std::fs::remove_file(&layout.config_path).expect("remove");
    std::fs::create_dir(&layout.config_path).expect("directory");
    assert_eq!(load(), Some(DispatchError::ConfigInvalid));
    std::fs::remove_dir(&layout.config_path).expect("remove directory");

    let executable = install_executable(&layout.vendor_dir, "codex");
    let real_config = layout.temp.path().join("real_dispatch.json");
    write_json(
        &real_config,
        &config_json(&[(CODEX_EXEC, executable)], &default_limits()),
    );
    if let Err(error) = symlink_file(&real_config, &layout.config_path) {
        eprintln!("skipped: cannot create a file symlink here ({error})");
        return;
    }
    // A linked config is refused rather than followed.
    assert_eq!(load(), Some(DispatchError::ConfigInvalid));
}
