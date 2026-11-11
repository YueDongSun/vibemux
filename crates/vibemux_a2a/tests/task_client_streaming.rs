use axum::{
    Json, Router,
    body::{Body, Bytes},
    response::IntoResponse,
    routing::{get, post},
};
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc};
use tokio::{sync::oneshot, task::JoinHandle, time::timeout};
use vibemux_a2a::*;

const TASK_ID: &str = "stream_task";
const CONTEXT_ID: &str = "stream_context";
const EVENT_COUNT: usize = 30;

struct SsePeer {
    url: String,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}
impl SsePeer {
    async fn start(
        binding: TaskBinding,
        chunks: impl Fn(Option<Value>) -> Vec<Bytes> + Send + Sync + 'static,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let protocol = match binding {
            TaskBinding::HttpJson => "HTTP+JSON",
            TaskBinding::JsonRpc => "JSONRPC",
        };
        let card = json!({
            "name": "sse_fixture", "description": "bounded streaming fixture", "version": "1",
            "supportedInterfaces": [{
                "url": url, "protocolBinding": protocol, "protocolVersion": "1.0"
            }],
            "capabilities": {"streaming": true},
            "defaultInputModes": ["application/json"],
            "defaultOutputModes": ["application/json"], "skills": []
        });
        let chunks = Arc::new(chunks);
        let http_chunks = chunks.clone();
        let app = Router::new()
            .route(
                "/.well-known/agent-card.json",
                get(move || {
                    let card = card.clone();
                    async move { Json(card) }
                }),
            )
            .route(
                &format!("/tasks/{TASK_ID}:subscribe"),
                get(move || {
                    let chunks = http_chunks(None);
                    async move { sse_response(chunks) }
                }),
            )
            .route(
                "/",
                post(move |Json(request): Json<Value>| {
                    let chunks = chunks(Some(request["id"].clone()));
                    async move { sse_response(chunks) }
                }),
            );
        let (stop, receiver) = oneshot::channel();
        let worker = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = receiver.await;
                })
                .await
                .expect("fixture server");
        });
        Self {
            url,
            stop: Some(stop),
            worker: Some(worker),
        }
    }

    async fn client(&self, binding: TaskBinding) -> TaskClient {
        TaskClient::connect(
            &self.url,
            TaskClientConfig {
                allowed_origins: vec![self.url.clone()],
                credential: PeerCredential {
                    subject: "test_subject".to_string(),
                    bearer_token: "x".repeat(40),
                },
                preferred_bindings: vec![binding],
            },
        )
        .await
        .expect("fixture discovery")
    }

    async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.as_mut() {
            timeout(TASK_SHUTDOWN_DEADLINE, worker)
                .await
                .expect("bounded shutdown")
                .expect("joined fixture");
        }
        self.worker.take();
    }
}
impl Drop for SsePeer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

fn sse_response(chunks: Vec<Bytes>) -> impl IntoResponse {
    (
        [("Content-Type", "text/event-stream")],
        Body::from_stream(stream::iter(chunks.into_iter().map(Ok::<_, Infallible>))),
    )
}

fn snapshot(index: usize, terminal: bool) -> (Value, TaskSnapshot) {
    let text = format!("{index:02}-café-{}", "x".repeat(15 * 1024));
    let state = if terminal {
        TaskState::Completed
    } else {
        TaskState::Working
    };
    let wire = json!({"task": {
        "id": TASK_ID, "contextId": CONTEXT_ID,
        "status": {"state": if terminal { "TASK_STATE_COMPLETED" } else { "TASK_STATE_WORKING" }},
        "artifacts": [{"artifactId": "result", "parts": [{"text": text}]}]
    }});
    let expected = TaskSnapshot {
        task_id: TASK_ID.to_string(),
        context_id: CONTEXT_ID.to_string(),
        state,
        artifacts: vec![TaskArtifact {
            artifact_id: "result".to_string(),
            name: None,
            parts: vec![TaskPart::Text(text)],
        }],
        error_code: None,
    };
    expected.validate().expect("valid bounded snapshot");
    (wire, expected)
}

fn event(value: Value, rpc_id: Option<&Value>, newline: &str) -> Bytes {
    let value = match rpc_id {
        Some(id) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
        None => value,
    };
    Bytes::from(format!("data: {value}{newline}{newline}"))
}

