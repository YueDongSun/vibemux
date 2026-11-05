use super::*;
use async_trait::async_trait;
use std::{
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream},
    sync::Notify,
    time::timeout,
};
use vibemux_harness::dispatch::ProcessExit;

const WAIT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Shared {
    writer_dropped: AtomicBool,
    finished: AtomicBool,
    forced: AtomicBool,
    root_exited: AtomicBool,
    read_failed: AtomicBool,
    bytes_read: AtomicUsize,
    read_notice: Notify,
    input_bytes: Mutex<Vec<u8>>,
}

#[derive(Default)]
struct Gate {
    open: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Gate {
    fn release(&self) {
        self.open.store(true, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().expect("gate waker").take() {
            waker.wake();
        }
    }
}

struct OwnedInput {
    pipe: DuplexStream,
    shared: Arc<Shared>,
    gate: Arc<Gate>,
    held_flush: Option<usize>,
    flush_index: usize,
    flushing: bool,
    fail: bool,
}

impl Drop for OwnedInput {
    fn drop(&mut self) {
        self.shared.writer_dropped.store(true, Ordering::SeqCst);
    }
}

impl AsyncWrite for OwnedInput {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.fail {
            return Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)));
        }
        match Pin::new(&mut self.pipe).poll_write(context, bytes) {
            Poll::Ready(Ok(written)) => {
                self.shared
                    .input_bytes
                    .lock()
                    .expect("captured input")
                    .extend_from_slice(&bytes[..written]);
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.flushing {
            self.flushing = true;
            self.flush_index += 1;
        }
        if self.held_flush == Some(self.flush_index) && !self.gate.open.load(Ordering::SeqCst) {
            *self.gate.waker.lock().expect("gate waker") = Some(context.waker().clone());
            if !self.gate.open.load(Ordering::SeqCst) {
                return Poll::Pending;
            }
        }
        match Pin::new(&mut self.pipe).poll_flush(context) {
            Poll::Ready(result) => {
                self.flushing = false;
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.pipe).poll_shutdown(context)
    }
}

struct Transport {
    input: Option<crate::chat_session::input_writer::Stdin>,
    output: DuplexStream,
    shared: Arc<Shared>,
}

#[async_trait]
impl RuntimeTransport for Transport {
    fn take_stdin(&mut self) -> Option<crate::chat_session::input_writer::Stdin> {
        self.input.take()
    }

    fn root_exited(&mut self) -> Result<bool, crate::chat_session::RuntimeError> {
        Ok(self.shared.root_exited.load(Ordering::SeqCst))
    }

    async fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.shared.read_failed.load(Ordering::SeqCst) {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        let bytes = self.output.read(buffer).await?;
        self.shared.bytes_read.fetch_add(bytes, Ordering::SeqCst);
        self.shared.read_notice.notify_one();
        Ok(bytes)
    }

    async fn finish(&mut self, force: bool, _deadline: Instant) -> RuntimeCleanup {
        self.shared.finished.store(true, Ordering::SeqCst);
        self.shared.forced.store(force, Ordering::SeqCst);
        let joined = self.shared.writer_dropped.load(Ordering::SeqCst);
        RuntimeCleanup {
            exit: ProcessExit {
                exit_code: Some(0),
                forced_termination: force,
            },
            root_reaped: true,
            tree_empty: true,
            cleanup_verified: joined,
            stderr_bytes: 7,
        }
    }
}

struct Peer {
    input: BufReader<DuplexStream>,
    output: DuplexStream,
}

fn setup(held_flush: Option<usize>, fail: bool) -> (Transport, Peer, Arc<Shared>, Arc<Gate>) {
    let (stdin, peer_input) = tokio::io::duplex(8192);
    let (stdout, peer_output) = tokio::io::duplex(8192);
    let shared = Arc::new(Shared::default());
    let gate = Arc::new(Gate::default());
    let transport = Transport {
        input: Some(Box::new(OwnedInput {
            pipe: stdin,
            shared: shared.clone(),
            gate: gate.clone(),
            held_flush,
            flush_index: 0,
            flushing: false,
            fail,
        })),
        output: stdout,
        shared: shared.clone(),
    };
    (
        transport,
        Peer {
            input: BufReader::new(peer_input),
            output: peer_output,
        },
        shared,
        gate,
    )
}

fn working_directory() -> String {
    if cfg!(windows) {
        "C:\\private_diagnostic".into()
    } else {
        "/private_diagnostic".into()
    }
}

fn context() -> ProbeContext {
    ProbeContext {
        session_id: Uuid::from_u128(1),
        user_id: Uuid::from_u128(2),
        working_directory: working_directory(),
        prompt: "private diagnostic prompt".into(),
        limits: super::super::DiagnosticLimits {
            initialize: Duration::from_secs(1),
            turn: Duration::from_secs(1),
            shutdown: Duration::from_millis(100),
            ..super::super::DiagnosticLimits::default()
        },
    }
}

async fn frame(input: &mut BufReader<DuplexStream>) -> Value {
    let mut line = String::new();
    assert!(input.read_line(&mut line).await.expect("peer input") > 0);
    serde_json::from_str(&line).expect("input JSON")
}

async fn send(output: &mut DuplexStream, value: Value) -> usize {
    let mut bytes = serde_json::to_vec(&value).expect("synthetic JSON");
    bytes.push(b'\n');
    output.write_all(&bytes).await.expect("peer output");
    bytes.len()
}

fn response(initialize: &Value) -> Value {
    json!({"type":"control_response","response":{"subtype":"success",
        "request_id":initialize["request_id"],"response":{"claude_code_version":"2.1.304"}}})
}

fn init() -> Value {
    json!({"type":"system","subtype":"init","uuid":"private_init","session_id":Uuid::from_u128(1),
        "cwd":working_directory(),"permissionMode":"manual","tools":["Write"],"mcp_servers":[],"plugins":[]})
}

fn tool(echo: bool) -> Value {
    let mut value = json!({"type":"assistant","uuid":"private_assistant",
        "session_id":Uuid::from_u128(1),"parent_tool_use_id":null,
        "message":{"id":"private_message","role":"assistant","content":[{
            "type":"tool_use","id":"private_tool","name":"Write",
            "input":{"file_path":"private_candidate","content":"private body"}}]}});
    if echo {
        value["user_message_uuid"] = json!(Uuid::from_u128(2));
    }
    value
}

fn request() -> Value {
    json!({"type":"control_request","request_id":"private_request","request":{
        "subtype":"can_use_tool","tool_name":"Write","tool_use_id":"private_tool",
        "input":{"file_path":"private_candidate","content":"private body"}}})
}

fn terminal() -> Value {
    json!({"type":"result","uuid":"private_terminal","session_id":Uuid::from_u128(1),
        "subtype":"success","is_error":false,"terminal_reason":"completed"})
}

async fn initialized(peer: &mut Peer) -> Value {
    let initialize = frame(&mut peer.input).await;
    assert_eq!(initialize["type"], "control_request");
    send(&mut peer.output, response(&initialize)).await;
    let user = frame(&mut peer.input).await;
    assert_eq!(user["type"], "user");
    assert_eq!(user["uuid"], Uuid::from_u128(2).to_string());
    assert_eq!(user["session_id"], Uuid::from_u128(1).to_string());
    user
}

fn assert_deny(value: &Value) {
    assert_eq!(
        value,
        &json!({"type":"control_response","response":{
        "subtype":"success","request_id":"private_request",
        "response":{"behavior":"deny","message":"Diagnostic denied"}}})
    );
}

fn assert_cleanup(report: &DiagnosticReport, shared: &Shared) {
    assert!(shared.writer_dropped.load(Ordering::SeqCst));
    assert!(shared.finished.load(Ordering::SeqCst));
    assert!(shared.forced.load(Ordering::SeqCst));
    assert!(report.cleanup.cleanup_verified);
    assert!(report.cleanup.root_reaped);
    assert!(report.cleanup.tree_empty);
    assert_eq!(report.cleanup.stderr_bytes, 7);
}

fn report_json(report: &DiagnosticReport) -> Value {
    serde_json::to_value(report).expect("content-free diagnostic report")
}

fn assert_report_flag(report: &DiagnosticReport, name: &str, expected: bool) {
    let public = report_json(report);
    assert_eq!(
        public.get(name),
        Some(&json!(expected)),
        "diagnostic report field {name}"
    );
}

fn assert_init_checks(report: &DiagnosticReport, permission: bool, directory: bool, tools: bool) {
    let public = report_json(report);
    assert_eq!(
        public["observations"]["init_checks"],
        json!({
            "permission_mode_matches": permission,
            "working_directory_matches": directory,
            "tools_match": tools
        })
    );
}

fn assert_no_init_checks(report: &DiagnosticReport) {
    let public = report_json(report);
    assert_eq!(
        public["observations"].get("init_checks"),
        Some(&Value::Null)
    );
}

async fn wait_read(shared: &Shared, expected: usize) {
    loop {
        let notified = shared.read_notice.notified();
        if shared.bytes_read.load(Ordering::SeqCst) >= expected {
            return;
        }
        notified.await;
    }
}

#[tokio::test]
async fn one_generated_input_and_one_fixed_denial_leave_only_redacted_observations() {
    let (transport, mut peer, shared, _gate) = setup(None, false);
    let (_owner, cancellation) = watch::channel(false);
    let (report, ()) = timeout(WAIT, async {
        tokio::join!(drive_probe(transport, context(), cancellation), async {
            initialized(&mut peer).await;
            send(&mut peer.output, init()).await;
            send(&mut peer.output, tool(true)).await;
            send(&mut peer.output, request()).await;
            assert_deny(&frame(&mut peer.input).await);
            send(&mut peer.output, terminal()).await;
        })
    })
    .await
    .expect("bounded probe");
    assert_eq!(report.stop, DiagnosticStop::ObservedTerminal);
    assert!(report.initialize_written && report.input_written && report.denial_written);
    assert_eq!(report_json(&report)["report_version"], 3);
    assert_report_flag(&report, "initialize_accepted", true);
    assert_report_flag(&report, "input_queued", true);
    assert_report_flag(&report, "denial_queued", true);
    assert_init_checks(&report, true, true, true);
    assert!(report.observations.proof_before_reply);
    assert_cleanup(&report, &shared);
    let bytes = shared.input_bytes.lock().expect("captured input");
    let frames: Vec<Value> = String::from_utf8_lossy(&bytes)
        .lines()
        .map(|line| serde_json::from_str(line).expect("captured JSON"))
        .collect();
    assert_eq!(frames.len(), 3);
    assert_deny(&frames[2]);
    let public = serde_json::to_string(&report).expect("redacted report");
    let debug = format!("{report:?}");
    for private in [
        "private diagnostic prompt",
        "private_candidate",
        "private body",
        "private_request",
        "private_message",
        "private_tool",
        "private_diagnostic",
        "00000000-0000-0000-0000-000000000001",
        "00000000-0000-0000-0000-000000000002",
    ] {
        assert!(!public.contains(private));
        assert!(!debug.contains(private));
    }
}

#[tokio::test]
async fn early_request_and_missing_echo_are_denied_without_retroactive_proof() {
    for early in [false, true] {
        let (transport, mut peer, shared, _gate) = setup(None, false);
        let (_owner, cancellation) = watch::channel(false);
        let (report, ()) = timeout(WAIT, async {
            tokio::join!(drive_probe(transport, context(), cancellation), async {
                initialized(&mut peer).await;
                send(&mut peer.output, init()).await;
                if !early {
                    send(&mut peer.output, tool(false)).await;
                }
                send(&mut peer.output, request()).await;
                assert_deny(&frame(&mut peer.input).await);
                if early {
                    send(&mut peer.output, tool(true)).await;
                }
                send(&mut peer.output, terminal()).await;
            })
        })
        .await
        .expect("bounded observation");
        assert_eq!(report.stop, DiagnosticStop::ObservedTerminal);
        assert!(report.denial_written);
        assert!(!report.observations.proof_before_reply);
        assert!(report.observations.matching_request_seen);
        assert_cleanup(&report, &shared);
    }
}

#[tokio::test]
async fn canonical_sdk_manual_mode_keeps_one_input_and_one_fixed_denial() {
    let (transport, mut peer, shared, _gate) = setup(None, false);
    let (_owner, cancellation) = watch::channel(false);
    let (report, ()) = timeout(WAIT, async {
        tokio::join!(drive_probe(transport, context(), cancellation), async {
            initialized(&mut peer).await;
            let mut initialization = init();
            initialization["permissionMode"] = json!("default");
            send(&mut peer.output, initialization).await;
            send(&mut peer.output, tool(true)).await;
            send(&mut peer.output, request()).await;
            assert_deny(&frame(&mut peer.input).await);
            send(&mut peer.output, terminal()).await;
        })
    })
    .await
    .expect("bounded canonical-mode diagnostic");
    assert_eq!(report.stop, DiagnosticStop::ObservedTerminal);
    assert_eq!(report_json(&report)["report_version"], 3);
    assert_init_checks(&report, true, true, true);
    assert!(report.initialize_written && report.input_written && report.denial_written);
    assert!(report.observations.proof_before_reply);
    assert_cleanup(&report, &shared);
    let input = shared.input_bytes.lock().expect("owned input");
    let frames: Vec<Value> = String::from_utf8_lossy(&input)
        .lines()
        .map(|line| serde_json::from_str(line).expect("input frame"))
        .collect();
    assert_eq!(frames.len(), 3, "one handshake, one input and one denial");
    assert_deny(&frames[2]);
}

#[tokio::test]
async fn work_in_the_initialize_response_batch_prevents_user_submission() {
    let (transport, mut peer, shared, _gate) = setup(None, false);
    let (_owner, cancellation) = watch::channel(false);
    let (report, ()) = timeout(WAIT, async {
        tokio::join!(drive_probe(transport, context(), cancellation), async {
            let initialize = frame(&mut peer.input).await;
            let batch = format!("{}\n{}\n", response(&initialize), tool(true));
            peer.output
                .write_all(batch.as_bytes())
                .await
                .expect("early output");
            let mut following = String::new();
            assert_eq!(
                peer.input
                    .read_line(&mut following)
                    .await
                    .expect("closed input"),
                0
            );
        })
    })
    .await
    .expect("pre-input work stops");
    assert_eq!(report.stop, DiagnosticStop::PreInputWork);
    assert_report_flag(&report, "initialize_accepted", true);
    assert_report_flag(&report, "input_queued", false);
    assert_report_flag(&report, "denial_queued", false);
    assert_no_init_checks(&report);
    assert!(!report.input_written);
    let bytes = shared.input_bytes.lock().expect("captured input");
    assert_eq!(String::from_utf8_lossy(&bytes).lines().count(), 1);
    assert_cleanup(&report, &shared);
}

#[tokio::test]
async fn invalid_profile_foreign_session_nested_identity_and_foreign_echo_are_rejected() {
    for scenario in 0..4 {
        let (transport, mut peer, shared, _gate) = setup(None, false);
        let (_owner, cancellation) = watch::channel(false);
        let (report, ()) = timeout(WAIT, async {
            tokio::join!(drive_probe(transport, context(), cancellation), async {
                initialized(&mut peer).await;
                let mut value = if scenario == 0 { init() } else { tool(true) };
                match scenario {
                    0 => value["permissionMode"] = json!("dontAsk"),
                    1 => value["session_id"] = json!(Uuid::from_u128(42)),
                    2 => value["parent_tool_use_id"] = json!("private_parent"),
                    _ => value["user_message_uuid"] = json!(Uuid::from_u128(42)),
                }
                send(&mut peer.output, value).await;
            })
        })
        .await
        .expect("invalid evidence stops");
        assert_eq!(report.stop, DiagnosticStop::ProtocolRejected);
        assert!(!report.observations.proof_before_reply);
        assert_cleanup(&report, &shared);
    }
}

#[tokio::test]
async fn handshake_requires_the_exact_single_response_and_a_deadline() {
    for scenario in 0..3 {
        let (transport, mut peer, shared, _gate) = setup(None, false);
        let (_owner, cancellation) = watch::channel(false);
        let mut configured = context();
        configured.limits.initialize = Duration::from_millis(40);
        let (report, ()) = timeout(WAIT, async {
            tokio::join!(drive_probe(transport, configured, cancellation), async {
                let initialize = frame(&mut peer.input).await;
                if scenario == 0 {
                    let mut wrong = response(&initialize);
                    wrong["response"]["request_id"] = json!("foreign_request");
                    send(&mut peer.output, wrong).await;
                } else if scenario == 1 {
                    let response = response(&initialize);
                    let batch = format!("{response}\n{response}\n");
                    peer.output
                        .write_all(batch.as_bytes())
                        .await
                        .expect("duplicate handshake");
                }
                let mut following = String::new();
                assert_eq!(
                    peer.input
                        .read_line(&mut following)
                        .await
                        .expect("input closed"),
                    0
                );
            })
        })
        .await
        .expect("bounded handshake");
        assert_eq!(
            report.stop,
            if scenario == 2 {
                DiagnosticStop::InitializeDeadline
            } else {
                DiagnosticStop::ProtocolRejected
            }
        );
        assert_report_flag(&report, "initialize_accepted", scenario == 1);
        assert_report_flag(&report, "input_queued", false);
        assert_report_flag(&report, "denial_queued", false);
        assert_no_init_checks(&report);
        assert!(!report.input_written);
        assert_cleanup(&report, &shared);
    }
}

#[tokio::test]
async fn held_input_and_denial_flushes_keep_output_and_cancellation_live() {
    for held_flush in [2, 3] {
        let (transport, mut peer, shared, gate) = setup(Some(held_flush), false);
        let (owner, cancellation) = watch::channel(false);
        let read_shared = shared.clone();
        let (report, ()) = timeout(WAIT, async {
            tokio::join!(drive_probe(transport, context(), cancellation), async {
                initialized(&mut peer).await;
                let initial = read_shared.bytes_read.load(Ordering::SeqCst);
                let mut bytes = 0;
                bytes += send(&mut peer.output, init()).await;
                bytes += send(&mut peer.output, tool(true)).await;
                bytes += send(&mut peer.output, request()).await;
                if held_flush == 3 {
                    assert_deny(&frame(&mut peer.input).await);
                }
                bytes += send(&mut peer.output, terminal()).await;
                wait_read(&read_shared, initial + bytes).await;
                owner.send(true).expect("cancel");
            })
        })
        .await
        .expect("cancellation survives held flush");
        assert_eq!(report.stop, DiagnosticStop::Cancelled);
        assert!(report.observations.complete_tool_seen);
        assert!(report.observations.denied);
        assert_report_flag(&report, "initialize_accepted", true);
        assert_report_flag(&report, "input_queued", true);
        assert_report_flag(&report, "denial_queued", true);
        assert_init_checks(&report, true, true, true);
        assert!(!report.denial_written);
        if held_flush == 2 {
            assert!(!report.input_written);
        }
        assert_cleanup(&report, &shared);
        gate.release();
    }
}

#[tokio::test]
async fn a_terminal_waits_for_the_denial_flush_instead_of_completing_on_pipe_bytes() {
    let (transport, mut peer, shared, gate) = setup(Some(3), false);
    let (_owner, cancellation) = watch::channel(false);
    let read_shared = shared.clone();
    let (report, ()) = timeout(WAIT, async {
        tokio::join!(drive_probe(transport, context(), cancellation), async {
            initialized(&mut peer).await;
            let initial = read_shared.bytes_read.load(Ordering::SeqCst);
            let mut bytes = send(&mut peer.output, init()).await;
            bytes += send(&mut peer.output, tool(true)).await;
            bytes += send(&mut peer.output, request()).await;
            assert_deny(&frame(&mut peer.input).await);
            bytes += send(&mut peer.output, terminal()).await;
            wait_read(&read_shared, initial + bytes).await;
            assert!(!read_shared.finished.load(Ordering::SeqCst));
            gate.release();
        })
    })
    .await
    .expect("terminal settles after flush");
    assert_eq!(report.stop, DiagnosticStop::ObservedTerminal);
    assert!(report.input_written && report.denial_written);
    assert_cleanup(&report, &shared);
}

#[tokio::test]
async fn additional_challenges_and_invalid_json_close_without_a_second_response() {
    for duplicate in [false, true] {
        let (transport, mut peer, shared, _gate) = setup(None, false);
        let (_owner, cancellation) = watch::channel(false);
        let (report, ()) = timeout(WAIT, async {
            tokio::join!(drive_probe(transport, context(), cancellation), async {
                initialized(&mut peer).await;
                if duplicate {
                    send(&mut peer.output, init()).await;
                    send(&mut peer.output, request()).await;
                    assert_deny(&frame(&mut peer.input).await);
                    send(&mut peer.output, request()).await;
                } else {
                    peer.output
                        .write_all(b"{invalid JSON}\n")
                        .await
                        .expect("invalid output");
                }
            })
        })
        .await
        .expect("invalid records stop");
        assert_eq!(report.stop, DiagnosticStop::ProtocolRejected);
        let bytes = shared.input_bytes.lock().expect("captured input");
        let denials = String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| line.contains("Diagnostic denied"))
            .count();
        assert_eq!(denials, if duplicate { 1 } else { 0 });
        assert_cleanup(&report, &shared);
    }
}

