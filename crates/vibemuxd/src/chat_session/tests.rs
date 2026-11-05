//! Actor seam fixtures use bounded owned pipes; no vendor inference.
use super::{actor::run_transport, contracts::*, input_writer::Stdin, transport::RuntimeTransport};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{
        AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf,
    },
    sync::{Notify, mpsc, watch},
    time::{Instant, timeout},
};
use uuid::Uuid;
use vibemux_harness::{
    HarnessDetection,
    chat_session::{
        SessionConfig, SessionDecoder, SessionLimits, SessionMachine, SessionPhase, SessionTarget,
        TurnOutcome, TurnRequest,
    },
};
use vibemux_store::{ChatOpen, SqliteStore};
use vibemux_types::chat::ChatHarness;

const WAIT: Duration = Duration::from_secs(5);
const IDLE_CLOSE_OUTPUT_DELAY: Duration = Duration::from_millis(20);
const IDLE_CLOSE_OBSERVER_DELAY: Duration = Duration::from_millis(5);

pub(super) fn progress_batch(request: &TurnRequest, revision: u64) -> RuntimeCommandProgress {
    RuntimeCommandProgress {
        request_id: request.request_id,
        turn_id: request.turn_id,
        limited: false,
        updates: vec![vibemux_harness::chat_session::CommandProgress {
            request_id: request.request_id,
            turn_id: request.turn_id,
            call_id: "live_command".into(),
            revision,
            command: "PRIVATE_PROGRESS".into(),
            command_original_bytes: 16,
            command_truncated: false,
            working_directory: "C:\\private".into(),
            directory_original_bytes: 10,
            directory_truncated: false,
            output_tail: Some(format!("output {revision}")),
            output_original_bytes: Some(format!("output {revision}").len() as u64),
            output_omitted_bytes: 0,
        }],
    }
}

#[derive(Default)]
pub(super) struct Capture {
    claimed: Mutex<BTreeSet<Uuid>>,
    events: Mutex<Vec<RuntimeEvent>>,
    changed: Notify,
    completed: AtomicUsize,
    refuse_claim: AtomicBool,
    fail_text: AtomicBool,
    hang_tool: AtomicBool,
    hang_claim: AtomicBool,
    progress: Mutex<Vec<RuntimeCommandProgress>>,
    progress_calls: AtomicUsize,
}

#[async_trait]
impl ChatRuntimeSink for Capture {
    fn try_command_progress(
        &self,
        _: &RuntimeBinding,
        progress: RuntimeCommandProgress,
    ) -> Result<bool, RuntimeError> {
        self.progress_calls.fetch_add(1, Ordering::AcqRel);
        let Ok(mut captured) = self.progress.try_lock() else {
            return Ok(false);
        };
        captured.push(progress);
        Ok(true)
    }
    async fn claim_turn(&self, _: &RuntimeBinding, turn: &TurnRequest) -> Result<(), RuntimeError> {
        if self.hang_claim.load(Ordering::Acquire) {
            std::future::pending::<()>().await;
        }
        if self.refuse_claim.load(Ordering::Acquire)
            || !self.claimed.lock().expect("claims").insert(turn.request_id)
        {
            return Err(RuntimeError::ClaimRefused);
        }
        Ok(())
    }
    async fn event(&self, _: &RuntimeBinding, event: RuntimeEvent) -> Result<(), RuntimeError> {
        let hang_tool = matches!(&event, RuntimeEvent::ToolCompleted(_))
            && self.hang_tool.load(Ordering::Acquire);
        let failed = matches!(&event, RuntimeEvent::AssistantSnapshot(_))
            && self.fail_text.load(Ordering::Acquire);
        if matches!(&event, RuntimeEvent::TurnFinished { .. }) {
            self.completed.fetch_add(1, Ordering::AcqRel);
        }
        self.events.lock().expect("events").push(event);
        self.changed.notify_waiters();
        if hang_tool {
            std::future::pending::<()>().await;
        }
        if failed {
            Err(RuntimeError::ContentUnavailable)
        } else {
            Ok(())
        }
    }
}

