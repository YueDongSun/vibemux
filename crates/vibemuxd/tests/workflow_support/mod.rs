//! Shared fixture for the dual-track workflow tests (ADR 031).
//!
//! Each fixture is a TaskBoard Lite git project in a temporary directory,
//! the trusted verifier copied outside the project (plus a fast syntax
//! suite), scripted worker fixtures installed as `codex` and `claude` (and
//! an ACP `opencode` route when a test needs a non-executing slot), and a
//! control server with the harness dispatch and workflow services, driven
//! only through `ControlClient`. Everything here is synthetic evidence: it
//! proves the daemon's orchestration, never a vendor model's behavior.
#![allow(dead_code)]

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio::time::sleep;
use uuid::Uuid;
use vibemux_harness::{
    AgentKind, HarnessDetection,
    dispatch::{NativeProtocol, route_config::ROUTE_CONFIG_SCHEMA_VERSION},
};
use vibemux_workflow::{Sha256Digest, SpecIdentifier, gates::WorkflowPhase};
use vibemuxd::{
    WriterWorker,
    control::{ControlClient, ControlError, DaemonControlServer, ExecutionSettings},
    harness_dispatch::{DISPATCH_CONFIG_FILE_NAME, DispatchServiceSettings},
    workflow::{WorkflowServiceSettings, settings::WORKFLOW_CONFIG_FILE_NAME, turns},
};

pub const TRAMPOLINE: &str = env!("CARGO_BIN_EXE_vibemux_launch_trampoline");
pub const WORKER_FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_worker_fixture");
pub const TASKBOARD_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/taskboard_lite"
);
const SYNTAX_SUITE_SOURCE: &str = include_str!("syntax_suite.mjs");
const SYNTAX_SUITE_FILE_NAME: &str = "syntax_suite.mjs";
const SCRIPT_FILE_NAME: &str = "worker_fixture_script.json";
const LOG_FILE_NAME: &str = "worker_fixture_log.jsonl";
const DATABASE_FILE_NAME: &str = "vibemux_rust.sqlite3";
const SNAPSHOT_TIME: &str = "2026-10-01T00:00:00Z";
const MIN_NODE_MAJOR: u32 = 22;
const TURN_DEADLINE_MS: u64 = 120_000;
const SHUTDOWN_GRACE_MS: u64 = 2_000;
const VERIFIER_TIMEOUT_MS: u64 = 300_000;
const HEARTBEAT_MS: u64 = 30_000;
/// Long enough for a run with every TaskBoard suite on a slow runner.
pub const SETTLE_TIMEOUT: Duration = Duration::from_secs(420);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_EXPORT_PAGES: usize = 256;
/// Names the verifier may need beyond the system names; only names the
/// test environment defines are forwarded, because a missing allowlisted
/// name blocks the suite.
const VERIFIER_ENVIRONMENT_CANDIDATES: [&str; 8] = [
    "PATH",
    "LOCALAPPDATA",
    "APPDATA",
    "USERPROFILE",
    "PROGRAMDATA",
    "SYSTEMDRIVE",
    "PROGRAMFILES",
    "HOME",
];
const TASKBOARD_SUITES: [&str; 4] = ["api", "store", "browser", "browser_frontend_only"];

/// Set by the acceptance runner (`scripts/verify_dual_track.py`) to collect
/// the daemon's exported evidence of selected tests.
pub const EVIDENCE_DIR_ENV: &str = "VIBEMUX_ACCEPTANCE_EVIDENCE_DIR";
const EVIDENCE_SCHEMA_VERSION: u32 = 1;

pub const WORKFLOW_KEY: &str = "taskboard_lite";
pub const POLICY_VERSION: &str = "policy_v1";

/// The two-track request every cooperative test compiles.
pub const COOPERATIVE_REQUEST: &str = "Build TaskBoard Lite in two tracks. Track A must implement the API server and task store in `src/server.mjs` and `src/store.mjs`. Track B must implement the browser page in `public/app.mjs`. Do not modify `package.json`.";
const COOPERATIVE_OPENING: &str = "Build TaskBoard Lite in two tracks.";
const TRACK_A_CLAUSE: &str =
    "Track A must implement the API server and task store in `src/server.mjs` and `src/store.mjs`.";
const TRACK_B_CLAUSE: &str = "Track B must implement the browser page in `public/app.mjs`.";
const PROHIBITION_CLAUSE: &str = "Do not modify `package.json`.";