#[tokio::test]
async fn eof_root_exit_turn_deadline_and_output_limit_retain_distinct_stops() {
    for scenario in 0..4 {
        let (transport, mut peer, shared, _gate) = setup(None, false);
        let (_owner, cancellation) = watch::channel(false);
        let mut configured = context();
        configured.limits.turn = Duration::from_millis(50);
        if scenario == 3 {
            configured.limits.output_bytes = 32;
        }
        let control = shared.clone();
        let (report, ()) = timeout(WAIT, async {
            tokio::join!(drive_probe(transport, configured, cancellation), async {
                if scenario == 2 {
                    initialized(&mut peer).await;
                } else {
                    let _ = frame(&mut peer.input).await;
                }
                match scenario {
                    0 => {
                        peer.output.shutdown().await.expect("EOF");
                    }
                    1 => {
                        control.root_exited.store(true, Ordering::SeqCst);
                    }
                    2 => {}
                    _ => {
                        peer.output
                            .write_all(&[b'x'; 64])
                            .await
                            .expect("oversized output");
                    }
                }
                let mut following = String::new();
                assert_eq!(
                    peer.input
                        .read_line(&mut following)
                        .await
                        .expect("closed stdin"),
                    0
                );
            })
        })
        .await
        .expect("bounded stop");
        let expected = [
            DiagnosticStop::EofWithoutTerminal,
            DiagnosticStop::RootExited,
            DiagnosticStop::TurnDeadline,
            DiagnosticStop::OutputLimit,
        ][scenario];
        assert_eq!(report.stop, expected);
        if scenario == 3 {
            assert_eq!(report.stdout_bytes, 32);
        }
        assert_cleanup(&report, &shared);
    }
}