impl Capture {
    pub(super) async fn wait_completed(&self, count: usize) {
        timeout(WAIT, async {
            loop {
                let changed = self.changed.notified();
                if self.completed.load(Ordering::Acquire) >= count {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("bounded completion");
    }
}

struct Pipes {
    stdout: ReadHalf<DuplexStream>,
    stdin: Option<WriteHalf<DuplexStream>>,
    finishes: Arc<AtomicUsize>,
}
#[async_trait]
impl RuntimeTransport for Pipes {
    fn take_stdin(&mut self) -> Option<Stdin> {
        self.stdin.take().map(|stdin| Box::new(stdin) as Stdin)
    }
    async fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.stdout.read(buffer).await
    }
    async fn finish(&mut self, force: bool, _: Instant) -> RuntimeCleanup {
        self.finishes.fetch_add(1, Ordering::AcqRel);
        RuntimeCleanup {
            exit: vibemux_harness::dispatch::ProcessExit {
                exit_code: Some(0),
                forced_termination: force,
            },
            root_reaped: true,
            tree_empty: true,
            cleanup_verified: true,
            stderr_bytes: 0,
        }
    }
}

pub(super) fn binding() -> RuntimeBinding {
    let temp = tempfile::tempdir().expect("temporary store");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    store
        .commit_harness_snapshot(
            &std::collections::BTreeMap::from([(
                "codex".to_owned(),
                HarnessDetection {
                    detected: true,
                    path: None,
                    launcher: None,
                    version: Some("synthetic".into()),
                },
            )]),
            "2026-10-31T00:00:00Z",
            "runtime_fixture",
        )
        .expect("detection");
    let now = time::OffsetDateTime::now_utc();
    let session = store
        .open_chat(&ChatOpen {
            operation_id: Uuid::new_v4(),
            harness: ChatHarness::Codex,
            profile_sha256: "b".repeat(64),
            resource_key: "c".repeat(64),
            base_commit: "d".repeat(40),
            title_ref: None,
            timestamp: now,
        })
        .expect("open")
        .session;
    let claim = store
        .claim_chat_owner(session.chat_id, 1, None, now)
        .expect("fenced owner");
    RuntimeBinding {
        chat_id: session.chat_id,
        generation: 1,
        owner_fence: claim.fence,
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        target: SessionTarget::Codex {
            resume_thread_id: None,
        },
        generation_id: Uuid::new_v4(),
        working_directory: if cfg!(windows) {
            "C:\\synthetic_project".into()
        } else {
            "/synthetic_project".into()
        },
        model: None,
    }
}

#[cfg(feature = "test_helpers")]
#[test]
fn a_fixture_protocol_machine_cannot_use_the_unguarded_native_launch_seam() {
    let mut configuration = config();
    configuration.target = SessionTarget::Claude {
        session_id: Uuid::new_v4(),
        resume: false,
    };
    let machine =
        SessionMachine::new_fixture_file_approval(configuration.clone(), SessionLimits::default())
            .expect("fixture machine");
    let never = std::path::PathBuf::from("never_launched.exe");
    let result = super::actor::spawn_runtime_with_machine(
        RuntimePlan {
            binding: binding(),
            protocol: configuration,
            protocol_limits: SessionLimits::default(),
            limits: RuntimeLimits::default(),
            launch: crate::harness_dispatch::native_process::LaunchPlan {
                trampoline: never.clone(),
                executable: never.clone(),
                working_directory: std::path::PathBuf::from("never_used"),
                spec: vibemux_harness::dispatch::launch_spec::LaunchSpec {
                    executable: never,
                    arguments: Vec::new(),
                    environment_names: Vec::new(),
                },
            },
        },
        Arc::new(Capture::default()),
        machine,
        super::actor::LaunchCheck::Standard,
    );
    assert!(matches!(result, Err(RuntimeError::InvalidConfiguration)));
}

fn turn(ordinal: u128) -> TurnRequest {
    TurnRequest {
        request_id: Uuid::from_u128(100 + ordinal),
        turn_id: Uuid::from_u128(200 + ordinal),
        prompt: format!("synthetic turn {ordinal}"),
    }
}

fn owner(
    sink: Arc<Capture>,
    limits: RuntimeLimits,
) -> (RuntimeOwner, DuplexStream, Arc<AtomicUsize>) {
    let configuration = config();
    let machine = SessionMachine::new(configuration, SessionLimits::default()).expect("machine");
    let decoder = SessionDecoder::new(&SessionLimits::default()).expect("decoder");
    let (client, server) = tokio::io::duplex(4096);
    let (stdout, stdin) = tokio::io::split(client);
    let finishes = Arc::new(AtomicUsize::new(0));
    let transport = Pipes {
        stdout,
        stdin: Some(stdin),
        finishes: finishes.clone(),
    };
    let (sender, commands) = mpsc::channel(RUNTIME_INBOX_CAPACITY);
    let (close, closing) = watch::channel(None);
    let (snapshots, receiver) = watch::channel(machine.snapshot());
    let (approval_updates, approvals) = watch::channel(None);
    let shutdown_limit = limits.shutdown;
    let task = tokio::spawn(run_transport(
        binding(),
        machine,
        decoder,
        limits,
        sink,
        transport,
        commands,
        closing,
        snapshots,
        approval_updates,
    ));
    (
        RuntimeOwner {
            handle: RuntimeHandle {
                sender,
                close,
                snapshots: receiver,
                approvals,
            },
            task,
            shutdown_limit,
        },
        server,
        finishes,
    )
}

async fn write(writer: &mut WriteHalf<DuplexStream>, value: Value) {
    let mut bytes = serde_json::to_vec(&value).expect("JSON");
    bytes.push(b'\n');
    writer.write_all(&bytes).await.expect("fixture pipe");
    writer.flush().await.expect("flush");
}

#[derive(Default)]
struct VendorFacts {
    initializes: usize,
    opens: usize,
    turns: Vec<String>,
    interrupts: usize,
}

async fn vendor(server: DuplexStream, facts: Arc<Mutex<VendorFacts>>, mode: &'static str) {
    let (reader, mut writer) = tokio::io::split(server);
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut active = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await.expect("fixture read") == 0 {
            if mode == "idle_close_output" {
                // This owner exits only after its stdin closes. Output during
                // that grace must be read while unrelated tasks can progress.
                tokio::time::sleep(IDLE_CLOSE_OUTPUT_DELAY).await;
                write(
                    &mut writer,
                    json!({"method":"fixture/closing_notice","params":{}}),
                )
                .await;
            }
            return;
        }
        let value: Value = serde_json::from_str(&line).expect("runtime JSON");
        match value["method"].as_str() {
            Some("initialize") => {
                facts.lock().expect("facts").initializes += 1;
                write(
                    &mut writer,
                    json!({"id":value["id"],"result":{"userAgent":"codex/0.166.0"}}),
                )
                .await;
            }
            Some("thread/start") => {
                facts.lock().expect("facts").opens += 1;
                write(&mut writer, json!({"id":value["id"],"result":{
                    "thread":{"id":"root_thread","sessionId":"root_thread","ephemeral":false,"status":{"type":"idle"},"turns":[]},
                    "approvalPolicy":"never","sandbox":{"type":"readOnly","networkAccess":false},
                    "cwd":value["params"]["cwd"],"model":"fixture_model"}})).await;
                if mode == "silent_after_ready" {
                    std::future::pending::<()>().await;
                }
            }
            Some("turn/start") => {
                let ordinal = {
                    let mut facts = facts.lock().expect("facts");
                    assert_eq!(value["params"]["threadId"], "root_thread");
                    facts
                        .turns
                        .push(value["id"].as_str().expect("RPC ID").to_owned());
                    facts.turns.len()
                };
                active = format!("vendor_turn_{ordinal}");
                if mode == "crash" {
                    return;
                }
                write(&mut writer, json!({"id":value["id"],"result":{"turn":{"id":active,"status":"inProgress","items":[]}}})).await;
                if mode == "progress_terminal" || mode == "progress_cancel" {
                    write(&mut writer, json!({"method":"item/started","params":{
                        "threadId":"root_thread","turnId":active,"item":{
                            "type":"commandExecution","id":"live_command","command":"synthetic command",
                            "cwd":config().working_directory,"status":"inProgress","aggregatedOutput":null
                        }}})).await;
                    for _ in 0..128 {
                        write(&mut writer, json!({"method":"item/commandExecution/outputDelta","params":{
                            "threadId":"root_thread","turnId":active,"itemId":"live_command","delta":"equal output\n"
                        }})).await;
                    }
                    if mode == "progress_cancel" {
                        continue;
                    }
                }
                if mode == "hang" || mode == "cancel_race" || mode == "ignore_cancel" {
                    continue;
                }
                let thread = if mode == "wrong_correlation" {
                    "foreign_thread"
                } else {
                    "root_thread"
                };
                write(&mut writer, json!({"method":"item/agentMessage/delta","params":{"threadId":thread,"turnId":active,"itemId":"message","delta":"synthetic answer"}})).await;
                let items = if mode == "command_terminal" || mode == "progress_terminal" {
                    (0..64).map(|index|
                        json!({"type":"commandExecution","id":if mode == "progress_terminal" && index == 0 { "live_command".to_owned() } else { format!("held_command_{index}") },"command":"synthetic command",
                        "cwd":config().working_directory,"status":"completed","aggregatedOutput":"private synthetic output","exitCode":0,"durationMs":1}),
                    ).collect()
                } else {
                    Vec::new()
                };
                write(&mut writer, json!({"method":"turn/completed","params":{"threadId":thread,"turn":{"id":active,"status":"completed","items":items}}})).await;
            }
            Some("turn/interrupt") => {
                facts.lock().expect("facts").interrupts += 1;
                assert_eq!(value["params"]["threadId"], "root_thread");
                assert_eq!(value["params"]["turnId"], active);
                write(&mut writer, json!({"id":value["id"],"result":{}})).await;
                if mode == "ignore_cancel" {
                    continue;
                }
                let status = if mode == "cancel_race" {
                    "completed"
                } else {
                    "interrupted"
                };
                let items = if mode == "progress_cancel" {
                    vec![
                        json!({"type":"commandExecution","id":"live_command","command":"synthetic command",
                        "cwd":config().working_directory,"status":"completed","aggregatedOutput":"final","exitCode":0}),
                    ]
                } else {
                    vec![]
                };
                write(&mut writer, json!({"method":"turn/completed","params":{"threadId":"root_thread","turn":{"id":active,"status":status,"items":items}}})).await;
            }
            _ => {}
        }
    }
}

fn hold_progress_lock(
    sink: Arc<Capture>,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::SyncSender<()>,
    std::thread::JoinHandle<()>,
) {
    let (ready, observed) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::sync_channel(1);
    let holder = std::thread::spawn(move || {
        let held = sink
            .progress
            .lock()
            .expect("contended optional observation");
        let _ = ready.send(());
        let _ = released.recv();
        assert!(held.is_empty(), "all contended replacements were skipped");
    });
    (observed, release, holder)
}

#[tokio::test]
async fn contended_progress_flood_does_not_delay_or_drop_correlated_terminal_tools() {
    let sink = Arc::new(Capture::default());
    let (owner, server, _) = owner(sink.clone(), RuntimeLimits::default());
    let peer = tokio::spawn(vendor(
        server,
        Arc::new(Mutex::new(VendorFacts::default())),
        "progress_terminal",
    ));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    let (held, release, holder) = hold_progress_lock(sink.clone());
    held.await.expect("lock held");
    owner.handle.send(turn(1), WAIT).await.expect("send");
    timeout(Duration::from_secs(1), sink.wait_completed(1))
        .await
        .expect("no progress callback delay");
    assert!(sink.progress_calls.load(Ordering::Acquire) > 0);
    release.send(()).expect("release");
    holder.join().expect("holder join");
    {
        let events = sink.events.lock().expect("events");
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::ToolCompleted(_)))
                .count(),
            64
        );
        assert!(events.iter().any(
            |event| matches!(event, RuntimeEvent::TurnFinished { turn, output_complete: true, .. }
        if turn.outcome == Some(TurnOutcome::Completed))
        ));
    }
    owner.shutdown().await.expect("shutdown");
    timeout(WAIT, peer)
        .await
        .expect("peer EOF")
        .expect("peer join");
}

