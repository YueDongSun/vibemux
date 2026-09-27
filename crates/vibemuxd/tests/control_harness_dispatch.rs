#![cfg(feature = "test_helpers")]
//! Control v5 harness dispatch end to end (ADR 029 §7): a daemon control
//! server with a configured dispatch service, driven only through
//! `ControlClient`, against the synthetic native fixture.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::time::sleep;
use uuid::Uuid;
use vibemux_harness::{
    AgentKind, HarnessDetection,
    dispatch::{
        AttemptOutcome, DispatchPhase, DispatchRequest, NativeProtocol, Sha256Digest,
        request::DISPATCH_REQUEST_SCHEMA_VERSION, route_config::ROUTE_CONFIG_SCHEMA_VERSION,
    },
};
use vibemuxd::{
    WriterWorker,
    control::{
        ControlClient, ControlError, DaemonControlServer, HarnessDispatchOutputQuery,
        HarnessDispatchStatus,
    },
    harness_dispatch::{
        DISPATCH_CONFIG_FILE_NAME, DispatchServiceSettings, OutputCursor, OutputReassembler,
        ReassembledRecord,
    },
};

const TRAMPOLINE: &str = env!("CARGO_BIN_EXE_vibemux_launch_trampoline");
const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_native_fixture");
/// The fixture reads its mode from this file next to its executable.
const MODE_FILE_NAME: &str = "fixture_mode.json";
const DATABASE_FILE_NAME: &str = "vibemux_rust.sqlite3";
const DETACHED_HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const SNAPSHOT_TIME: &str = "2026-09-27T00:00:00Z";
/// Long enough that only a cancelled probe ends well before it.
const DEADLINE_MS: u64 = 30_000;
const SHUTDOWN_GRACE_MS: u64 = 2_000;
const SETTLE_TIMEOUT: Duration = Duration::from_secs(45);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// Bounds a reader that makes no progress.
const MAX_OUTPUT_PAGES: usize = 256;

const CODEX_EXEC: (AgentKind, NativeProtocol, bool) =
    (AgentKind::Codex, NativeProtocol::CodexExec, true);
/// ACP routes are probe-only.
const OPENCODE_ACP: (AgentKind, NativeProtocol, bool) =
    (AgentKind::OpenCode, NativeProtocol::Acp, false);

fn remote(code: &str) -> ControlError {
    ControlError::Remote {
        code: code.to_string(),
    }
}

fn write_json(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec(value).expect("encode")).expect("write json");
}

/// A running control server over a project with a detached HEAD, a route
/// config, and fixture copies outside the project root.
struct ControlFixture {
    server: Option<DaemonControlServer>,
    client: ControlClient,
    vendor_dir: PathBuf,
    /// Dropped last: the server and every fixture process must be gone.
    temp: tempfile::TempDir,
}

impl ControlFixture {
    async fn start(routes: &[(AgentKind, NativeProtocol, bool)]) -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let project_root = temp.path().join("project");
        let state_dir = project_root.join(".vibemux");
        let vendor_dir = temp.path().join("vendor");
        let runtime_dir = temp.path().join("runtime");
        for directory in [
            project_root.join(".git"),
            state_dir.clone(),
            vendor_dir.clone(),
        ] {
            std::fs::create_dir_all(directory).expect("directory");
        }
        std::fs::create_dir_all(&runtime_dir).expect("runtime dir");
        std::fs::write(
            project_root.join(".git").join("HEAD"),
            format!("{DETACHED_HEAD}\n"),
        )
        .expect("head");
        let routes: Vec<Value> = routes
            .iter()
            .map(|(harness, protocol, allow_execution)| {
                let executable = vendor_dir.join(format!(
                    "{}{}",
                    harness.command_name(),
                    std::env::consts::EXE_SUFFIX
                ));
                std::fs::copy(FIXTURE, &executable).expect("install fixture");
                json!({"harness": harness, "protocol": protocol, "executable": executable,
                    "enabled": true, "allow_execution": allow_execution})
            })
            .collect();
        let config_path = state_dir.join(DISPATCH_CONFIG_FILE_NAME);
        write_json(
            &config_path,
            &json!({"schema_version": ROUTE_CONFIG_SCHEMA_VERSION, "routes": routes,
                "limits": {"deadline_ms": DEADLINE_MS, "shutdown_grace_ms": SHUTDOWN_GRACE_MS}}),
        );
        write_json(&vendor_dir.join(MODE_FILE_NAME), &json!({"mode": "normal"}));
        let database = state_dir.join(DATABASE_FILE_NAME);
        let harnesses: Vec<AgentKind> = routes_harnesses(&config_path);
        record_detection(&database, &harnesses).await;
        let settings = DispatchServiceSettings {
            project_root,
            config_path,
            trampoline: Some(PathBuf::from(TRAMPOLINE)),
        };
        let server = DaemonControlServer::start_with_dispatch(&database, &runtime_dir, settings)
            .await
            .expect("control server");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        Self {
            server: Some(server),
            client,
            vendor_dir,
            temp,
        }
    }

    /// Selects the fixture mode for the next launch; returns its ready path.
    fn set_mode(&self, mode: &str) -> PathBuf {
        let ready_path = self.temp.path().join(format!("ready_{}", Uuid::new_v4()));
        write_json(
            &self.vendor_dir.join(MODE_FILE_NAME),
            &json!({"mode": mode, "ready_path": ready_path}),
        );
        ready_path
    }

    async fn wait_for_phase(
        &self,
        request_id: Uuid,
        phase: DispatchPhase,
    ) -> HarnessDispatchStatus {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            let status = self
                .client
                .harness_dispatch_status(request_id)
                .await
                .expect("status");
            if status.phase == phase {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the attempt stayed {:?}",
                status.phase
            );
            sleep(POLL_INTERVAL).await;
        }
    }

    /// Reads the whole transcript page by page; returns the records and the
    /// page count.
    async fn read_output(&self, request_id: Uuid) -> (Vec<ReassembledRecord>, usize) {
        let mut reassembler = OutputReassembler::starting_after(0);
        let mut records = Vec::new();
        for pages in 1..=MAX_OUTPUT_PAGES {
            let page = self
                .client
                .harness_dispatch_output(HarnessDispatchOutputQuery {
                    request_id,
                    cursor: reassembler.cursor(),
                })
                .await
                .expect("output page");
            let complete = page.complete;
            records.extend(reassembler.accept(page).expect("consistent page"));
            if complete {
                return (records, pages);
            }
        }
        panic!("the output never completed");
    }

    async fn shutdown(mut self) {
        if let Some(server) = self.server.take() {
            server.shutdown().await.expect("server shutdown");
        }
    }
}

