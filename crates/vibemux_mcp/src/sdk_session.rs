//! ADR 075 Scope A live session driver (slice 3).
//!
//! The driver owns everything the SDK does not: raw bounded framing, JSON
//! parse and `-32700`/`-32600` synthesis, the initialize handshake (the
//! grant stays a first-party boolean), the pre-initialize gate, the
//! single-flight admission answers, cancelled-id reply suppression, one
//! serialized writer, and the finalizer that joins every task.
//! Feature-gated (`rmcp`); the hand-written session remains the default.

use std::future::Future;

use rmcp::{
    model::ClientConfig,
    service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage, serve_directly},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncWrite},
    sync::mpsc,
    task::JoinHandle,
};

use crate::{
    config::{MAX_INPUT_BYTES, SHUTDOWN_DEADLINE},
    framing::{LineReader, write_json},
    protocol::{error, metadata_only, result, valid_envelope},
    sdk_driver::{
        Admission, MetadataExecutor, SdkHost, WireState, initialize_reply_value, parse_initialize,
        wire_id,
    },
    sdk_seam::{ChannelPeer, SeamError, channel_transport, write_typed},
};

/// Capacity of the driver-to-writer frame queue; bounded on purpose.
pub const WRITER_QUEUE: usize = 32;

enum Frame {
    Value(Value),
    Typed(Box<TxJsonRpcMessage<RoleServer>>),
}

type FrameSender = mpsc::Sender<Frame>;
type InboundSender = mpsc::Sender<RxJsonRpcMessage<RoleServer>>;

/// Runs one SDK-backed MCP session over the supplied streams.
///
/// Returns the first fatal code: a framing fault from the reader, a writer
/// fault, or the finalizer's code. EOF is a clean end of the session.
pub async fn run_session<R, W, E, F>(
    reader: R,
    writer: W,
    host: SdkHost<E>,
    host_grant: bool,
    finalize: F,
) -> Result<(), &'static str>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
    E: MetadataExecutor,
    F: Future<Output = Result<(), &'static str>>,
{
    let state = host.state();
    let admission = host.admission();
    let (frames, frame_rx) = mpsc::channel(WRITER_QUEUE);
    let writer_task = spawn_writer(writer, frame_rx);
    let (transport, peer) = channel_transport();
    let mut service =
        serve_directly::<RoleServer, _, _, _, _>(host, transport, None::<ClientConfig>);
    let inbound = peer.inbound.clone();
    let outbound_task = spawn_outbound(peer, frames.clone(), admission.clone());

    let loop_result = drive(reader, &frames, &inbound, &state, &admission, host_grant).await;

    // Close the inbound side first so the service observes EOF, bound its
    // cleanup, then join every task before the finalizer runs.
    drop(inbound);
    let _ = service.close_with_timeout(SHUTDOWN_DEADLINE).await;
    drop(service);
    let _ = outbound_task.await;
    drop(frames);
    let writer_result = match writer_task.await {
        Ok(result) => result,
        Err(_) => Err("writer_task_failed"),
    };
    let finalize_result = finalize.await;
    loop_result.and(writer_result).and(finalize_result)
}

