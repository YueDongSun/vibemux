use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use vibemux_a2a::*;

#[derive(Default)]
struct Backend {
    tasks: Mutex<HashMap<String, (String, watch::Sender<TaskSnapshot>)>>,
    keys: Mutex<HashMap<(String, String), String>>,
}
impl Backend {
    fn complete(&self, id: &str) {
        self.tasks
            .lock()
            .expect("tasks")
            .get(id)
            .expect("task")
            .1
            .send_modify(|task| {
                task.state = TaskState::Completed;
                task.artifacts
                    .push(TaskArtifact::data("result", json!({"checked":true})));
            });
    }
}
#[async_trait]
impl TaskBackend for Backend {
    async fn list(
        &self,
        subject: &str,
        request: TaskListRequest,
    ) -> Result<TaskList, TaskGatewayError> {
        request.validate()?;
        let mut tasks = self
            .tasks
            .lock()
            .expect("tasks")
            .values()
            .filter(|(owner, _)| owner == subject)
            .map(|(_, sender)| sender.borrow().clone())
            .filter(|task| {
                request
                    .context_id
                    .as_ref()
                    .is_none_or(|id| id == &task.context_id)
                    && request.state.is_none_or(|state| state == task.state)
            })
            .collect::<Vec<_>>();
        tasks.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        let total_size = tasks.len() as u32;
        let start = request
            .page_token
            .as_deref()
            .map(str::parse::<usize>)
            .transpose()
            .map_err(|_| TaskGatewayError::InvalidRequest)?
            .unwrap_or(0);
        if start > tasks.len() {
            return Err(TaskGatewayError::InvalidRequest);
        }
        let end = start
            .saturating_add(request.page_size as usize)
            .min(tasks.len());
        let mut page = tasks[start..end].to_vec();
        if !request.include_artifacts {
            for task in &mut page {
                task.artifacts.clear();
            }
        }
        Ok(TaskList {
            tasks: page,
            next_page_token: (end < tasks.len()).then(|| end.to_string()),
            total_size,
        })
    }
    async fn send(
        &self,
        subject: &str,
        request: TaskRequest,
    ) -> Result<TaskSnapshot, TaskGatewayError> {
        request.validate()?;
        if let Some(id) = &request.task_id {
            let task = self.get(subject, id).await?;
            if task.context_id != request.context_id {
                return Err(TaskGatewayError::InvalidRequest);
            }
            return Ok(task);
        }
        let key = (subject.to_string(), request.idempotency_key.clone());
        let existing = { self.keys.lock().expect("keys").get(&key).cloned() };
        if let Some(id) = existing {
            return self.get(subject, &id).await;
        }
        let task = TaskSnapshot {
            task_id: format!("task_{}", request.request_id),
            context_id: request.context_id,
            state: TaskState::Working,
            artifacts: vec![TaskArtifact::data("input", request.payload)],
            error_code: None,
        };
        let (sender, _) = watch::channel(task.clone());
        self.tasks
            .lock()
            .expect("tasks")
            .insert(task.task_id.clone(), (subject.to_string(), sender));
        self.keys
            .lock()
            .expect("keys")
            .insert(key, task.task_id.clone());
        Ok(task)
    }
    async fn get(&self, subject: &str, id: &str) -> Result<TaskSnapshot, TaskGatewayError> {
        let tasks = self.tasks.lock().expect("tasks");
        let (owner, task) = tasks.get(id).ok_or(TaskGatewayError::NotFound)?;
        if owner != subject {
            return Err(TaskGatewayError::Forbidden);
        }
        Ok(task.borrow().clone())
    }
    async fn cancel(&self, subject: &str, id: &str) -> Result<TaskSnapshot, TaskGatewayError> {
        self.get(subject, id).await?;
        self.tasks
            .lock()
            .expect("tasks")
            .get(id)
            .expect("task")
            .1
            .send_modify(|task| task.state = TaskState::Canceled);
        self.get(subject, id).await
    }
    async fn subscribe(
        &self,
        subject: &str,
        id: &str,
    ) -> Result<TaskEventStream, TaskGatewayError> {
        self.get(subject, id).await?;
        let receiver = self
            .tasks
            .lock()
            .expect("tasks")
            .get(id)
            .expect("task")
            .1
            .subscribe();
        Ok(Box::pin(stream::unfold(
            (receiver, true),
            |(mut receiver, first)| async move {
                if !first && receiver.changed().await.is_err() {
                    return None;
                }
                let task = receiver.borrow_and_update().clone();
                Some((Ok(task), (receiver, false)))
            },
        )))
    }
}
fn credential(subject: &str) -> PeerCredential {
    PeerCredential {
        subject: subject.to_string(),
        bearer_token: format!("{subject}_{}", "x".repeat(40)),
    }
}
fn request(id: &str) -> TaskRequest {
    TaskRequest {
        request_id: id.to_string(),
        context_id: format!("context_{id}"),
        task_id: None,
        idempotency_key: format!("key_{id}"),
        payload: json!({"operation":"bounded_data","value":42}),
    }
}
async fn client(url: &str, subject: &str, binding: TaskBinding) -> TaskClient {
    TaskClient::connect(
        url,
        TaskClientConfig {
            allowed_origins: vec![url.to_string()],
            credential: credential(subject),
            preferred_bindings: vec![binding],
        },
    )
    .await
    .expect("client")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_http_jsonrpc_task_auth_cancel_resubscribe_and_shutdown() {
    for binding in [TaskBinding::HttpJson, TaskBinding::JsonRpc] {
        let backend = Arc::new(Backend::default());
        let server = TaskServer::start(
            TaskServerConfig {
                peer_id: "test_peer".to_string(),
                credentials: vec![credential("alice"), credential("bob")],
            },
            backend.clone(),
        )
        .await
        .expect("server");
        let alice = client(server.base_url(), "alice", binding).await;
        let bob = client(server.base_url(), "bob", binding).await;
        assert_eq!(alice.binding(), binding);
        let first = alice.send(&request("one")).await.expect("send");
        assert_eq!(first.context_id, "context_one");
        assert_eq!(
            alice.send(&request("one")).await.expect("idempotent"),
            first
        );
        assert_eq!(alice.get(&first.task_id).await.expect("get"), first);
        assert_eq!(
            bob.get(&first.task_id).await.expect_err("foreign get"),
            TaskGatewayError::Forbidden
        );
        assert_eq!(
            bob.cancel(&first.task_id)
                .await
                .expect_err("foreign cancel"),
            TaskGatewayError::Forbidden
        );
        assert!(matches!(
            bob.subscribe(&first.task_id).await,
            Err(TaskGatewayError::Forbidden)
        ));
        let mut updates = alice.subscribe(&first.task_id).await.expect("subscribe");
        assert_eq!(
            updates.next().await.expect("snapshot").expect("valid"),
            first
        );
        drop(updates);
        let mut reconnected = alice.subscribe(&first.task_id).await.expect("resubscribe");
        assert_eq!(
            reconnected.next().await.expect("snapshot").expect("valid"),
            first
        );
        backend
            .tasks
            .lock()
            .expect("tasks")
            .get(&first.task_id)
            .expect("task")
            .1
            .send_modify(|task| task.state = TaskState::InputRequired);
        assert_eq!(
            reconnected
                .next()
                .await
                .expect("interrupted update")
                .expect("valid")
                .state,
            TaskState::InputRequired
        );
        backend.complete(&first.task_id);
        let artifact_update = reconnected
            .next()
            .await
            .expect("artifact update")
            .expect("valid");
        assert_eq!(artifact_update.state, TaskState::InputRequired);
        assert_eq!(artifact_update.artifacts.len(), 2);
        let done = reconnected.next().await.expect("completed").expect("valid");
        assert_eq!(done.state, TaskState::Completed);
        assert_eq!(done.artifacts.len(), 2);
        assert!(reconnected.next().await.is_none());
        let page_request = TaskListRequest {
            context_id: None,
            state: None,
            page_size: 1,
            page_token: None,
            include_artifacts: true,
        };
        let alice_page = alice.list(&page_request).await.expect("principal list");
        assert_eq!(alice_page.total_size, 1);
        assert_eq!(alice_page.tasks[0].task_id, first.task_id);
        assert!(
            bob.list(&page_request)
                .await
                .expect("foreign principal list")
                .tasks
                .is_empty()
        );
        let cancel = alice.send(&request("two")).await.expect("second");
        assert_eq!(
            alice.cancel(&cancel.task_id).await.expect("cancel").state,
            TaskState::Canceled
        );
        assert_eq!(
            alice
                .cancel(&cancel.task_id)
                .await
                .expect_err("terminal not cancelable"),
            TaskGatewayError::Unsupported
        );
        let running = alice.send(&request("three")).await.expect("third");
        let mut open = alice.subscribe(&running.task_id).await.expect("open");
        open.next().await.expect("snapshot").expect("valid");
        let address = server.address();
        server.shutdown().await.expect("joined shutdown");
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
        assert_eq!(
            backend
                .get("alice", &running.task_id)
                .await
                .expect("backend remains owned")
                .state,
            TaskState::Working
        );
        assert!(open.next().await.expect("explicit disconnect").is_err());
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_inputs_and_wrong_bearer_never_reach_backend() {
    let backend = Arc::new(Backend::default());
    let server = TaskServer::start(
        TaskServerConfig {
            peer_id: "test_peer".to_string(),
            credentials: vec![credential("alice")],
        },
        backend.clone(),
    )
    .await
    .expect("server");
    let bad = TaskClient::connect(
        server.base_url(),
        TaskClientConfig {
            allowed_origins: vec![server.base_url().to_string()],
            credential: credential("bad"),
            preferred_bindings: vec![TaskBinding::HttpJson],
        },
    )
    .await
    .expect("discovery is public");
    assert_eq!(
        bad.send(&request("denied")).await.expect_err("denied"),
        TaskGatewayError::Unauthorized
    );
    let response = reqwest::Client::new()
        .post(format!("{}/message:send", server.base_url()))
        .header("Content-Type", "application/json")
        .body("x".repeat(task_contract::MAX_TASK_WIRE_BYTES + 1))
        .send()
        .await
        .expect("oversize request");
    assert_eq!(response.status(), 413);
    assert!(backend.tasks.lock().expect("tasks").is_empty());
    let debug = format!("{:?}", credential("alice"));
    assert!(!debug.contains(&credential("alice").bearer_token));
    let mut oversized = request("oversized");
    oversized.payload = json!({"data":"x".repeat(task_contract::MAX_TASK_PAYLOAD_BYTES)});
    assert_eq!(
        oversized.validate().expect_err("bounded payload"),
        TaskGatewayError::InvalidRequest
    );
    server.shutdown().await.expect("shutdown");
}
#[tokio::test]
async fn destinations_are_exact_numeric_loopback_allowlists() {
    for url in [
        "http://localhost:1234",
        "http://example.com:1234",
        "http://127.0.0.1:1234/path",
        "http://127.0.0.1:1234?query=1",
        "http://user:secret@127.0.0.1:1234",
    ] {
        let result = TaskClient::connect(
            url,
            TaskClientConfig {
                allowed_origins: vec![url.to_string()],
                credential: credential("alice"),
                preferred_bindings: vec![TaskBinding::HttpJson],
            },
        )
        .await;
        assert!(matches!(result, Err(TaskGatewayError::Forbidden)));
    }
}

struct Adversary {
    url: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    worker: Option<tokio::task::JoinHandle<()>>,
}
impl Adversary {
    async fn start(mode: &str) -> Self {
        use axum::{Router, response::IntoResponse, routing::get};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let card = json!({"name":"adversary","description":"bounded test","version":"1","supportedInterfaces":[{"url":url,"protocolBinding":"HTTP+JSON","protocolVersion":"1.0"}],"capabilities":{"streaming":true},"defaultInputModes":["application/json"],"defaultOutputModes":["application/json"],"skills":[]});
        let body = match mode {
            "large_card" => "x".repeat(task_contract::MAX_TASK_WIRE_BYTES + 1),
            "escaped_card" => {
                let mut card = card.clone();
                card["supportedInterfaces"][0]["url"] = json!("http://127.0.0.1:65530");
                card.to_string()
            }
            _ => card.to_string(),
        };
        let redirect = mode == "redirect";
        let app = Router::new()
            .route(
                "/.well-known/agent-card.json",
                get(move || {
                    let body = body.clone();
                    async move {
                        if redirect {
                            (
                                axum::http::StatusCode::FOUND,
                                [("Location", "http://127.0.0.1:65530")],
                                "redirect",
                            )
                                .into_response()
                        } else {
                            ([("Content-Type", "application/json")], body).into_response()
                        }
                    }
                }),
            )
            .route(
                "/tasks/{id}",
                get(|| async {
                    (
                        [("Content-Type", "text/event-stream")],
                        "x".repeat(task_contract::MAX_TASK_WIRE_BYTES + 1),
                    )
                }),
            );
        let (stop, receiver) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = receiver.await;
                })
                .await
                .expect("serve");
        });
        Self {
            url,
            stop: Some(stop),
            worker: Some(worker),
        }
    }
    async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker.await.expect("joined");
        }
    }
}
impl Drop for Adversary {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_cards_sse_and_redirected_discovery_fail_closed() {
    for (mode, expected) in [
        ("large_card", TaskGatewayError::Protocol),
        ("escaped_card", TaskGatewayError::Forbidden),
        ("redirect", TaskGatewayError::Transport),
    ] {
        let adversary = Adversary::start(mode).await;
        let result = TaskClient::connect(
            &adversary.url,
            TaskClientConfig {
                allowed_origins: vec![adversary.url.clone()],
                credential: credential("alice"),
                preferred_bindings: vec![TaskBinding::HttpJson],
            },
        )
        .await;
        match result {
            Err(error) => assert_eq!(error, expected),
            Ok(_) => panic!("invalid discovery accepted"),
        };
        adversary.shutdown().await;
    }
    let adversary = Adversary::start("large_stream").await;
    let client = client(&adversary.url, "alice", TaskBinding::HttpJson).await;
    let mut stream = client.subscribe("probe").await.expect("SSE headers");
    assert_eq!(
        stream
            .next()
            .await
            .expect("explicit bounded error")
            .expect_err("oversized incomplete SSE"),
        TaskGatewayError::Protocol
    );
    assert!(stream.next().await.is_none());
    drop(stream);
    adversary.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_followup_infers_authorized_context_from_task() {
    let backend = Arc::new(Backend::default());
    let server = TaskServer::start(
        TaskServerConfig {
            peer_id: "test_peer".to_string(),
            credentials: vec![credential("alice")],
        },
        backend,
    )
    .await
    .expect("server");
    for binding in [TaskBinding::HttpJson, TaskBinding::JsonRpc] {
        let client = client(server.base_url(), "alice", binding).await;
        let created = client
            .send(&request(if binding == TaskBinding::HttpJson {
                "native_rest"
            } else {
                "native_rpc"
            }))
            .await
            .expect("task");
        let params = json!({"message":{"messageId":"followup","taskId":created.task_id,"role":"ROLE_USER","parts":[{"text":"Continue"}]},"configuration":{"returnImmediately":true}});
        let (url, body) = if binding == TaskBinding::HttpJson {
            (format!("{}/message:send", server.base_url()), params)
        } else {
            (
                server.base_url().to_string(),
                json!({"jsonrpc":"2.0","id":"test","method":"SendMessage","params":params}),
            )
        };
        let response = reqwest::Client::new()
            .post(url)
            .bearer_auth(credential("alice").bearer_token)
            .json(&body)
            .send()
            .await
            .expect("native send");
        assert!(response.status().is_success());
        let response: serde_json::Value = response.json().await.expect("JSON");
        let pointer = if binding == TaskBinding::HttpJson {
            "/task/contextId"
        } else {
            "/result/task/contextId"
        };
        assert_eq!(
            response
                .pointer(pointer)
                .and_then(serde_json::Value::as_str),
            Some(created.context_id.as_str())
        );
    }
    server.shutdown().await.expect("shutdown");
}