/// The single-task request of the store tests (compare, repair, scope).
pub const STORE_REQUEST: &str =
    "Implement the TaskBoard Lite task store in `src/store.mjs`. Do not modify `package.json`.";
const STORE_CLAUSE: &str = "Implement the TaskBoard Lite task store in `src/store.mjs`.";

/// One configured workflow slot.
#[derive(Clone, Copy, Debug)]
pub struct SlotSetup {
    pub slot_id: &'static str,
    pub harness: AgentKind,
    pub route_role: &'static str,
}

#[derive(Clone, Debug)]
pub struct FixtureOptions {
    pub slots: Vec<SlotSetup>,
    /// `None` keeps the opt-in content store off whatever the policy says.
    pub content_retention_days: Option<u32>,
    /// Installs `opencode` as an ACP route, which never executes turns.
    pub acp_route: bool,
}

impl Default for FixtureOptions {
    fn default() -> Self {
        Self {
            slots: vec![
                SlotSetup {
                    slot_id: "slot_a",
                    harness: AgentKind::Codex,
                    route_role: "worker_a",
                },
                SlotSetup {
                    slot_id: "slot_b",
                    harness: AgentKind::Codex,
                    route_role: "worker_b",
                },
                SlotSetup {
                    slot_id: "review",
                    harness: AgentKind::Claude,
                    route_role: "reviewer",
                },
            ],
            content_retention_days: Some(7),
            acp_route: false,
        }
    }
}

impl FixtureOptions {
    /// The default slots plus `slot_acp`, an OpenCode slot whose ACP route
    /// can only probe, never execute a writable turn.
    pub fn with_acp_slot() -> Self {
        let mut options = Self::default();
        options.slots.push(SlotSetup {
            slot_id: "slot_acp",
            harness: AgentKind::OpenCode,
            route_role: "worker_b",
        });
        options.acp_route = true;
        options
    }
}

pub fn remote(code: &str) -> ControlError {
    ControlError::Remote {
        code: code.to_string(),
    }
}

fn write_json(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).expect("encode")).expect("write json");
}

/// The first `name` executable on `PATH`.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .find(|candidate| candidate.is_file())
}

/// Node 22 or newer: the TaskBoard browser suite needs the global
/// `WebSocket`. A missing or older Node fails the test loudly instead of
/// skipping it.
fn require_node() -> PathBuf {
    let node = find_on_path("node").expect("node 22+ must be on PATH for the workflow tests");
    let output = Command::new(&node)
        .arg("--version")
        .output()
        .expect("node --version");
    let version = String::from_utf8_lossy(&output.stdout);
    let major: u32 = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .expect("node version");
    assert!(
        major >= MIN_NODE_MAJOR,
        "the workflow tests need node {MIN_NODE_MAJOR}+, found {}",
        version.trim()
    );
    node
}

fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).expect("create directory");
    for entry in std::fs::read_dir(source).expect("read directory") {
        let entry = entry.expect("entry");
        let destination = target.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            std::fs::copy(entry.path(), &destination).expect("copy file");
        }
    }
}

/// A TaskBoard Lite file, relative to the fixture root (for example
/// `calibration/good/src/store.mjs`).
pub fn taskboard_file(relative: &str) -> String {
    std::fs::read_to_string(Path::new(TASKBOARD_DIR).join(relative)).expect("taskboard file")
}

/// The calibrated implementation of `paths`, keyed by project path.
pub fn good_files(paths: &[&str]) -> Value {
    let files: serde_json::Map<String, Value> = paths
        .iter()
        .map(|path| {
            (
                (*path).to_string(),
                Value::String(taskboard_file(&format!("calibration/good/{path}"))),
            )
        })
        .collect();
    Value::Object(files)
}

pub fn session_id(workflow_id: Uuid, task_key: &str, slot_id: &str) -> Uuid {
    turns::session_id(
        workflow_id,
        &SpecIdentifier::new(task_key).expect("task key"),
        &SpecIdentifier::new(slot_id).expect("slot id"),
    )
}

pub fn workflow_id(prepared: &Value) -> Uuid {
    prepared["workflow_id"]
        .as_str()
        .and_then(|text| Uuid::parse_str(text).ok())
        .expect("workflow id")
}

/// The paths of one fixture project.
#[derive(Clone, Debug)]
struct ProjectPaths {
    project_root: PathBuf,
    state_dir: PathBuf,
    vendor_dir: PathBuf,
    runtime_dir: PathBuf,
    git: PathBuf,
    git_home: PathBuf,
}

