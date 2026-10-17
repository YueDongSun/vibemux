#![cfg(feature = "test_helpers")]

use serde_json::{Value, json};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, atomic::Ordering},
    task::{Context, Poll},
    time::Duration,
};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf},
    sync::Notify,
    task::JoinHandle,
    time::{Instant, Sleep, timeout},
};
use vibemux_mcp::{
    config::{HostConfig, MAX_OUTPUT_BYTES},
    fixture_support::{FixtureMode, LaunchControl, LaunchGate},
    framing::LineReader,
    supervisor::Supervisor,
    tools::ReadOperation,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_mcp_fixture");
const CALL_DEADLINE: Duration = Duration::from_secs(3);
const RESPONSIVE_DEADLINE: Duration = Duration::from_millis(700);
/// Bound on a real OS launch reaching its gate. A freshly linked fixture can
/// take over a second to start cold, which is not a responsiveness guarantee.
const LAUNCH_ENTRY_DEADLINE: Duration = Duration::from_secs(10);

fn config(granted: bool) -> HostConfig {
    HostConfig {
        descriptor: PathBuf::from("unused_fixture_descriptor"),
        allow_metadata_read: granted,
    }
}
fn supervisor(mode: FixtureMode, granted: bool) -> Supervisor {
    Supervisor::for_fixture(config(granted), PathBuf::from(FIXTURE), mode, CALL_DEADLINE)
}

fn process_start(pid: u32) -> Option<u64> {
    let mut system = System::new();
    let pid = Pid::from_u32(pid);
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(|process| process.start_time())
}

#[tokio::test]
async fn denied_owner_cannot_launch_a_reader() {
    let mut owner = supervisor(FixtureMode::Healthy, false);
    let result = owner.read(ReadOperation::Health).await;
    let launches = owner.view().launches;
    owner.shutdown().await.unwrap();
    assert!(result.is_err());
    assert_eq!(launches, 0);
}

#[tokio::test]
async fn faults_and_partial_startup_reap_the_exact_owned_child() {
    for (mode, expected) in [
        (FixtureMode::Crash, "reader_exited"),
        (FixtureMode::Malformed, "reader_frame_invalid"),
        (FixtureMode::Oversized, "frame_too_large"),
        (FixtureMode::StaleGeneration, "reader_generation_mismatch"),
        (FixtureMode::NoOutputPipe, "reader_pipe_unavailable"),
        (FixtureMode::RejectContainment, "reader_containment_failed"),
    ] {
        let mut owner = supervisor(mode, true);
        let result = owner.read(ReadOperation::Health).await;
        let observed = owner.terminal_observer().unwrap();
        owner.unload("fixture_fault").await.unwrap();
        assert!(
            observed.load(Ordering::Acquire),
            "retained Child wait must observe exit"
        );
        assert_eq!(result.unwrap_err(), expected);
        assert!(!owner.view().available);
    }
}

#[tokio::test]
async fn non_object_success_is_rejected_as_a_malformed_reader_frame() {
    let mut owner = supervisor(FixtureMode::NonObjectResult, true);
    let result = owner.read(ReadOperation::Health).await;
    let returned_error = result.as_ref().err().copied();
    let observed = owner.terminal_observer().unwrap();
    owner.unload("fixture_result_check").await.unwrap();
    assert!(observed.load(Ordering::Acquire));
    assert!(!owner.view().available);
    assert_eq!(returned_error, Some("reader_frame_invalid"));
}

#[tokio::test]
async fn shutdown_terminates_descendants_and_reaps_the_retained_leader() {
    let mut owner = supervisor(FixtureMode::Descendant, true);
    let reply = owner.read(ReadOperation::Health).await.unwrap();
    let leader = owner.owned_child_id().unwrap();
    let descendant = reply["descendant_pid"].as_u64().unwrap() as u32;
    let leader_start = process_start(leader).expect("observed owned leader identity");
    let descendant_start = process_start(descendant).expect("observed owned descendant identity");
    let observed = owner.terminal_observer().unwrap();
    owner.shutdown().await.unwrap(); // ProcessTree termination precedes retained Child wait.
    assert!(observed.load(Ordering::Acquire));
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_start(descendant) == Some(descendant_start) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(process_start(leader) != Some(leader_start));
    assert!(
        process_start(descendant) != Some(descendant_start),
        "owned tree termination must end the exact fixture descendant"
    );
}

#[tokio::test]
async fn owner_drop_observes_terminal_child_without_a_detached_cleanup_task() {
    let mut owner = supervisor(FixtureMode::Healthy, true);
    owner.read(ReadOperation::Health).await.unwrap();
    let observed = owner.terminal_observer().unwrap();
    drop(owner);
    assert!(
        observed.load(Ordering::Acquire),
        "Drop must actually observe retained Child try_wait"
    );
}

#[tokio::test]
async fn a_reader_timeout_is_bounded_and_then_reaped() {
    let mut owner = Supervisor::for_fixture(
        config(true),
        PathBuf::from(FIXTURE),
        FixtureMode::Hang,
        Duration::from_millis(150),
    );
    let started = Instant::now();
    let result = owner.read(ReadOperation::Health).await;
    let observed = owner.terminal_observer().unwrap();
    owner.unload("reader_timeout").await.unwrap();
    assert_eq!(result.unwrap_err(), "reader_timeout");
    assert!(observed.load(Ordering::Acquire));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn successful_reads_do_not_replenish_the_session_restart_budget() {
    let mut owner = supervisor(FixtureMode::Healthy, true);
    for attempt in 1..=4 {
        let reply = owner.read(ReadOperation::Health).await.unwrap();
        assert_eq!(reply["sequence"], 1);
        let observed = owner.terminal_observer().unwrap();
        owner.unload("fixture_restart").await.unwrap();
        assert!(observed.load(Ordering::Acquire));
        assert_eq!(owner.view().launches, attempt);
        tokio::time::sleep(Duration::from_millis(210)).await;
        owner.finish_backoff();
    }
    let refused = owner.read(ReadOperation::Health).await;
    let launches = owner.view().launches;
    owner.shutdown().await.unwrap();
    assert!(refused.is_err());
    assert_eq!(launches, 4);
}

struct SessionFixture {
    input: Option<WriteHalf<DuplexStream>>,
    output: LineReader<BufReader<ReadHalf<DuplexStream>>>,
    owner: JoinHandle<Result<(), &'static str>>,
}

struct DelayedOutput<W> {
    inner: W,
    signal: Option<Arc<Notify>>,
    delay: Option<Pin<Box<Sleep>>>,
    delayed: bool,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for DelayedOutput<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if !this.delayed
            && this.delay.is_none()
            && this.signal.is_some()
            && serde_json::from_slice::<Value>(bytes)
                .is_ok_and(|value| value["result"]["isError"] == true)
        {
            this.delay = Some(Box::pin(tokio::time::sleep(Duration::from_millis(350))));
            this.signal.as_ref().unwrap().notify_one();
        }
        if let Some(delay) = this.delay.as_mut() {
            if delay.as_mut().poll(context).is_pending() {
                return Poll::Pending;
            }
            this.delay = None;
            this.delayed = true;
        }
        Pin::new(&mut this.inner).poll_write(context, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(context)
    }
    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }
}

impl SessionFixture {
    async fn start(mode: FixtureMode, delayed_error: Option<Arc<Notify>>) -> Self {
        Self::start_owned(supervisor(mode, true), delayed_error).await
    }

    async fn start_owned(supervisor: Supervisor, delayed_error: Option<Arc<Notify>>) -> Self {
        let (client, server) = tokio::io::duplex(8192);
        let (input, output) = tokio::io::split(server);
        let output = DelayedOutput {
            inner: output,
            signal: delayed_error,
            delay: None,
            delayed: false,
        };
        let owner = tokio::spawn(vibemux_mcp::session::run_fixture(
            BufReader::new(input),
            output,
            config(true),
            supervisor,
        ));
        let (output, input) = tokio::io::split(client);
        let mut fixture = Self {
            input: Some(input),
            output: LineReader::new(BufReader::new(output), MAX_OUTPUT_BYTES),
            owner,
        };
        fixture.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}})).await;
        fixture.receive().await.expect("initialize");
        fixture
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        fixture
    }
    async fn send(&mut self, message: Value) {
        self.input
            .as_mut()
            .unwrap()
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn receive(&mut self) -> Option<Value> {
        let bytes = timeout(Duration::from_secs(5), self.output.next())
            .await
            .ok()?
            .ok()??;
        serde_json::from_slice(&bytes).ok()
    }
    async fn health(&mut self, id: i32) {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"health","arguments":{}}})).await;
    }
    async fn finish(mut self) -> (bool, Vec<Value>) {
        if let Some(mut input) = self.input.take() {
            input.shutdown().await.unwrap();
        }
        let responsive = timeout(RESPONSIVE_DEADLINE, &mut self.owner).await;
        let quickly = responsive.is_ok();
        match responsive {
            Ok(result) => assert!(result.unwrap().is_ok()),
            Err(_) => match timeout(Duration::from_secs(6), &mut self.owner).await {
                Ok(result) => assert!(result.unwrap().is_ok()),
                Err(_) => {
                    self.owner.abort();
                    let _ = self.owner.await;
                    panic!("session cleanup did not finish");
                }
            },
        }
        let mut messages = Vec::new();
        while let Some(message) = self.receive().await {
            messages.push(message);
        }
        (quickly, messages)
    }
}