#[tokio::test]
async fn contended_progress_flood_preserves_cancellation_and_its_terminal() {
    let sink = Arc::new(Capture::default());
    let limits = RuntimeLimits {
        callback: Duration::from_millis(20),
        cancel_grace: Duration::from_millis(200),
        ..RuntimeLimits::default()
    };
    let (owner, server, _) = owner(sink.clone(), limits);
    let facts = Arc::new(Mutex::new(VendorFacts::default()));
    let peer = tokio::spawn(vendor(server, facts.clone(), "progress_cancel"));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    let (held, release, holder) = hold_progress_lock(sink.clone());
    held.await.expect("lock held");
    let request = turn(1);
    owner
        .handle
        .send(request.clone(), WAIT)
        .await
        .expect("send");
    timeout(WAIT, async {
        while sink.progress_calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("progress flood began");
    timeout(Duration::from_secs(1), owner.handle.cancel(request.turn_id))
        .await
        .expect("cancel remains responsive")
        .expect("cancel");
    timeout(Duration::from_secs(1), sink.wait_completed(1))
        .await
        .expect("terminal remains responsive");
    release.send(()).expect("release");
    holder.join().expect("holder join");
    assert!(sink.events.lock().expect("events").iter().any(|event| matches!(event,
        RuntimeEvent::TurnFinished { turn, output_complete: true, .. } if turn.outcome == Some(TurnOutcome::Cancelled))));
    assert_eq!(facts.lock().expect("facts").interrupts, 1);
    owner.shutdown().await.expect("shutdown");
    timeout(WAIT, peer)
        .await
        .expect("peer EOF")
        .expect("peer join");
}

#[tokio::test]
async fn two_turns_keep_one_transport_and_vendor_thread_until_explicit_close() {
    let sink = Arc::new(Capture::default());
    let (owner, server, finishes) = owner(sink.clone(), RuntimeLimits::default());
    let facts = Arc::new(Mutex::new(VendorFacts::default()));
    let vendor = tokio::spawn(vendor(server, facts.clone(), "normal"));
    owner
        .handle
        .wait_ready(WAIT)
        .await
        .expect("initialized owner");
    for ordinal in 1..=2 {
        owner
            .handle
            .send(turn(ordinal), WAIT)
            .await
            .expect("one send");
        sink.wait_completed(ordinal as usize).await;
        owner
            .handle
            .wait_ready(WAIT)
            .await
            .expect("ready after persisted terminal");
        assert_eq!(
            finishes.load(Ordering::Acquire),
            0,
            "turn completion does not end the process owner"
        );
    }
    let result = owner.shutdown().await.expect("bounded cleanup");
    assert!(result.cleanup.cleanup_verified);
    timeout(WAIT, vendor)
        .await
        .expect("vendor EOF")
        .expect("vendor join");
    let facts = facts.lock().expect("facts");
    assert_eq!(facts.initializes, 1);
    assert_eq!(facts.opens, 1);
    assert_eq!(facts.turns.len(), 2);
    assert_ne!(facts.turns[0], facts.turns[1]);
    assert_eq!(sink.claimed.lock().expect("claims").len(), 2);
    assert_eq!(finishes.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_tool_callback_timeout_keeps_the_same_transition_vendor_terminal_and_marks_capture_incomplete()
 {
    let sink = Arc::new(Capture::default());
    sink.hang_tool.store(true, Ordering::Release);
    let limits = RuntimeLimits {
        callback: Duration::from_millis(30),
        ..RuntimeLimits::default()
    };
    let (owner, server, finishes) = owner(sink.clone(), limits);
    let vendor = tokio::spawn(vendor(
        server,
        Arc::new(Mutex::new(VendorFacts::default())),
        "command_terminal",
    ));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    owner.handle.send(turn(1), WAIT).await.expect("one send");
    let completed = timeout(Duration::from_secs(1), async {
        loop {
            let changed = sink.changed.notified();
            if sink.completed.load(Ordering::Acquire) == 1 {
                break;
            }
            changed.await;
        }
    })
    .await;
    if completed.is_ok() {
        sink.hang_tool.store(false, Ordering::Release);
        owner
            .handle
            .wait_ready(WAIT)
            .await
            .expect("ready after incomplete display");
        owner
            .handle
            .send(turn(2), WAIT)
            .await
            .expect("next exact turn");
        sink.wait_completed(2).await;
    }
    let result = owner.shutdown().await.expect("join owned transport");
    timeout(WAIT, vendor)
        .await
        .expect("fixture EOF")
        .expect("fixture join");
    assert!(result.cleanup.cleanup_verified);
    assert_eq!(finishes.load(Ordering::Acquire), 1);
    assert!(
        completed.is_ok(),
        "private tool callback timeout must not discard the correlated terminal"
    );
    let events = sink.events.lock().expect("events");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolCompleted(tool) if tool.request_id == turn(1).request_id))
            .count(),
        1
    );
    assert!(events.iter().any(|event| matches!(event, RuntimeEvent::TurnFinished { turn, output_complete: false, .. } if turn.outcome == Some(TurnOutcome::Completed))));
    assert_eq!(events.iter().filter(|event| matches!(event, RuntimeEvent::ToolCompleted(tool) if tool.request_id == turn(2).request_id)).count(), 64, "next turn has fresh capture authority");
    assert!(events.iter().any(|event| matches!(event, RuntimeEvent::TurnFinished { turn: summary, output_complete: true, .. } if summary.request_id == turn(2).request_id)));
}

#[tokio::test]
async fn cancellation_racing_completion_preserves_correlated_completed_turn() {
    let sink = Arc::new(Capture::default());
    let (owner, server, _) = owner(sink.clone(), RuntimeLimits::default());
    let facts = Arc::new(Mutex::new(VendorFacts::default()));
    let vendor = tokio::spawn(vendor(server, facts.clone(), "cancel_race"));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    let request = turn(1);
    owner
        .handle
        .send(request.clone(), WAIT)
        .await
        .expect("send");
    owner
        .handle
        .cancel(request.turn_id)
        .await
        .expect("correlated cancellation");
    sink.wait_completed(1).await;
    owner
        .handle
        .wait_ready(WAIT)
        .await
        .expect("completed race remains reusable");
    assert!(
        sink.events
            .lock()
            .expect("events")
            .iter()
            .any(|event| matches!(event,
        RuntimeEvent::TurnFinished {turn, ..} if turn.outcome == Some(TurnOutcome::Completed)))
    );
    owner.shutdown().await.expect("shutdown");
    timeout(WAIT, vendor)
        .await
        .expect("fixture exit")
        .expect("fixture join");
    assert_eq!(facts.lock().expect("facts").interrupts, 1);
}

#[tokio::test]
async fn wrong_correlation_and_crash_keep_exact_turn_uncertain_without_replay() {
    for mode in ["wrong_correlation", "crash"] {
        let sink = Arc::new(Capture::default());
        let (owner, server, finishes) = owner(sink.clone(), RuntimeLimits::default());
        let facts = Arc::new(Mutex::new(VendorFacts::default()));
        let vendor = tokio::spawn(vendor(server, facts.clone(), mode));
        owner.handle.wait_ready(WAIT).await.expect("ready");
        let request = turn(1);
        owner
            .handle
            .send(request.clone(), WAIT)
            .await
            .expect("one send");
        let handle = owner.handle.clone();
        let result = timeout(WAIT, async {
            let mut snapshots = handle.snapshots.clone();
            loop {
                let snapshot = snapshots.borrow().clone();
                if snapshot.phase == SessionPhase::RecoveryRequired {
                    break snapshot;
                }
                snapshots.changed().await.expect("actor final status");
            }
        })
        .await
        .expect("bounded failure");
        assert_eq!(
            result.active_turn.expect("retained turn").request_id,
            request.request_id
        );
        assert_eq!(sink.completed.load(Ordering::Acquire), 0);
        let exit = owner.shutdown().await.expect("joined owner");
        assert!(exit.cleanup.cleanup_verified);
        assert_eq!(finishes.load(Ordering::Acquire), 1);
        assert_eq!(handle.send(request, WAIT).await, Err(RuntimeError::Stopped));
        assert_eq!(facts.lock().expect("facts").turns.len(), 1);
        vendor.abort();
        let _ = vendor.await;
    }
}

#[tokio::test]
async fn failed_private_text_does_not_change_a_confirmed_vendor_outcome() {
    let sink = Arc::new(Capture::default());
    sink.fail_text.store(true, Ordering::Release);
    let (owner, server, _) = owner(sink.clone(), RuntimeLimits::default());
    let vendor = tokio::spawn(vendor(
        server,
        Arc::new(Mutex::new(VendorFacts::default())),
        "normal",
    ));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    owner.handle.send(turn(1), WAIT).await.expect("send");
    sink.wait_completed(1).await;
    assert!(sink.events.lock().expect("events").iter().any(|event| matches!(event,
        RuntimeEvent::TurnFinished {turn, output_complete: false, ..} if turn.outcome == Some(TurnOutcome::Completed))));
    owner.shutdown().await.expect("shutdown");
    timeout(WAIT, vendor)
        .await
        .expect("fixture exit")
        .expect("fixture join");
}

#[tokio::test]
async fn rejected_single_use_claim_never_writes_a_vendor_turn() {
    let sink = Arc::new(Capture::default());
    sink.refuse_claim.store(true, Ordering::Release);
    let (owner, server, _) = owner(sink.clone(), RuntimeLimits::default());
    let facts = Arc::new(Mutex::new(VendorFacts::default()));
    let vendor = tokio::spawn(vendor(server, facts.clone(), "normal"));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    let request = turn(1);
    assert_eq!(
        owner.handle.send(request.clone(), WAIT).await,
        Err(RuntimeError::ClaimRefused)
    );
    let exit = owner.shutdown().await.expect("shutdown");
    assert_eq!(
        exit.pending_turn
            .expect("claim identity retained")
            .request_id,
        request.request_id
    );
    assert_eq!(facts.lock().expect("facts").turns.len(), 0);
    timeout(WAIT, vendor)
        .await
        .expect("fixture exit")
        .expect("fixture join");
}

#[tokio::test]
async fn a_timed_out_claim_retains_identity_and_never_emits_a_turn_frame() {
    let sink = Arc::new(Capture::default());
    sink.hang_claim.store(true, Ordering::Release);
    let limits = RuntimeLimits {
        callback: Duration::from_millis(20),
        ..RuntimeLimits::default()
    };
    let (owner, server, _) = owner(sink, limits);
    let facts = Arc::new(Mutex::new(VendorFacts::default()));
    let vendor = tokio::spawn(vendor(server, facts.clone(), "normal"));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    let request = turn(1);
    assert_eq!(
        owner.handle.send(request.clone(), WAIT).await,
        Err(RuntimeError::ClaimRefused)
    );
    let exit = owner.shutdown().await.expect("bounded shutdown");
    assert_eq!(
        exit.pending_turn
            .expect("unknown claim identity")
            .request_id,
        request.request_id
    );
    assert_eq!(facts.lock().expect("facts").turns.len(), 0);
    timeout(WAIT, vendor)
        .await
        .expect("fixture exit")
        .expect("fixture join");
}

#[tokio::test]
async fn hung_stdin_cannot_block_deadline_or_owned_cleanup() {
    let sink = Arc::new(Capture::default());
    let limits = RuntimeLimits {
        cancel_grace: Duration::from_millis(30),
        ..RuntimeLimits::default()
    };
    let (owner, server, finishes) = owner(sink.clone(), limits);
    let vendor = tokio::spawn(vendor(
        server,
        Arc::new(Mutex::new(VendorFacts::default())),
        "silent_after_ready",
    ));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    let mut request = turn(1);
    request.prompt = "x".repeat(32 * 1024);
    owner
        .handle
        .send(request.clone(), Duration::from_millis(30))
        .await
        .expect("bounded input queue acceptance");
    let exit = timeout(Duration::from_secs(1), async {
        let mut snapshots = owner.handle.snapshots.clone();
        loop {
            if snapshots.borrow().phase == SessionPhase::RecoveryRequired {
                break;
            }
            snapshots.changed().await.expect("final status");
        }
        owner.shutdown().await
    })
    .await
    .expect("hung stdin did not stall owner")
    .expect("joined owner");
    assert_eq!(exit.reason, RuntimeStopReason::Deadline);
    assert!(exit.cleanup.cleanup_verified);
    assert_eq!(finishes.load(Ordering::Acquire), 1);
    assert_eq!(sink.completed.load(Ordering::Acquire), 0);
    assert_eq!(
        exit.snapshot
            .active_turn
            .expect("uncertain turn retained")
            .request_id,
        request.request_id
    );
    vendor.abort();
    let _ = vendor.await;
}

#[tokio::test]
async fn queue_only_submission_survives_a_dropped_acknowledgement_receiver() {
    let sink = Arc::new(Capture::default());
    let (owner, server, _) = owner(sink.clone(), RuntimeLimits::default());
    let facts = Arc::new(Mutex::new(VendorFacts::default()));
    let vendor = tokio::spawn(vendor(server, facts.clone(), "normal"));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    owner
        .handle
        .queue_turn(turn(1), WAIT)
        .expect("prompt admission receipt");
    sink.wait_completed(1).await;
    owner
        .handle
        .wait_ready(WAIT)
        .await
        .expect("state survives the lost ACK");
    assert_eq!(sink.claimed.lock().expect("single-use claims").len(), 1);
    assert_eq!(facts.lock().expect("facts").turns.len(), 1);
    assert_eq!(
        owner
            .handle
            .snapshot()
            .last_turn
            .expect("canonical observation input")
            .request_id,
        turn(1).request_id
    );
    owner.shutdown().await.expect("shutdown");
    timeout(WAIT, vendor)
        .await
        .expect("fixture exit")
        .expect("fixture join");
}

#[tokio::test]
async fn explicit_close_intent_survives_the_following_shutdown_join() {
    let sink = Arc::new(Capture::default());
    let (owner, server, _) = owner(sink, RuntimeLimits::default());
    let vendor = tokio::spawn(vendor(
        server,
        Arc::new(Mutex::new(VendorFacts::default())),
        "normal",
    ));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    owner.handle.close();
    let exit = owner.shutdown().await.expect("join exact owner");
    assert_eq!(exit.reason, RuntimeStopReason::Closed);
    assert_eq!(exit.snapshot.phase, SessionPhase::Closed);
    timeout(WAIT, vendor)
        .await
        .expect("fixture EOF")
        .expect("fixture join");
}

#[tokio::test]
async fn repeated_cancel_requests_do_not_extend_the_original_cleanup_grace() {
    let sink = Arc::new(Capture::default());
    let limits = RuntimeLimits {
        cancel_grace: Duration::from_millis(100),
        ..RuntimeLimits::default()
    };
    let (owner, server, finishes) = owner(sink, limits);
    let facts = Arc::new(Mutex::new(VendorFacts::default()));
    let vendor = tokio::spawn(vendor(server, facts.clone(), "ignore_cancel"));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    let request = turn(1);
    owner
        .handle
        .send(request.clone(), WAIT)
        .await
        .expect("send");
    let bound = Instant::now() + Duration::from_millis(700);
    while finishes.load(Ordering::Acquire) == 0 && Instant::now() < bound {
        let _ = owner.handle.cancel(request.turn_id).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let finished_in_original_budget = finishes.load(Ordering::Acquire);
    let exit = owner.shutdown().await.expect("owned shutdown");
    vendor.abort();
    let _ = vendor.await;
    assert_eq!(
        finished_in_original_budget, 1,
        "repeated cancellation must not postpone the original bounded cleanup"
    );
    assert_eq!(exit.reason, RuntimeStopReason::CancellationUnconfirmed);
    assert_eq!(
        facts.lock().expect("facts").interrupts,
        1,
        "duplicate cancellation does not write another interrupt"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn idle_close_yields_to_peer_output_and_other_tasks_during_its_grace() {
    let limits = RuntimeLimits {
        idle: Duration::from_millis(20),
        cancel_grace: Duration::from_millis(200),
        ..RuntimeLimits::default()
    };
    let (owner, server, _) = owner(Arc::new(Capture::default()), limits);
    let vendor = tokio::spawn(vendor(
        server,
        Arc::new(Mutex::new(VendorFacts::default())),
        "idle_close_output",
    ));
    let ready = owner.handle.wait_ready(WAIT).await.expect("ready");
    let initial_records = ready.records_received;
    let mut observer_snapshots = owner.handle.snapshots.clone();
    let observer = tokio::spawn(async move {
        loop {
            let phase = observer_snapshots.borrow().phase;
            if phase == SessionPhase::Closing {
                tokio::time::sleep(IDLE_CLOSE_OBSERVER_DELAY).await;
                return observer_snapshots.borrow().phase == SessionPhase::Closing;
            }
            if matches!(phase, SessionPhase::Closed | SessionPhase::Suspended) {
                return false;
            }
            if observer_snapshots.changed().await.is_err() {
                return false;
            }
        }
    });
    let mut snapshots = owner.handle.snapshots.clone();
    timeout(WAIT, async {
        loop {
            if snapshots.borrow().phase == SessionPhase::Closed {
                break;
            }
            snapshots.changed().await.expect("idle owner status");
        }
    })
    .await
    .expect("bounded idle close");
    let exit = owner.shutdown().await.expect("join idle owner");
    let other_task_progressed = observer.await.expect("observer join");
    let orderly_peer_exit = vendor.is_finished();
    if orderly_peer_exit {
        vendor.await.expect("orderly peer EOF");
    } else {
        vendor.abort();
        let _ = vendor.await;
    }
    assert!(
        exit.snapshot.records_received > initial_records,
        "idle close must drain the peer's closing output before ending the grace; records_before={initial_records}, records_after={}, other_task_progressed={other_task_progressed}, orderly_peer_exit={orderly_peer_exit}",
        exit.snapshot.records_received
    );
    assert!(
        other_task_progressed,
        "another current-thread task must run while the owner is closing"
    );
    assert!(
        orderly_peer_exit,
        "peer must consume stdin EOF and exit during the grace"
    );
    assert_eq!(exit.reason, RuntimeStopReason::Idle);
}

#[tokio::test]
async fn close_preserves_an_earlier_active_cancellation_deadline() {
    let limits = RuntimeLimits {
        cancel_grace: Duration::from_millis(200),
        ..RuntimeLimits::default()
    };
    let (owner, server, finishes) = owner(Arc::new(Capture::default()), limits);
    let vendor = tokio::spawn(vendor(
        server,
        Arc::new(Mutex::new(VendorFacts::default())),
        "ignore_cancel",
    ));
    owner.handle.wait_ready(WAIT).await.expect("ready");
    let request = turn(1);
    owner
        .handle
        .send(request.clone(), WAIT)
        .await
        .expect("send");
    owner
        .handle
        .cancel(request.turn_id)
        .await
        .expect("first cancel");
    tokio::time::sleep(Duration::from_millis(150)).await;
    owner.handle.close();
    let mut snapshots = owner.handle.snapshots.clone();
    let kept_earlier_grace = timeout(Duration::from_millis(100), async {
        while finishes.load(Ordering::Acquire) == 0 {
            snapshots.changed().await.expect("owner lifecycle");
        }
    })
    .await
    .is_ok();
    let exit = owner.shutdown().await.expect("bounded owner join");
    vendor.abort();
    let _ = vendor.await;
    assert!(
        kept_earlier_grace,
        "explicit close must not renew a cancellation already in progress"
    );
    assert_eq!(exit.reason, RuntimeStopReason::Closed);
}