impl ProjectPaths {
    fn database(&self) -> PathBuf {
        self.state_dir.join(DATABASE_FILE_NAME)
    }

    /// Runs git on the original checkout with an isolated configuration.
    fn git(&self, arguments: &[&str]) -> String {
        let output = Command::new(&self.git)
            .args([
                "-c",
                "core.autocrlf=false",
                "-c",
                "user.name=VibeMux Test",
                "-c",
                "user.email=vibemux-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(arguments)
            .current_dir(&self.project_root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.git_home.join("gitconfig"))
            .env("HOME", &self.git_home)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("git output")
    }

    fn install_harnesses(&self, options: &FixtureOptions) {
        let mut routes = Vec::new();
        let mut install = |harness: AgentKind, protocol: NativeProtocol, allow_execution: bool| {
            let executable = self.vendor_dir.join(format!(
                "{}{}",
                harness.command_name(),
                std::env::consts::EXE_SUFFIX
            ));
            std::fs::copy(WORKER_FIXTURE, &executable).expect("install worker fixture");
            routes.push(json!({"harness": harness, "protocol": protocol,
                "executable": executable, "enabled": true, "allow_execution": allow_execution}));
        };
        install(AgentKind::Codex, NativeProtocol::CodexExec, true);
        install(AgentKind::Claude, NativeProtocol::ClaudeStreamJson, true);
        if options.acp_route {
            install(AgentKind::OpenCode, NativeProtocol::Acp, false);
        }
        write_json(
            &self.state_dir.join(DISPATCH_CONFIG_FILE_NAME),
            &json!({"schema_version": ROUTE_CONFIG_SCHEMA_VERSION, "routes": routes,
                "limits": {"deadline_ms": TURN_DEADLINE_MS, "shutdown_grace_ms": SHUTDOWN_GRACE_MS}}),
        );
    }

    fn write_workflow_config(&self, options: &FixtureOptions, node: &Path, verifier_dir: &Path) {
        let mut suites = serde_json::Map::new();
        for suite in TASKBOARD_SUITES {
            suites.insert(
                suite.to_string(),
                json!([
                    "{verifier_dir}/run_verifier.mjs",
                    "--candidate",
                    "{candidate}",
                    "--suite",
                    suite,
                    "--out",
                    "{out}"
                ]),
            );
        }
        suites.insert(
            "syntax".to_string(),
            json!([
                format!("{{verifier_dir}}/{SYNTAX_SUITE_FILE_NAME}"),
                "--candidate",
                "{candidate}",
                "--suite",
                "syntax",
                "--out",
                "{out}"
            ]),
        );
        let environment_names: Vec<&str> = VERIFIER_ENVIRONMENT_CANDIDATES
            .into_iter()
            .filter(|name| std::env::var_os(name).is_some())
            .collect();
        let slots: Vec<Value> = options
            .slots
            .iter()
            .map(|slot| {
                json!({"slot_id": slot.slot_id, "harness": slot.harness,
                    "route_role": slot.route_role})
            })
            .collect();
        write_json(
            &self.state_dir.join(WORKFLOW_CONFIG_FILE_NAME),
            &json!({
                "schema_version": 1,
                "evidence_class": "fixture",
                "git_executable": self.git,
                "slots": slots,
                "verifier": {
                    "directory": verifier_dir,
                    "executable": node,
                    "suites": suites,
                    "timeout_ms": VERIFIER_TIMEOUT_MS,
                    "environment_names": environment_names,
                },
                "heartbeat_ms": HEARTBEAT_MS,
                "candidate_limits": {"max_files": 256, "max_file_bytes": 1_048_576,
                    "max_total_bytes": 8_388_608},
                "content_retention_days": options.content_retention_days,
            }),
        );
    }

    async fn start_server(&self) -> (DaemonControlServer, ControlClient) {
        let settings = ExecutionSettings {
            dispatch: DispatchServiceSettings {
                project_root: self.project_root.clone(),
                config_path: self.state_dir.join(DISPATCH_CONFIG_FILE_NAME),
                trampoline: Some(PathBuf::from(TRAMPOLINE)),
            },
            workflow: Some(WorkflowServiceSettings::for_project(
                &self.project_root,
                &self.state_dir,
            )),
        };
        let server = DaemonControlServer::start_with_execution(
            &self.database(),
            &self.runtime_dir,
            settings,
        )
        .await
        .expect("control server");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        (server, client)
    }
}

/// A running control server over a committed TaskBoard Lite project.
pub struct WorkflowFixture {
    server: Option<DaemonControlServer>,
    pub client: ControlClient,
    pub project_root: PathBuf,
    pub state_dir: PathBuf,
    pub vendor_dir: PathBuf,
    pub head: String,
    paths: ProjectPaths,
    /// Dropped last: the server and every fixture process must be gone.
    temp: tempfile::TempDir,
}

impl WorkflowFixture {
    pub async fn start(options: FixtureOptions) -> Self {
        let node = require_node();
        let git = find_on_path("git").expect("git must be on PATH for the workflow tests");
        let temp = tempfile::tempdir().expect("temp");
        let project_root = temp.path().join("project");
        let paths = ProjectPaths {
            state_dir: project_root.join(".vibemux"),
            project_root,
            vendor_dir: temp.path().join("vendor"),
            runtime_dir: temp.path().join("runtime"),
            git,
            git_home: temp.path().join("git_home"),
        };
        let verifier_dir = temp.path().join("verifier");
        copy_tree(&Path::new(TASKBOARD_DIR).join("base"), &paths.project_root);
        copy_tree(&Path::new(TASKBOARD_DIR).join("verifier"), &verifier_dir);
        std::fs::write(
            verifier_dir.join(SYNTAX_SUITE_FILE_NAME),
            SYNTAX_SUITE_SOURCE,
        )
        .expect("syntax suite");
        for directory in [
            &paths.state_dir,
            &paths.vendor_dir,
            &paths.runtime_dir,
            &paths.git_home,
        ] {
            std::fs::create_dir_all(directory).expect("directory");
        }
        paths.git(&["init", "-q"]);
        paths.git(&["add", "-A"]);
        paths.git(&["commit", "-q", "-m", "TaskBoard Lite base"]);
        let head = paths.git(&["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(
            paths.project_root.join(".git").join("info").join("exclude"),
            ".vibemux/\n",
        )
        .expect("exclude state directory");
        paths.install_harnesses(&options);
        paths.write_workflow_config(&options, &node, &verifier_dir);
        let mut detected = vec![AgentKind::Codex, AgentKind::Claude];
        if options.acp_route {
            detected.push(AgentKind::OpenCode);
        }
        record_detection(&paths.database(), &detected).await;
        write_json(
            &paths.vendor_dir.join(SCRIPT_FILE_NAME),
            &json!({"steps": {}}),
        );
        let (server, client) = paths.start_server().await;
        Self {
            server: Some(server),
            client,
            project_root: paths.project_root.clone(),
            state_dir: paths.state_dir.clone(),
            vendor_dir: paths.vendor_dir.clone(),
            head,
            paths,
            temp,
        }
    }

    /// Stops the server (pausing any running coordinator) and starts a new
    /// one over the same database, as a daemon restart would.
    pub async fn restart(&mut self) {
        if let Some(server) = self.server.take() {
            server.shutdown().await.expect("server shutdown");
        }
        let (server, client) = self.paths.start_server().await;
        self.server = Some(server);
        self.client = client;
    }

    /// Restarts over the phase a crashed daemon leaves behind. A graceful
    /// shutdown pauses a running workflow, so with the server stopped a
    /// writer of its own moves the workflow back to `running`, exactly as
    /// if its coordinator had never stopped; then a new server starts.
    pub async fn restart_after_crash(&mut self, workflow_id: Uuid) {
        if let Some(server) = self.server.take() {
            server.shutdown().await.expect("server shutdown");
        }
        let database = self.paths.database();
        tokio::task::spawn_blocking(move || {
            let writer = WriterWorker::start(&database).expect("writer");
            let handle = writer.handle().expect("handle");
            let record = handle
                .workflow_record(workflow_id)
                .expect("record")
                .expect("workflow");
            if record.phase != WorkflowPhase::Running {
                handle
                    .transition_workflow(
                        workflow_id,
                        record.version,
                        WorkflowPhase::Running,
                        None,
                        OffsetDateTime::now_utc(),
                    )
                    .expect("running");
            }
            writer.shutdown().expect("writer shutdown");
        })
        .await
        .expect("join");
        let (server, client) = self.paths.start_server().await;
        self.server = Some(server);
        self.client = client;
    }

    pub async fn shutdown(mut self) {
        if let Some(server) = self.server.take() {
            server.shutdown().await.expect("server shutdown");
        }
    }

    pub fn git(&self, arguments: &[&str]) -> String {
        self.paths.git(arguments)
    }

    /// A scratch directory outside the project.
    pub fn scratch(&self, name: &str) -> PathBuf {
        let directory = self.temp.path().join(name);
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    /// The original checkout's porcelain status: empty while no workflow
    /// touched it.
    pub fn checkout_status(&self) -> String {
        self.git(&["status", "--porcelain", "--untracked-files=all"])
    }

    pub fn write_script(&self, script: &Value) {
        write_json(&self.vendor_dir.join(SCRIPT_FILE_NAME), script);
    }

    /// Every record the worker fixtures logged, in order.
    pub fn log(&self) -> Vec<Value> {
        let Ok(text) = std::fs::read_to_string(self.vendor_dir.join(LOG_FILE_NAME)) else {
            return Vec::new();
        };
        text.lines()
            .map(|line| serde_json::from_str(line).expect("log record"))
            .collect()
    }

    /// The `started` records of one fixture key (`<task>/<purpose>_<n>`).
    pub fn started(&self, key: &str) -> Vec<Value> {
        self.log()
            .into_iter()
            .filter(|record| record["event"] == "started" && record["key"] == key)
            .collect()
    }

    pub fn started_keys(&self) -> Vec<String> {
        self.log()
            .into_iter()
            .filter(|record| record["event"] == "started")
            .map(|record| record["key"].as_str().expect("key").to_string())
            .collect()
    }

    /// Prepares a workflow and asserts it was admitted.
    pub async fn prepare(&self, request: Value, policy: Value) -> Value {
        let prepared = self
            .client
            .workflow_prepare(request, policy)
            .await
            .expect("prepare");
        assert!(
            prepared.get("start_contract").is_some(),
            "the workflow was not admitted: {prepared}"
        );
        prepared
    }

    pub async fn start_workflow(&self, prepared: &Value) -> Value {
        let contract = prepared["start_contract"].as_str().expect("start contract");
        self.client
            .workflow_start(contract, Uuid::new_v4())
            .await
            .expect("start")
    }

    pub async fn status(&self, workflow_id: Uuid) -> Value {
        self.client
            .workflow_status(workflow_id)
            .await
            .expect("status")
    }

    /// Polls status until `done` holds.
    pub async fn wait_until(&self, workflow_id: Uuid, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            let status = self.status(workflow_id).await;
            if done(&status) {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the workflow never settled: phase {} running_here {}",
                status["phase"],
                status["running_here"]
            );
            sleep(POLL_INTERVAL).await;
        }
    }

    /// Waits until no coordinator of this daemon drives the workflow.
    pub async fn wait_idle(&self, workflow_id: Uuid) -> Value {
        self.wait_until(workflow_id, |status| {
            status["running_here"] == false && !status["last_run"].is_null()
        })
        .await
    }

    /// Waits until no coordinator drives the workflow and its phase is one
    /// of `phases`; unlike [`Self::wait_idle`], a previous run's report
    /// cannot satisfy it while the workflow is still running.
    pub async fn wait_phase(&self, workflow_id: Uuid, phases: &[&str]) -> Value {
        self.wait_until(workflow_id, |status| {
            status["running_here"] == false
                && phases.iter().any(|expected| status["phase"] == *expected)
        })
        .await
    }

    /// Waits until a fixture turn of `key` has started.
    pub async fn wait_for_turn(&self, key: &str) {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        while self.started(key).is_empty() {
            assert!(Instant::now() < deadline, "turn {key} never started");
            sleep(POLL_INTERVAL).await;
        }
    }

    /// Every export item, following the page cursor to the end.
    pub async fn export_all(&self, workflow_id: Uuid) -> Vec<Value> {
        let mut items = Vec::new();
        let mut cursor = 0;
        for _ in 0..MAX_EXPORT_PAGES {
            let page = self
                .client
                .workflow_export(workflow_id, cursor)
                .await
                .expect("export page");
            assert_eq!(page["schema_version"], 1);
            items.extend(page["items"].as_array().expect("items").iter().cloned());
            match page["next"].as_u64() {
                Some(next) => cursor = usize::try_from(next).expect("cursor"),
                None => {
                    assert_eq!(
                        page["total_items"].as_u64(),
                        Some(items.len() as u64),
                        "the export lost items"
                    );
                    return items;
                }
            }
        }
        panic!("the export never ended");
    }

    /// Writes the daemon's own view of a settled workflow (status, every
    /// export item, every session inspection) to the acceptance evidence
    /// directory, when the runner set one. Everything comes back through
    /// Control, so the acceptance report reconciles exported receipts, not
    /// claims a test makes about itself.
    pub async fn record_evidence(&self, name: &str, workflow_id: Uuid) {
        let Some(directory) = evidence_dir() else {
            return;
        };
        let status = self.status(workflow_id).await;
        let export = self.export_all(workflow_id).await;
        let mut sessions = Vec::new();
        for session in status["sessions"].as_array().into_iter().flatten() {
            let session_id = session["session_id"]
                .as_str()
                .and_then(|id| Uuid::parse_str(id).ok())
                .expect("session id");
            let inspected = self
                .client
                .workflow_session_inspect(session_id)
                .await
                .expect("session inspect");
            sessions.push(inspected);
        }
        let evidence = json!({
            "schema_version": EVIDENCE_SCHEMA_VERSION,
            "name": name,
            "workflow_id": workflow_id,
            "status": status,
            "export": export,
            "sessions": sessions,
        });
        write_evidence(&directory, name, &evidence);
    }

    /// Writes one value the daemon returned (such as an optimizer report)
    /// to the acceptance evidence directory, when the runner set one.
    pub fn record_value(&self, name: &str, value: &Value) {
        let Some(directory) = evidence_dir() else {
            return;
        };
        let evidence = json!({
            "schema_version": EVIDENCE_SCHEMA_VERSION,
            "name": name,
            "value": value,
        });
        write_evidence(&directory, name, &evidence);
    }
}

/// The acceptance runner's evidence directory; unset outside a runner.
fn evidence_dir() -> Option<PathBuf> {
    std::env::var_os(EVIDENCE_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn write_evidence(directory: &Path, name: &str, evidence: &Value) {
    assert!(
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
        "evidence names are snake_case: {name}"
    );
    std::fs::create_dir_all(directory).expect("evidence directory");
    write_json(&directory.join(format!("{name}.json")), evidence);
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
                "workflow_fixture_snapshot".to_string(),
            )
            .expect("commit detection snapshot");
        writer.shutdown().expect("writer shutdown");
    })
    .await
    .expect("join");
}

// ---------------------------------------------------------------- specs --

/// One requirement of a drafted TaskSpec; `quoted` must occur verbatim in
/// the request.
#[derive(Clone, Debug)]
pub struct RequirementDraft {
    pub requirement_id: &'static str,
    pub statement: &'static str,
    pub force: &'static str,
    pub quoted: &'static str,
    pub literals: &'static [&'static str],
}

/// What a test varies between TaskSpecs; everything else is fixed.
#[derive(Clone, Debug)]
pub struct TaskDraft {
    pub task_key: &'static str,
    pub role: &'static str,
    pub owned: &'static [&'static str],
    pub may_message: &'static [&'static str],
    pub suites: Vec<&'static str>,
    pub requirements: Vec<RequirementDraft>,
    /// Clauses addressed to another task of the workflow.
    pub excluded: &'static [&'static str],
    pub max_turns: u32,
    pub max_repairs: u32,
}