#[tokio::test]
async fn slow_os_launch_keeps_ping_responsive() {
    let gate = LaunchGate::default();
    let owner = supervisor(FixtureMode::Healthy, true).with_launch_control(LaunchControl {
        before_spawn: Some(gate.clone()),
        ..Default::default()
    });
    let mut fixture = SessionFixture::start_owned(owner, None).await;
    let started = Instant::now();
    fixture.health(2).await;
    let entered = entered_gate(&gate).await;
    fixture
        .send(json!({"jsonrpc":"2.0","id":3,"method":"ping"}))
        .await;
    let response = timeout(RESPONSIVE_DEADLINE, fixture.receive())
        .await
        .ok()
        .flatten();
    let elapsed = started.elapsed();
    gate.release();
    fixture.finish().await;
    assert!(entered);
    assert!(
        elapsed < RESPONSIVE_DEADLINE,
        "OS launch blocked foreground for {elapsed:?}"
    );
    assert_eq!(response.unwrap()["id"], 3);
}

async fn entered_gate(gate: &LaunchGate) -> bool {
    timeout(LAUNCH_ENTRY_DEADLINE, async {
        while !gate.entered() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .is_ok()
}

fn gated_owner(
    gate: &LaunchGate,
    read_deadline: Duration,
) -> (Supervisor, Arc<std::sync::atomic::AtomicBool>) {
    let control = LaunchControl {
        after_containment: Some(gate.clone()),
        ..Default::default()
    };
    let observed = Arc::clone(&control.terminal_observed);
    let owner = Supervisor::for_fixture(
        config(true),
        PathBuf::from(FIXTURE),
        FixtureMode::Healthy,
        read_deadline,
    )
    .with_launch_control(control);
    (owner, observed)
}

#[tokio::test]
async fn gated_timeout_keeps_host_requests_available_until_cleanup_and_full_backoff() {
    let gate = LaunchGate::default();
    let (owner, observed) = gated_owner(&gate, Duration::from_millis(150));
    let mut fixture = SessionFixture::start_owned(owner, None).await;
    fixture.health(2).await;
    let entered = entered_gate(&gate).await;
    let failure = timeout(RESPONSIVE_DEADLINE, fixture.receive())
        .await
        .ok()
        .flatten();
    let unloaded = fixture.receive().await;
    fixture.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"host_status","arguments":{}}})).await;
    let status = fixture.receive().await;
    fixture
        .send(json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}))
        .await;
    let tools = fixture.receive().await;
    fixture
        .send(json!({"jsonrpc":"2.0","id":5,"method":"ping"}))
        .await;
    let ping = fixture.receive().await;
    fixture.health(6).await;
    let refused = fixture.receive().await;
    let terminal_before_release = observed.load(Ordering::Acquire);
    let released_at = Instant::now();
    gate.release();
    let restored = fixture.receive().await;
    let recovery_elapsed = released_at.elapsed();
    fixture.finish().await;
    assert!(entered);
    assert_eq!(failure.unwrap()["id"], 2);
    assert_eq!(
        unloaded.unwrap()["method"],
        "notifications/tools/list_changed"
    );
    let status = status.unwrap();
    assert_eq!(
        status["result"]["structuredContent"]["backend_state"],
        "cleanup_pending"
    );
    assert_eq!(
        status["result"]["structuredContent"]["last_error"],
        "reader_timeout"
    );
    assert_eq!(status["result"]["structuredContent"]["worker_launches"], 1);
    assert_eq!(
        tools.unwrap()["result"]["tools"].as_array().unwrap().len(),
        1
    );
    assert_eq!(ping.unwrap()["id"], 5);
    assert_eq!(refused.unwrap()["result"]["isError"], true);
    assert!(!terminal_before_release);
    assert!(observed.load(Ordering::Acquire));
    assert_eq!(
        restored.unwrap()["method"],
        "notifications/tools/list_changed"
    );
    assert!(recovery_elapsed >= Duration::from_millis(200));
}