#[tokio::test]
async fn invalid_setup_failed_input_and_failed_output_always_join_before_finish() {
    for scenario in 0..3 {
        let (transport, _peer, shared, _gate) = setup(None, scenario == 1);
        let (_owner, cancellation) = watch::channel(false);
        let mut configured = context();
        if scenario == 0 {
            configured.prompt.clear();
        }
        if scenario == 2 {
            shared.read_failed.store(true, Ordering::SeqCst);
        }
        let report = timeout(WAIT, drive_probe(transport, configured, cancellation))
            .await
            .expect("bounded invalid setup");
        assert_eq!(
            report.stop,
            [
                DiagnosticStop::InvalidConfiguration,
                DiagnosticStop::InputUnknown,
                DiagnosticStop::ReadFailed
            ][scenario]
        );
        assert_cleanup(&report, &shared);
    }
}

#[tokio::test]
async fn a_matching_handshake_cannot_bypass_the_initialization_flush_receipt() {
    let (transport, mut peer, shared, _gate) = setup(Some(1), false);
    let (owner, cancellation) = watch::channel(false);
    let read_shared = shared.clone();
    let (report, ()) = timeout(WAIT, async {
        tokio::join!(drive_probe(transport, context(), cancellation), async {
            let initialize = frame(&mut peer.input).await;
            let bytes = send(&mut peer.output, response(&initialize)).await
                + send(&mut peer.output, init()).await;
            wait_read(&read_shared, bytes).await;
            owner.send(true).expect("cancel held initialization");
            let mut following = String::new();
            assert_eq!(
                peer.input
                    .read_line(&mut following)
                    .await
                    .expect("closed input"),
                0
            );
        })
    })
    .await
    .expect("initialization flush remains owned");
    assert_eq!(report.stop, DiagnosticStop::Cancelled);
    assert!(!report.initialize_written);
    assert_report_flag(&report, "initialize_accepted", true);
    assert_report_flag(&report, "input_queued", false);
    assert_report_flag(&report, "denial_queued", false);
    assert_init_checks(&report, true, true, true);
    assert!(!report.input_written);
    assert!(report.observations.root_init_seen);
    assert_eq!(
        String::from_utf8_lossy(&shared.input_bytes.lock().expect("input"))
            .lines()
            .count(),
        1
    );
    assert_cleanup(&report, &shared);
}