async fn drive<R: AsyncBufRead + Unpin>(
    reader: R,
    frames: &FrameSender,
    inbound: &InboundSender,
    state: &std::sync::Arc<WireState>,
    admission: &Admission,
    host_grant: bool,
) -> Result<(), &'static str> {
    let mut reader = LineReader::new(reader, MAX_INPUT_BYTES);
    let mut initialization_sent = false;
    loop {
        let bytes = reader.next().await?;
        let Some(bytes) = bytes else {
            return Ok(());
        };
        let message = match serde_json::from_slice::<Value>(&bytes) {
            Ok(message) => message,
            Err(_) => {
                send_value(frames, error(Value::Null, -32700, "parse_error")).await?;
                continue;
            }
        };
        let Some(object) = message.as_object() else {
            send_value(frames, error(Value::Null, -32600, "invalid_request")).await?;
            continue;
        };
        if !valid_envelope(object) {
            send_value(frames, error(Value::Null, -32600, "invalid_request")).await?;
            continue;
        }
        let method = object["method"].as_str().unwrap_or_default();
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(raw_id) = object.get("id").cloned() else {
            handle_notification(
                &message,
                method,
                &params,
                &mut initialization_sent,
                state,
                admission,
                inbound,
            )
            .await;
            continue;
        };
        // Single-flight parity: while one call is active, any request other
        // than ping is busy; the active id is a duplicate. Unlike the
        // hand-written loop's null-id quirk, the request id is echoed.
        if let Some(active) = admission.active() {
            let key = json_id_key(&raw_id);
            let reply = if key.as_deref() == Some(active.as_str()) {
                error(raw_id.clone(), -32600, "duplicate_request_id")
            } else if method == "ping" && metadata_only(&params) {
                result(raw_id.clone(), json!({}))
            } else {
                error(raw_id.clone(), -32000, "request_busy")
            };
            send_value(frames, reply).await?;
            continue;
        }
        if method == "initialize" {
            if initialization_sent {
                send_value(frames, error(raw_id.clone(), -32600, "already_initialized")).await?;
                continue;
            }
            match parse_initialize(&params, host_grant) {
                Ok(granted) => {
                    state.set_grant(granted);
                    initialization_sent = true;
                    send_value(frames, result(raw_id.clone(), initialize_reply_value())).await?;
                }
                Err(_) => {
                    send_value(frames, error(raw_id.clone(), -32602, "invalid_arguments")).await?;
                }
            }
            continue;
        }
        if method == "ping" {
            let reply = if metadata_only(&params) {
                result(raw_id.clone(), json!({}))
            } else {
                error(raw_id.clone(), -32602, "invalid_arguments")
            };
            send_value(frames, reply).await?;
            continue;
        }
        if !state.initialized() {
            send_value(frames, error(raw_id.clone(), -32002, "not_initialized")).await?;
            continue;
        }
        match serde_json::from_value::<RxJsonRpcMessage<RoleServer>>(message.clone()) {
            Ok(typed) => {
                inbound.send(typed).await.map_err(|_| "session_closed")?;
            }
            Err(_) => {
                let reply = if matches!(method, "tools/list" | "tools/call") {
                    error(raw_id.clone(), -32602, "invalid_arguments")
                } else {
                    error(raw_id.clone(), -32601, "method_not_found")
                };
                send_value(frames, reply).await?;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_notification(
    message: &Value,
    method: &str,
    params: &Value,
    initialization_sent: &mut bool,
    state: &std::sync::Arc<WireState>,
    admission: &Admission,
    inbound: &InboundSender,
) {
    match method {
        "notifications/initialized" if *initialization_sent && metadata_only(params) => {
            // First-party gate, independent of the SDK callback timing.
            state.mark_initialized();
            if let Ok(typed) =
                serde_json::from_value::<RxJsonRpcMessage<RoleServer>>(message.clone())
            {
                let _ = inbound.send(typed).await;
            }
        }
        "notifications/cancelled" => {
            if let Some(key) = params.get("requestId").and_then(json_id_key) {
                admission.cancel(&key);
            }
            if let Ok(typed) =
                serde_json::from_value::<RxJsonRpcMessage<RoleServer>>(message.clone())
            {
                let _ = inbound.send(typed).await;
            }
        }
        _ => {}
    }
}

fn json_id_key(value: &Value) -> Option<String> {
    if let Some(number) = value.as_i64() {
        return Some(format!("n:{number}"));
    }
    value
        .as_str()
        .filter(|text| text.len() <= 128)
        .map(|text| format!("s:{text}"))
}

async fn send_value(frames: &FrameSender, value: Value) -> Result<(), &'static str> {
    frames
        .send(Frame::Value(value))
        .await
        .map_err(|_| "writer_closed")
}

fn spawn_writer<W: AsyncWrite + Unpin + Send + 'static>(
    mut writer: W,
    mut frames: mpsc::Receiver<Frame>,
) -> JoinHandle<Result<(), &'static str>> {
    tokio::spawn(async move {
        while let Some(frame) = frames.recv().await {
            match frame {
                Frame::Value(value) => write_json(&mut writer, &value).await?,
                Frame::Typed(message) => {
                    write_typed(&mut writer, &message)
                        .await
                        .map_err(|error| match error {
                            SeamError::OutputTooLarge => "output_too_large",
                            SeamError::OutputInvalid => "output_invalid",
                            SeamError::OutputFailed => "output_unavailable",
                            SeamError::TransportClosed => "writer_closed",
                        })?
                }
            }
        }
        Ok(())
    })
}