pub fn span(request_text: &str, quoted: &str) -> Value {
    let start = request_text
        .find(quoted)
        .expect("quoted text is in the request");
    json!({"start": start, "end": start + quoted.len(), "quoted": quoted})
}

fn prohibition() -> RequirementDraft {
    RequirementDraft {
        requirement_id: "r_manifest",
        statement: "Do not modify the package manifest.",
        force: "must_not",
        quoted: PROHIBITION_CLAUSE,
        literals: &["package.json"],
    }
}

/// Track A of [`COOPERATIVE_REQUEST`]: the server and the store.
pub fn track_a(suites: &[&'static str]) -> TaskDraft {
    TaskDraft {
        task_key: "track_a",
        role: "backend_domain",
        owned: &["src/server.mjs", "src/store.mjs", "tests/worker_a/**"],
        may_message: &["track_b"],
        suites: suites.to_vec(),
        requirements: vec![
            RequirementDraft {
                requirement_id: "r_tracks",
                statement: "Build TaskBoard Lite as one of two tracks.",
                force: "must",
                quoted: COOPERATIVE_OPENING,
                literals: &[],
            },
            RequirementDraft {
                requirement_id: "r_backend",
                statement: "Implement the API server and the task store.",
                force: "must",
                quoted: TRACK_A_CLAUSE,
                literals: &["src/server.mjs", "src/store.mjs"],
            },
            prohibition(),
        ],
        excluded: &[TRACK_B_CLAUSE],
        max_turns: 4,
        max_repairs: 1,
    }
}

/// Track B of [`COOPERATIVE_REQUEST`]: the browser page.
pub fn track_b(suites: &[&'static str]) -> TaskDraft {
    TaskDraft {
        task_key: "track_b",
        role: "frontend",
        owned: &["public/**", "tests/worker_b/**"],
        may_message: &["track_a"],
        suites: suites.to_vec(),
        requirements: vec![
            RequirementDraft {
                requirement_id: "r_tracks",
                statement: "Build TaskBoard Lite as one of two tracks.",
                force: "must",
                quoted: COOPERATIVE_OPENING,
                literals: &[],
            },
            RequirementDraft {
                requirement_id: "r_frontend",
                statement: "Implement the browser page.",
                force: "must",
                quoted: TRACK_B_CLAUSE,
                literals: &["public/app.mjs"],
            },
            prohibition(),
        ],
        excluded: &[TRACK_A_CLAUSE],
        max_turns: 4,
        max_repairs: 1,
    }
}

/// The single task of [`STORE_REQUEST`].
pub fn store_task(suites: &[&'static str]) -> TaskDraft {
    TaskDraft {
        task_key: "store",
        role: "store_component",
        owned: &["src/store.mjs", "tests/worker_store/**"],
        may_message: &[],
        suites: suites.to_vec(),
        requirements: vec![
            RequirementDraft {
                requirement_id: "r_store",
                statement: "Implement the TaskBoard Lite task store.",
                force: "must",
                quoted: STORE_CLAUSE,
                literals: &["src/store.mjs"],
            },
            prohibition(),
        ],
        excluded: &[],
        max_turns: 4,
        max_repairs: 1,
    }
}

pub fn task_spec(draft: &TaskDraft, request_text: &str, mode: &str, base_commit: &str) -> Value {
    let suite_checks: Vec<String> = draft
        .suites
        .iter()
        .map(|suite| format!("c_{suite}"))
        .collect();
    let mut checks: Vec<Value> = draft
        .suites
        .iter()
        .zip(&suite_checks)
        .map(|(suite, check_id)| {
            json!({"check_id": check_id, "kind": "existing_verifier_suite",
                "description": format!("The trusted {suite} suite passes."),
                "verifier_suite": suite})
        })
        .collect();
    checks.push(json!({"check_id": "c_review", "kind": "review_criterion",
        "description": "No change outside the owned paths.", "verifier_suite": null}));
    let requirements: Vec<Value> = draft
        .requirements
        .iter()
        .map(|requirement| {
            let check = if requirement.force == "must_not" {
                "c_review".to_string()
            } else {
                suite_checks
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "c_review".into())
            };
            json!({
                "requirement_id": requirement.requirement_id,
                "statement": requirement.statement,
                "force": requirement.force,
                "source_refs": [span(request_text, requirement.quoted)],
                "contract_excerpts": [],
                "literals": requirement.literals,
                "acceptance_check_ids": [check],
            })
        })
        .collect();
    let exclusions: Vec<Value> = draft
        .excluded
        .iter()
        .map(|quoted| json!({"span": span(request_text, quoted), "reason": "addressed_to_other_task"}))
        .collect();
    json!({
        "schema_version": 1,
        "workflow_key": WORKFLOW_KEY,
        "task_key": draft.task_key,
        "contract_version": 1,
        "original_request_ref": Sha256Digest::of(request_text.as_bytes()).to_hex(),
        "source_language": "en",
        "requirements": requirements,
        "objective": format!("Deliver the {} part of TaskBoard Lite.", draft.task_key),
        "non_goals": ["Authentication."],
        "assumptions": [],
        "unresolved_questions": [],
        "source_exclusions": exclusions,
        "base_commit": base_commit,
        "input_artifacts": [],
        "shared_contract_refs": [],
        "owned_paths": draft.owned,
        "read_scopes": ["CONTRACT.md"],
        "forbidden_paths": ["package.json"],
        "dependencies": [],
        "required_capabilities": ["structured_turn", "writable_worktree"],
        "permitted_tools": ["read_files", "search_files", "edit_files", "write_files",
            "run_dev_tests"],
        "acceptance_checks": checks,
        "output_contract": {"checkpoint_report_required": true,
            "changes_within_owned_paths": true},
        "completion_definition": {"required_levels": ["candidate_artifact_collected",
            "independent_review_passed", "local_verifier_passed"]},
        "communication_policy": {"may_message": draft.may_message, "max_messages_per_turn": 2,
            "auto_authorized_kinds": ["answer", "progress"]},
        "context_policy": {"max_bundle_bytes": 8192,
            "retrieval_order": ["verifier_receipts", "candidate_snapshot"],
            "allow_public_excerpts": true},
        "resource_budget": {"max_turns": draft.max_turns, "max_repairs": draft.max_repairs,
            "max_elapsed_seconds": 1800, "max_model_requests": 40},
        "role": draft.role,
        "workflow_mode": mode,
        "template_version": "worker_v1",
        "policy_version": POLICY_VERSION,
    })
}

/// A cooperative request over [`COOPERATIVE_REQUEST`]: track A on
/// `slot_a`, track B on `slot_b`, reviewed by `review`.
pub fn cooperative_request(
    request_key: &str,
    base_commit: &str,
    track_a: &TaskDraft,
    track_b: &TaskDraft,
    integration_suites: &[&str],
) -> Value {
    json!({
        "schema_version": 1,
        "request_key": request_key,
        "workflow_key": WORKFLOW_KEY,
        "mode": "cooperate",
        "request_text": COOPERATIVE_REQUEST,
        "task_specs": [
            task_spec(track_a, COOPERATIVE_REQUEST, "cooperate", base_commit),
            task_spec(track_b, COOPERATIVE_REQUEST, "cooperate", base_commit),
        ],
        "integration_suites": integration_suites,
        "assignments": {"track_a": ["slot_a"], "track_b": ["slot_b"]},
        "reviewer_slot": "review",
    })
}

/// A single store task: cooperative on `slot_a`, or a comparison of
/// `slot_a` and `slot_b`.
pub fn store_request(
    request_key: &str,
    base_commit: &str,
    mode: &str,
    draft: &TaskDraft,
    negative_control: bool,
) -> Value {
    let slots = if mode == "compare" {
        json!(["slot_a", "slot_b"])
    } else {
        json!(["slot_a"])
    };
    json!({
        "schema_version": 1,
        "request_key": request_key,
        "workflow_key": WORKFLOW_KEY,
        "mode": mode,
        "request_text": STORE_REQUEST,
        "task_specs": [task_spec(draft, STORE_REQUEST, mode, base_commit)],
        "integration_suites": draft.suites,
        "assignments": {"store": slots},
        "reviewer_slot": "review",
        "negative_control": negative_control,
    })
}

/// The operator policy every test uses; `suites` are the declared trusted
/// suites.
pub fn policy(suites: &[&str], max_parallel_slots: u32, content_store_opt_in: bool) -> Value {
    let declared: Vec<Value> = suites
        .iter()
        .map(|suite| json!({"suite_id": suite, "description": format!("Trusted {suite} suite.")}))
        .collect();
    json!({
        "schema_version": 1,
        "policy_version": POLICY_VERSION,
        "writable_roots": ["src/**", "public/**", "tests/worker_a/**", "tests/worker_b/**",
            "tests/worker_store/**"],
        "protected_paths": ["package.json", "CONTRACT.md"],
        "allowed_tools": ["read_files", "search_files", "edit_files", "write_files",
            "run_dev_tests"],
        "verifier_suites": declared,
        "budget_caps": {"max_turns": 6, "max_repairs": 2, "max_elapsed_seconds": 1800,
            "max_model_requests": 40},
        "max_parallel_slots": max_parallel_slots,
        "max_supervisor_decisions": 16,
        "max_optimizer_candidates": 4,
        "content_store_opt_in": content_store_opt_in,
        "aag_only": true,
    })
}

/// A passing review for every task.
pub fn review_pass() -> Value {
    json!({"review": {"verdict": "pass", "findings": [],
        "checks_executed": ["read the candidate diff"]}})
}

pub fn phase(status: &Value) -> &str {
    status["phase"].as_str().expect("phase")
}

pub fn task_view<'a>(status: &'a Value, task_key: &str) -> &'a Value {
    status["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .find(|task| task["task_key"] == task_key)
        .expect("task view")
}