#[tokio::test]
async fn report_v2_identifies_each_init_mismatch_without_admitting_the_profile() {
    let mut cases = Vec::new();
    let mut permission = init();
    permission["permissionMode"] = json!("PRIVATE_PERMISSION_MODE");
    cases.push((permission, [false, true, true]));
    let mut directory = init();
    directory["cwd"] = json!("PRIVATE_OBSERVED_WORKING_DIRECTORY");
    cases.push((directory, [true, false, true]));
    for tools in [
        json!([]),
        json!(["Read"]),
        json!(["Write", "Read"]),
        json!(["Write", "Write"]),
        Value::Null,
        json!("PRIVATE_TOOLS_SHAPE"),
        json!({"PRIVATE_TOOL_KEY":"PRIVATE_TOOL_VALUE"}),
        json!(42),
    ] {
        let mut value = init();
        value["tools"] = tools;
        cases.push((value, [true, true, false]));
    }
    let mut missing = init();
    missing
        .as_object_mut()
        .expect("init object")
        .remove("tools");
    cases.push((missing, [true, true, false]));
    let mut all = init();
    all["permissionMode"] = json!("PRIVATE_PERMISSION_MODE");
    all["cwd"] = json!("PRIVATE_OBSERVED_WORKING_DIRECTORY");
    all["tools"] = json!(["PRIVATE_TOOL_VALUE"]);
    cases.push((all, [false, false, false]));
    for (value, expected) in cases {
        let (transport, mut peer, shared, _gate) = setup(None, false);
        let (_owner, cancellation) = watch::channel(false);
        let (report, ()) = timeout(WAIT, async {
            tokio::join!(drive_probe(transport, context(), cancellation), async {
                initialized(&mut peer).await;
                send(&mut peer.output, value).await;
            })
        })
        .await
        .expect("strict init mismatch stops");
        assert_eq!(report.stop, DiagnosticStop::ProtocolRejected);
        assert_eq!(
            report.observations.error,
            Some(super::super::ProbeError::InitMismatch)
        );
        assert!(!report.observations.root_init_seen);
        assert!(!report.observations.proof_before_reply);
        assert!(report.initialize_written);
        assert_report_flag(&report, "initialize_accepted", true);
        assert_report_flag(&report, "input_queued", true);
        assert_report_flag(&report, "denial_queued", false);
        assert!(!report.denial_written);
        assert_init_checks(&report, expected[0], expected[1], expected[2]);
        let public = report_json(&report).to_string();
        let debug = format!("{report:?}");
        for private in [
            "PRIVATE_PERMISSION_MODE",
            "PRIVATE_OBSERVED_WORKING_DIRECTORY",
            "PRIVATE_TOOLS_SHAPE",
            "PRIVATE_TOOL_KEY",
            "PRIVATE_TOOL_VALUE",
            "private_diagnostic",
            "private diagnostic prompt",
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000002",
        ] {
            assert!(!public.contains(private));
            assert!(!debug.contains(private));
        }
        assert_cleanup(&report, &shared);
    }
}