async fn assert_coalesced_matches_separate(binding: TaskBinding) {
    let expected = (0..EVENT_COUNT)
        .map(|index| snapshot(index, index + 1 == EVENT_COUNT).1)
        .collect::<Vec<_>>();
    for coalesced in [false, true] {
        let peer = SsePeer::start(binding, move |rpc_id| {
            let events = (0..EVENT_COUNT)
                .map(|index| {
                    let frame = event(
                        snapshot(index, index + 1 == EVENT_COUNT).0,
                        rpc_id.as_ref(),
                        "\n",
                    );
                    assert!(frame.len() < MAX_TASK_WIRE_BYTES);
                    frame
                })
                .collect::<Vec<_>>();
            assert!(events.iter().map(Bytes::len).sum::<usize>() > MAX_TASK_WIRE_BYTES);
            if coalesced {
                vec![Bytes::from(events.concat())]
            } else {
                events
            }
        })
        .await;
        let client = peer.client(binding).await;
        let mut updates = client.subscribe(TASK_ID).await.expect("subscription");
        for (index, expected) in expected.iter().enumerate() {
            let actual = timeout(TASK_CALL_DEADLINE, updates.next())
                .await
                .expect("bounded next event")
                .expect("event before terminal");
            let actual = actual.unwrap_or_else(|error| {
                panic!("{binding:?}, coalesced={coalesced}, event={index}: {error}")
            });
            assert_eq!(&actual, expected);
        }
        assert!(updates.next().await.is_none());
        drop(updates);
        peer.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_json_coalesced_events_match_separate_events() {
    assert_coalesced_matches_separate(TaskBinding::HttpJson).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_rpc_coalesced_events_match_separate_events() {
    assert_coalesced_matches_separate(TaskBinding::JsonRpc).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delimiters_utf8_comments_and_empty_frames_can_span_chunks() {
    for binding in [TaskBinding::HttpJson, TaskBinding::JsonRpc] {
        for newline in ["\n", "\r\n", "\r"] {
            let peer = SsePeer::start(binding, move |rpc_id| {
                let frame = event(snapshot(0, true).0, rpc_id.as_ref(), newline);
                let utf8 = frame
                    .windows(2)
                    .position(|bytes| bytes == "é".as_bytes())
                    .expect("multi-byte text");
                let delimiter = frame.len() - 2 * newline.len();
                let mut cuts = vec![0, utf8 + 1];
                cuts.extend(delimiter..=frame.len());
                let mut chunks = vec![Bytes::from(format!(
                    "{newline}{newline}: keepalive{newline}{newline}data:{newline}{newline}"
                ))];
                chunks.extend(cuts.windows(2).map(|cut| frame.slice(cut[0]..cut[1])));
                // A terminal snapshot must stop iteration, even with trailing invalid data.
                chunks.push(Bytes::from_static(b"data: not-json\n\n"));
                chunks
            })
            .await;
            let client = peer.client(binding).await;
            let mut updates = client.subscribe(TASK_ID).await.expect("subscription");
            assert_eq!(updates.next().await, Some(Ok(snapshot(0, true).1)));
            assert!(updates.next().await.is_none());
            drop(updates);
            peer.shutdown().await;
        }
    }
}

fn padded_event(value: Value, rpc_id: Option<&Value>, size: usize) -> Bytes {
    let frame = event(value, rpc_id, "\r\n");
    // A long comment pads the wire frame without exceeding the snapshot's payload limit.
    let comment_size = size - frame.len();
    assert!(comment_size >= 3);
    let mut padded = vec![b':'];
    padded.resize(comment_size - 2, b'x');
    padded.extend_from_slice(b"\r\n");
    padded.extend_from_slice(&frame);
    assert_eq!(padded.len(), size);
    Bytes::from(padded)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_event_limit_is_inclusive_and_rejects_a_single_oversized_event() {
    for binding in [TaskBinding::HttpJson, TaskBinding::JsonRpc] {
        for size in [MAX_TASK_WIRE_BYTES, MAX_TASK_WIRE_BYTES + 1] {
            for coalesced in [false, true] {
                let peer = SsePeer::start(binding, move |rpc_id| {
                    let frame = padded_event(snapshot(0, true).0, rpc_id.as_ref(), size);
                    if coalesced {
                        vec![frame]
                    } else {
                        frame.chunks(1023).map(Bytes::copy_from_slice).collect()
                    }
                })
                .await;
                let client = peer.client(binding).await;
                let mut updates = client.subscribe(TASK_ID).await.expect("subscription");
                let expected = if size == MAX_TASK_WIRE_BYTES {
                    Ok(snapshot(0, true).1)
                } else {
                    Err(TaskGatewayError::Protocol)
                };
                assert_eq!(updates.next().await, Some(expected));
                assert!(updates.next().await.is_none());
                drop(updates);
                peer.shutdown().await;
            }
        }
    }
}