#[tokio::test]
async fn gated_cancellation_unloads_promptly_and_never_replies_to_cancelled_id() {
    let gate = LaunchGate::default();
    let (owner, observed) = gated_owner(&gate, CALL_DEADLINE);
    let mut fixture = SessionFixture::start_owned(owner, None).await;
    fixture.health(2).await;
    let entered = entered_gate(&gate).await;
    fixture
        .send(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}))
        .await;
    let busy = fixture.receive().await;
    fixture
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}))
        .await;
    let unloaded = timeout(RESPONSIVE_DEADLINE, fixture.receive())
        .await
        .ok()
        .flatten();
    fixture.send(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"host_status","arguments":{}}})).await;
    let status = fixture.receive().await;
    gate.release();
    let (_, remaining) = fixture.finish().await;
    assert!(entered);
    assert_eq!(busy.unwrap()["error"]["data"]["code"], "request_busy");
    assert_eq!(
        unloaded.unwrap()["method"],
        "notifications/tools/list_changed"
    );
    let status = status.unwrap();
    assert_eq!(status["result"]["structuredContent"]["worker_launches"], 1);
    assert_eq!(
        status["result"]["structuredContent"]["backend_state"],
        "cleanup_pending"
    );
    assert!(observed.load(Ordering::Acquire));
    assert!(
        !remaining
            .iter()
            .any(|message| message.get("id") == Some(&json!(2)))
    );
}