#[tokio::test]
async fn report_v2_does_not_publish_init_checks_for_foreign_or_nested_identity() {
    for (key, value, expected) in [
        (
            "session_id",
            json!(Uuid::from_u128(42)),
            super::super::ProbeError::ForeignSession,
        ),
        (
            "parent_tool_use_id",
            json!("PRIVATE_NESTED_TOOL"),
            super::super::ProbeError::NestedIdentity,
        ),
        (
            "agent_id",
            json!("PRIVATE_AGENT"),
            super::super::ProbeError::NestedIdentity,
        ),
        (
            "parent_agent_id",
            json!("PRIVATE_PARENT_AGENT"),
            super::super::ProbeError::NestedIdentity,
        ),
    ] {
        let (transport, mut peer, shared, _gate) = setup(None, false);
        let (_owner, cancellation) = watch::channel(false);
        let (report, ()) = timeout(WAIT, async {
            tokio::join!(drive_probe(transport, context(), cancellation), async {
                initialized(&mut peer).await;
                let mut initialization = init();
                initialization[key] = value;
                // Identity refusal precedes all profile-dimension observations.
                initialization["permissionMode"] = json!("PRIVATE_UNQUALIFIED_MODE");
                initialization["cwd"] = json!("PRIVATE_UNQUALIFIED_DIRECTORY");
                initialization["tools"] = json!({"PRIVATE_UNQUALIFIED_TOOL":"unknown"});
                send(&mut peer.output, initialization).await;
            })
        })
        .await
        .expect("identity refusal stops");
        assert_eq!(report.stop, DiagnosticStop::ProtocolRejected);
        assert_eq!(report.observations.error, Some(expected));
        assert!(!report.observations.root_init_seen);
        assert_no_init_checks(&report);
        assert_report_flag(&report, "initialize_accepted", true);
        assert_report_flag(&report, "input_queued", true);
        assert_report_flag(&report, "denial_queued", false);
        let public = report_json(&report).to_string();
        let debug = format!("{report:?}");
        for private in [
            "PRIVATE_NESTED_TOOL",
            "PRIVATE_AGENT",
            "PRIVATE_PARENT_AGENT",
            "PRIVATE_UNQUALIFIED_MODE",
            "PRIVATE_UNQUALIFIED_DIRECTORY",
            "PRIVATE_UNQUALIFIED_TOOL",
            "00000000-0000-0000-0000-00000000002a",
        ] {
            assert!(!public.contains(private));
            assert!(!debug.contains(private));
        }
        assert_cleanup(&report, &shared);
    }
}