fn spawn_outbound(
    mut peer: ChannelPeer,
    frames: FrameSender,
    admission: Admission,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(message) = peer.outbound.recv().await {
            if let Some(key) = message_id(&message)
                && admission.should_suppress(&key)
            {
                admission.clear_cancelled(&key);
                continue;
            }
            if frames.send(Frame::Typed(Box::new(message))).await.is_err() {
                return;
            }
        }
    })
}

fn message_id(message: &TxJsonRpcMessage<RoleServer>) -> Option<String> {
    match message {
        TxJsonRpcMessage::<RoleServer>::Response(response) => Some(wire_id(&response.id)),
        TxJsonRpcMessage::<RoleServer>::Error(error) => error.id.as_ref().map(wire_id),
        TxJsonRpcMessage::<RoleServer>::Notification(_)
        | TxJsonRpcMessage::<RoleServer>::Request(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use serde_json::Value;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, duplex},
        time::{Duration, timeout},
    };

    use super::*;
    use crate::{sdk_driver::WireState, tools::ReadOperation};

    struct StubExecutor {
        calls: Arc<AtomicUsize>,
        delay_ms: u64,
    }

    impl MetadataExecutor for StubExecutor {
        fn read(
            &self,
            operation: ReadOperation,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Value, &'static str>> + Send + '_>,
        > {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if self.delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
                }
                match operation {
                    ReadOperation::Health => Ok(serde_json::json!({"healthy": true})),
                    _ => Err("reader_failed"),
                }
            })
        }
    }

    struct Harness {
        stdin: DuplexStream,
        stdout: BufReader<DuplexStream>,
        calls: Arc<AtomicUsize>,
        finalized: Arc<AtomicBool>,
        task: tokio::task::JoinHandle<Result<(), &'static str>>,
    }

    fn start(delay_ms: u64) -> Harness {
        let (client_writer, server_reader) = duplex(1 << 20);
        let (server_writer, client_reader) = duplex(1 << 20);
        let calls = Arc::new(AtomicUsize::new(0));
        let finalized = Arc::new(AtomicBool::new(false));
        let host = SdkHost::new(
            Arc::new(WireState::new()),
            Admission::new(),
            StubExecutor {
                calls: Arc::clone(&calls),
                delay_ms,
            },
        );
        let done = Arc::clone(&finalized);
        let task = tokio::spawn(async move {
            run_session(
                BufReader::new(server_reader),
                server_writer,
                host,
                true,
                async move {
                    done.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        });
        Harness {
            stdin: client_writer,
            stdout: BufReader::new(client_reader),
            calls,
            finalized,
            task,
        }
    }

    impl Harness {
        async fn send(&mut self, value: Value) {
            let mut line = serde_json::to_vec(&value).expect("encode");
            line.push(b'\n');
            self.stdin.write_all(&line).await.expect("write line");
        }

        async fn read(&mut self) -> Value {
            let mut line = String::new();
            timeout(Duration::from_secs(3), self.stdout.read_line(&mut line))
                .await
                .expect("frame within deadline")
                .expect("read frame");
            assert!(!line.is_empty(), "expected another frame");
            serde_json::from_str(&line).expect("JSON frame")
        }

        async fn no_frame_within(&mut self, within: Duration) -> bool {
            let mut line = String::new();
            match timeout(within, self.stdout.read_line(&mut line)).await {
                Err(_elapsed) => true,
                Ok(Ok(0)) => true,
                Ok(Ok(_)) => panic!("unexpected frame: {line}"),
                Ok(Err(_)) => true,
            }
        }

        async fn finish(mut self) -> Result<(), &'static str> {
            drop(self.stdin);
            let result = timeout(Duration::from_secs(5), &mut self.task)
                .await
                .expect("session ends")
                .expect("task joins");
            assert!(self.finalized.load(Ordering::SeqCst), "finalizer ran");
            result
        }
    }

    fn initialize(id: u64) -> Value {
        serde_json::json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{
            "protocolVersion":"2025-06-18",
            "capabilities":{"experimental":{"vibemux_metadata_read":true}},
            "clientInfo":{"name":"fixture","version":"1.0"}
        }})
    }

    #[tokio::test]
    async fn the_sdk_session_serves_the_hand_written_contract() {
        let mut harness = start(0);
        // Pre-initialize request is refused.
        harness
            .send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
            .await;
        let refused = harness.read().await;
        assert_eq!(refused["error"]["data"]["code"], "not_initialized");

        harness.send(initialize(2)).await;
        let initialized = harness.read().await;
        assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(initialized["result"]["serverInfo"]["name"], "vibemux_mcp");

        harness
            .send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
            .await;
        harness
            .send(serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}))
            .await;
        let listed = harness.read().await;
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            names,
            [
                "host_status",
                "health",
                "frontend_tasks",
                "frontend_task",
                "harness_dispatch_status"
            ]
        );

        harness
            .send(
                serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
                "params":{"name":"health","arguments":{}}}),
            )
            .await;
        let called = harness.read().await;
        assert_eq!(called["id"], 4);
        assert_eq!(called["result"]["isError"], false);
        assert_eq!(called["result"]["structuredContent"]["healthy"], true);
        assert_eq!(harness.calls.load(Ordering::SeqCst), 1);

        // Parse errors keep the session alive.
        harness
            .stdin
            .write_all(b"{not json\n")
            .await
            .expect("bad line");
        let parse_error = harness.read().await;
        assert_eq!(parse_error["error"]["data"]["code"], "parse_error");
        assert_eq!(parse_error["id"], Value::Null);

        // A late tools/list after a tool-level error result still works.
        harness
            .send(
                serde_json::json!({"jsonrpc":"2.0","id":5,"method":"tools/call",
                "params":{"name":"frontend_tasks","arguments":{}}}),
            )
            .await;
        let failed = harness.read().await;
        assert_eq!(failed["result"]["isError"], true);

        harness.finish().await.expect("clean session");
    }

    #[tokio::test]
    async fn a_cancelled_call_never_replies_and_the_session_continues() {
        let mut harness = start(300);
        harness.send(initialize(1)).await;
        let _ = harness.read().await;
        harness
            .send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
            .await;
        harness
            .send(
                serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
                "params":{"name":"health","arguments":{}}}),
            )
            .await;
        harness
            .send(
                serde_json::json!({"jsonrpc":"2.0","method":"notifications/cancelled",
                "params":{"requestId":2}}),
            )
            .await;
        assert!(
            harness.no_frame_within(Duration::from_millis(500)).await,
            "a cancelled id must receive no reply"
        );
        harness
            .send(
                serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"host_status","arguments":{}}}),
            )
            .await;
        let status = harness.read().await;
        assert_eq!(status["id"], 3);
        assert_eq!(status["result"]["isError"], false);
        harness.finish().await.expect("clean session");
    }

    #[tokio::test]
    async fn duplicate_initialize_and_invalid_initialize_are_refused() {
        let mut harness = start(0);
        harness
            .send(
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-06-18","capabilities":{"experimental":[]},
                "clientInfo":{"name":"fixture","version":"1.0"}}}),
            )
            .await;
        let invalid = harness.read().await;
        assert_eq!(invalid["error"]["data"]["code"], "invalid_arguments");
        harness.send(initialize(2)).await;
        let _ = harness.read().await;
        harness.send(initialize(3)).await;
        let duplicate = harness.read().await;
        assert_eq!(duplicate["error"]["data"]["code"], "already_initialized");
        harness.finish().await.expect("clean session");
    }
}