#[tokio::test]
async fn eof_waits_for_pending_launch_then_observes_its_child_terminal() {
    let gate = LaunchGate::default();
    let (owner, observed) = gated_owner(&gate, CALL_DEADLINE);
    let mut fixture = SessionFixture::start_owned(owner, None).await;
    fixture.health(2).await;
    let entered = entered_gate(&gate).await;
    fixture.input.take().unwrap().shutdown().await.unwrap();
    let early = timeout(Duration::from_millis(80), &mut fixture.owner).await;
    let waited = early.is_err();
    gate.release();
    if waited {
        timeout(Duration::from_secs(3), &mut fixture.owner)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    let reply = fixture.receive().await;
    assert!(entered);
    assert!(waited, "EOF may not abandon the launch owner");
    assert!(observed.load(Ordering::Acquire));
    assert!(
        reply.is_none(),
        "final cleanup must not restore tools or emit the cancelled reply"
    );
}

#[tokio::test]
async fn dropping_pending_launch_joins_and_reaps_from_a_separate_thread() {
    let gate = LaunchGate::default();
    let (mut owner, observed) = gated_owner(&gate, Duration::from_millis(80));
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dropped_by_owner = Arc::clone(&dropped);
    let drop_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let _ = owner.read(ReadOperation::Health).await;
        });
        drop(owner);
        dropped_by_owner.store(true, Ordering::Release);
    });
    let entered = entered_gate(&gate).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    let joined_early = dropped.load(Ordering::Acquire);
    gate.release();
    timeout(Duration::from_secs(4), async {
        while !drop_thread.is_finished() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop_thread.join().unwrap();
    assert!(entered);
    assert!(!joined_early);
    assert!(dropped.load(Ordering::Acquire));
    assert!(observed.load(Ordering::Acquire));
}

#[tokio::test]
async fn failed_os_launches_consume_every_session_attempt() {
    let mut owner = Supervisor::for_fixture(
        config(true),
        PathBuf::from("missing_mcp_fixture_executable"),
        FixtureMode::Healthy,
        CALL_DEADLINE,
    );
    for attempt in 1..=4 {
        assert_eq!(
            owner.read(ReadOperation::Health).await,
            Err("reader_spawn_failed")
        );
        owner.unload("reader_spawn_failed").await.unwrap();
        assert_eq!(owner.view().launches, attempt);
        tokio::time::sleep(Duration::from_millis(210)).await;
        owner.finish_backoff();
    }
    assert_eq!(owner.view().state, "exhausted");
    assert!(!owner.view().available);
    owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_reads_consume_every_session_attempt() {
    let mut owner = supervisor(FixtureMode::Hang, true);
    for attempt in 1..=4 {
        assert!(
            timeout(Duration::from_millis(25), owner.read(ReadOperation::Health))
                .await
                .is_err()
        );
        let observed = owner.terminal_observer().unwrap();
        assert!(!observed.load(Ordering::Acquire));
        owner.unload("request_cancelled").await.unwrap();
        assert!(observed.load(Ordering::Acquire));
        assert_eq!(owner.view().launches, attempt);
        tokio::time::sleep(Duration::from_millis(210)).await;
        owner.finish_backoff();
    }
    assert_eq!(owner.view().state, "exhausted");
    assert!(!owner.view().available);
    owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn late_spawn_error_preserves_the_original_timeout_without_a_second_reply() {
    let gate = LaunchGate::default();
    let owner = Supervisor::for_fixture(
        config(true),
        PathBuf::from("missing_mcp_fixture_executable"),
        FixtureMode::Healthy,
        Duration::from_millis(100),
    )
    .with_launch_control(LaunchControl {
        before_spawn: Some(gate.clone()),
        ..Default::default()
    });
    let mut fixture = SessionFixture::start_owned(owner, None).await;
    fixture.health(2).await;
    let entered = entered_gate(&gate).await;
    let failure = fixture.receive().await.unwrap();
    let unloaded = fixture.receive().await.unwrap();
    gate.release();
    let restored = fixture.receive().await.unwrap();
    fixture.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"host_status","arguments":{}}})).await;
    let status = fixture.receive().await.unwrap();
    let (_, remaining) = fixture.finish().await;
    assert!(entered);
    assert_eq!(failure["id"], 2);
    assert!(
        failure["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("reader_timeout")
    );
    assert_eq!(unloaded["method"], "notifications/tools/list_changed");
    assert_eq!(restored["method"], "notifications/tools/list_changed");
    assert_eq!(
        status["result"]["structuredContent"]["last_error"],
        "reader_timeout"
    );
    assert_eq!(status["result"]["structuredContent"]["worker_launches"], 1);
    assert!(
        !remaining
            .iter()
            .any(|message| message.get("id") == Some(&json!(2)))
    );
}