#[tokio::test]
async fn report_v2_duplicate_init_cannot_replace_the_first_qualified_checks() {
    let (transport, mut peer, shared, _gate) = setup(None, false);
    let (_owner, cancellation) = watch::channel(false);
    let (report, ()) = timeout(WAIT, async {
        tokio::join!(drive_probe(transport, context(), cancellation), async {
            initialized(&mut peer).await;
            let first = init();
            let mut duplicate = init();
            duplicate["uuid"] = json!("private_duplicate_init");
            duplicate["permissionMode"] = json!("PRIVATE_UNCHECKED_MODE");
            duplicate["cwd"] = json!("PRIVATE_UNCHECKED_DIRECTORY");
            duplicate["tools"] = json!({"PRIVATE_UNCHECKED_TOOL":"unknown"});
            let batch = format!("{first}\n{duplicate}\n");
            peer.output
                .write_all(batch.as_bytes())
                .await
                .expect("duplicate init");
        })
    })
    .await
    .expect("duplicate init stops");
    assert_eq!(report.stop, DiagnosticStop::ProtocolRejected);
    assert_eq!(
        report.observations.error,
        Some(super::super::ProbeError::ReplayIdentity)
    );
    assert!(report.observations.root_init_seen);
    assert_init_checks(&report, true, true, true);
    assert_report_flag(&report, "initialize_accepted", true);
    assert_report_flag(&report, "input_queued", true);
    assert_report_flag(&report, "denial_queued", false);
    let public = report_json(&report).to_string();
    let debug = format!("{report:?}");
    for private in [
        "private_duplicate_init",
        "PRIVATE_UNCHECKED_MODE",
        "PRIVATE_UNCHECKED_DIRECTORY",
        "PRIVATE_UNCHECKED_TOOL",
    ] {
        assert!(!public.contains(private));
        assert!(!debug.contains(private));
    }
    assert_cleanup(&report, &shared);
}