fn routes_harnesses(config_path: &Path) -> Vec<AgentKind> {
    let config: Value =
        serde_json::from_slice(&std::fs::read(config_path).expect("config")).expect("json");
    config["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .map(|route| serde_json::from_value(route["harness"].clone()).expect("harness"))
        .collect()
}

/// Commits a detection snapshot through a writer of its own, released
/// before the server opens the database.
async fn record_detection(database: &Path, harnesses: &[AgentKind]) {
    let database = database.to_path_buf();
    let detections = harnesses
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
    tokio::task::spawn_blocking(move || {
        let writer = WriterWorker::start(&database).expect("writer");
        writer
            .handle()
            .expect("handle")
            .commit_harness_snapshot(
                detections,
                SNAPSHOT_TIME.to_string(),
                "control_dispatch_snapshot".to_string(),
            )
            .expect("commit detection snapshot");
        writer.shutdown().expect("writer shutdown");
    })
    .await
    .expect("join");
}

async fn wait_for_ready(path: &Path) {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    while !std::fs::read_to_string(path).is_ok_and(|text| !text.is_empty()) {
        assert!(Instant::now() < deadline, "the fixture never became ready");
        sleep(POLL_INTERVAL).await;
    }
}

/// About 30 KiB of text with every character class the frame escapes
/// differently, still inside the request frame.
fn escaped_prompt() -> String {
    "synthetic prompt é😀 \"quoted\" back\\slash\ttab\n".repeat(640)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dispatch_round_trips_through_control_v5() {
    let fixture = ControlFixture::start(&[CODEX_EXEC, OPENCODE_ACP]).await;
    let client = &fixture.client;

    let catalog = client.harness_dispatch_catalog().await.expect("catalog");
    assert_eq!(catalog.len(), 2);
    let codex = catalog
        .iter()
        .find(|entry| entry.harness == AgentKind::Codex)
        .expect("codex route");
    assert!(codex.enabled && codex.allow_execution && !codex.probe_supported);
    let opencode = catalog
        .iter()
        .find(|entry| entry.harness == AgentKind::OpenCode)
        .expect("opencode route");
    assert!(opencode.probe_supported && !opencode.allow_execution);

    assert_eq!(
        client.harness_dispatch_probe(AgentKind::Codex).await,
        Err(remote("harness_dispatch_probe_unsupported"))
    );
    let probe = client
        .harness_dispatch_probe(AgentKind::OpenCode)
        .await
        .expect("probe");
    assert_eq!(probe.outcome, AttemptOutcome::Probed);
    assert_eq!(probe.error_code, None);
    assert_eq!(
        client.harness_dispatch_probe(AgentKind::Claude).await,
        Err(remote("harness_dispatch_route_unavailable"))
    );

    let prompt = escaped_prompt();
    assert!(prompt.len() > 29 * 1024);
    let request = DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id: Uuid::new_v4(),
        harness: AgentKind::Codex,
        prompt: prompt.clone(),
    };
    let receipt = client
        .harness_dispatch_submit(&request)
        .await
        .expect("submit");
    assert!(!receipt.duplicate);
    assert_eq!(receipt.request_id, request.request_id);
    assert_eq!(receipt.phase, DispatchPhase::Admitted);
    let repeat = client
        .harness_dispatch_submit(&request)
        .await
        .expect("repeat");
    assert!(repeat.duplicate);
    assert_eq!(
        (repeat.task_id, repeat.run_id),
        (receipt.task_id, receipt.run_id)
    );

    let status = fixture
        .wait_for_phase(request.request_id, DispatchPhase::Completed)
        .await;
    assert_eq!(status.outcome, Some(AttemptOutcome::Completed));
    assert_eq!(status.prompt_bytes, prompt.len() as u64);
    assert_eq!(status.base_commit, DETACHED_HEAD);
    assert_eq!(status.live_capture, None);
    let capture = status.capture.expect("capture summary");

    let (records, pages) = fixture.read_output(request.request_id).await;
    assert!(pages > 1, "the echoed prompt spans pages");
    assert_eq!(records.len() as u64, capture.record_count);
    let transcript: Vec<u8> = records
        .iter()
        .flat_map(|record| record.raw_json.bytes().chain([b'\n']))
        .collect();
    assert_eq!(
        transcript.len() as u64,
        capture.record_bytes + capture.record_count
    );
    assert_eq!(Sha256Digest::of(&transcript), capture.transcript_sha256);
    // The fixture echoes the prompt in an extension record, which the
    // pages split and the reassembler must rebuild byte for byte.
    let echoed = records
        .iter()
        .find_map(|record| {
            let value: Value = serde_json::from_str(&record.raw_json).ok()?;
            value
                .pointer("/fixture/prompt_echo")?
                .as_str()
                .map(str::to_string)
        })
        .expect("prompt echo record");
    assert_eq!(echoed, prompt);

    let last = records.last().expect("records").sequence;
    assert_eq!(
        client
            .harness_dispatch_output(HarnessDispatchOutputQuery {
                request_id: request.request_id,
                cursor: OutputCursor {
                    after_sequence: last,
                    cursor_offset: 1,
                },
            })
            .await,
        Err(remote("harness_dispatch_invalid_request"))
    );
    assert_eq!(
        client.harness_dispatch_cancel(request.request_id).await,
        Err(remote("harness_dispatch_terminal"))
    );
    let unknown = Uuid::new_v4();
    assert_eq!(
        client.harness_dispatch_status(unknown).await,
        Err(remote("harness_dispatch_not_found"))
    );
    assert_eq!(
        client.harness_dispatch_cancel(unknown).await,
        Err(remote("harness_dispatch_not_found"))
    );
    assert_eq!(
        client
            .harness_dispatch_output(HarnessDispatchOutputQuery {
                request_id: unknown,
                cursor: OutputCursor::default(),
            })
            .await,
        Err(remote("harness_dispatch_not_found"))
    );
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_attempt_reports_live_capture_and_cancels() {
    let fixture = ControlFixture::start(&[CODEX_EXEC]).await;
    let ready_path = fixture.set_mode("hang");
    let request = DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id: Uuid::new_v4(),
        harness: AgentKind::Codex,
        prompt: "synthetic prompt".to_string(),
    };
    fixture
        .client
        .harness_dispatch_submit(&request)
        .await
        .expect("submit");
    wait_for_ready(&ready_path).await;
    // The fixture writes its records before the ready file.
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    let live = loop {
        let status = fixture
            .client
            .harness_dispatch_status(request.request_id)
            .await
            .expect("status");
        // thread.started, turn.started, the echo, and two text chunks.
        if let Some(live) = status.live_capture.filter(|live| live.record_count == 5) {
            assert_eq!(status.phase, DispatchPhase::Running);
            break live;
        }
        assert!(Instant::now() < deadline, "no live capture");
        sleep(POLL_INTERVAL).await;
    };
    assert!(live.record_bytes > 0);
    let cancelled = fixture
        .client
        .harness_dispatch_cancel(request.request_id)
        .await
        .expect("cancel");
    assert_eq!(cancelled.phase, DispatchPhase::CancelRequested);
    let status = fixture
        .wait_for_phase(request.request_id, DispatchPhase::Cancelled)
        .await;
    assert_eq!(status.live_capture, None);
    let (records, _) = fixture.read_output(request.request_id).await;
    assert!(records.len() as u64 >= live.record_count);
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_shutdown_cancels_a_waiting_probe_promptly() {
    let fixture = ControlFixture::start(&[OPENCODE_ACP]).await;
    let ready_path = fixture.set_mode("hang_initialize");
    let probe_client = fixture.client.clone();
    let probe = tokio::spawn(async move {
        probe_client
            .harness_dispatch_probe(AgentKind::OpenCode)
            .await
    });
    wait_for_ready(&ready_path).await;
    let started = Instant::now();
    fixture.client.shutdown().await.expect("shutdown accepted");
    let report = probe.await.expect("join").expect("probe answer");
    assert_ne!(report.outcome, AttemptOutcome::Probed);
    let mut fixture = fixture;
    fixture
        .server
        .take()
        .expect("server")
        .wait()
        .await
        .expect("server stopped");
    // Without the early signal the drain would wait out the probe deadline.
    assert!(
        started.elapsed() < Duration::from_millis(DEADLINE_MS / 2),
        "shutdown took {:?}",
        started.elapsed()
    );
}