#[tokio::test]
async fn cleanup_failure_closes_session_without_restoring_tools() {
    let control = LaunchControl {
        fail_cleanup: true,
        ..Default::default()
    };
    let observed = Arc::clone(&control.terminal_observed);
    let owner = supervisor(FixtureMode::HealthyThenHang, true).with_launch_control(control);
    let mut fixture = SessionFixture::start_owned(owner, None).await;
    fixture.health(2).await;
    fixture.receive().await.unwrap();
    fixture.health(3).await;
    fixture
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}))
        .await;
    let result = timeout(RESPONSIVE_DEADLINE, &mut fixture.owner).await;
    if result.is_err() {
        fixture.input.take().unwrap().shutdown().await.unwrap();
        let _ = timeout(Duration::from_secs(3), &mut fixture.owner).await;
    }
    let mut messages = Vec::new();
    while let Some(message) = fixture.receive().await {
        messages.push(message);
    }
    assert_eq!(result.unwrap().unwrap(), Err("reader_cleanup_failed"));
    assert!(observed.load(Ordering::Acquire));
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "notifications/tools/list_changed")
            .count(),
        1
    );
    assert!(
        !messages
            .iter()
            .any(|message| message.get("id") == Some(&json!(3)))
    );
}

#[tokio::test]
async fn interrupted_cleanup_retains_child_and_does_not_extend_its_deadline() {
    let gate = LaunchGate::default();
    let control = LaunchControl {
        before_reap: Some(gate.clone()),
        ..Default::default()
    };
    let observed = Arc::clone(&control.terminal_observed);
    let owner = supervisor(FixtureMode::HealthyThenHang, true).with_launch_control(control);
    let mut fixture = SessionFixture::start_owned(owner, None).await;
    fixture.health(2).await;
    fixture.receive().await.unwrap();
    fixture.health(3).await;
    let cancelled_at = Instant::now();
    fixture
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}))
        .await;
    let unloaded = fixture.receive().await.unwrap();
    let entered = entered_gate(&gate).await;
    let mut retained = true;
    for id in 10..70 {
        fixture
            .send(json!({"jsonrpc":"2.0","id":id,"method":"ping"}))
            .await;
        let reply = fixture.receive().await.unwrap();
        assert_eq!(reply["id"], id);
        retained &= !observed.load(Ordering::Acquire);
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    let result = tokio::time::timeout_at(
        cancelled_at + Duration::from_millis(2600),
        &mut fixture.owner,
    )
    .await;
    gate.release();
    if result.is_err() {
        fixture.input.take().unwrap().shutdown().await.unwrap();
        let _ = timeout(Duration::from_secs(3), &mut fixture.owner).await;
    }
    let reply = fixture.receive().await;
    assert!(entered);
    assert!(
        retained,
        "losing select branches must not drop the retained child"
    );
    assert_eq!(unloaded["method"], "notifications/tools/list_changed");
    assert_eq!(result.unwrap().unwrap(), Err("reader_cleanup_failed"));
    assert!(observed.load(Ordering::Acquire));
    assert!(
        reply.is_none(),
        "failed cleanup cannot restore tools or reply to cancellation"
    );
}

#[tokio::test]
async fn cancellation_is_responsive_and_suppresses_the_cancelled_response() {
    let mut fixture = SessionFixture::start(FixtureMode::HealthyThenHang, None).await;
    fixture.health(2).await;
    fixture.receive().await.expect("healthy owned worker");
    fixture.health(3).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    fixture
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}))
        .await;
    fixture
        .send(json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}))
        .await;
    let response = timeout(RESPONSIVE_DEADLINE, fixture.receive())
        .await
        .ok()
        .flatten();
    let (_, remaining) = fixture.finish().await;
    assert!(
        response.is_some(),
        "cancellation must interrupt the reader deadline"
    );
    assert!(
        !response
            .iter()
            .chain(&remaining)
            .any(|reply| reply.get("id") == Some(&json!(3)))
    );
}

#[tokio::test]
async fn eof_interrupts_an_active_reader_and_joins_cleanup() {
    let mut fixture = SessionFixture::start(FixtureMode::HealthyThenHang, None).await;
    fixture.health(2).await;
    fixture.receive().await.expect("healthy owned worker");
    fixture.health(3).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (quickly, remaining) = fixture.finish().await;
    assert!(
        quickly,
        "EOF must stop the active reader before its deadline"
    );
    assert!(remaining.is_empty());
}

#[tokio::test]
async fn idle_worker_exit_unloads_capability_without_a_new_read() {
    let mut fixture = SessionFixture::start(FixtureMode::IdleCrash, None).await;
    fixture.health(2).await;
    fixture.receive().await.expect("healthy owned worker");
    let changed = timeout(RESPONSIVE_DEADLINE, fixture.receive())
        .await
        .ok()
        .flatten();
    fixture.finish().await;
    assert_eq!(
        changed.as_ref().map(|value| &value["method"]),
        Some(&json!("notifications/tools/list_changed"))
    );
}

#[tokio::test]
async fn buffered_requests_after_a_slow_error_do_not_starve_expired_backoff() {
    let signal = Arc::new(Notify::new());
    let mut fixture = SessionFixture::start(FixtureMode::Crash, Some(Arc::clone(&signal))).await;
    fixture.health(2).await;
    timeout(Duration::from_secs(2), signal.notified())
        .await
        .expect("error write began after unload");
    fixture
        .send(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}))
        .await;
    let mut listed = None;
    for _ in 0..4 {
        let reply = fixture.receive().await.expect("bounded reply");
        if reply.get("id") == Some(&json!(3)) {
            listed = Some(reply);
            break;
        }
    }
    fixture.finish().await;
    assert_eq!(
        listed.unwrap()["result"]["tools"].as_array().unwrap().len(),
        5,
        "an already-expired backoff must run before the next buffered request"
    );
}

#[tokio::test]
async fn concurrent_requests_are_bounded_and_unrelated_cancellation_is_ignored() {
    let mut fixture = SessionFixture::start(FixtureMode::HealthyThenHang, None).await;
    fixture.health(2).await;
    fixture.receive().await.unwrap();
    fixture.health(3).await;
    fixture
        .send(
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":999}}),
        )
        .await;
    fixture.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3,"reason":7}})).await;
    fixture
        .send(json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}))
        .await;
    let busy = fixture.receive().await.unwrap();
    fixture
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}))
        .await;
    let (_, remaining) = fixture.finish().await;
    assert_eq!(busy["id"], 4);
    assert_eq!(busy["error"]["data"]["code"], "request_busy");
    assert!(
        !remaining
            .iter()
            .any(|reply| reply.get("id") == Some(&json!(3)))
    );
}
