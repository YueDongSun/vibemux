//! Real loopback gRPC tests; the backend is an isolated in-memory test fixture.
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::broadcast, time::timeout};
use vibemux_a2a::task_contract::{TaskList, TaskListRequest};
use vibemux_a2a::{
    GrpcTaskClient, GrpcTaskServer, PeerCredential, TaskBackend, TaskEventStream, TaskGatewayError,
    TaskReply, TaskRequest, TaskServerConfig, TaskSnapshot, TaskState,
};

const ALICE_TOKEN: &str = "fixture_alice_token_01234567890123456789";
const BOB_TOKEN: &str = "fixture_bob_token_012345678901234567890";

struct Entry {
    subject: String,
    snapshot: TaskSnapshot,
    updates: broadcast::Sender<TaskSnapshot>,
}
#[derive(Default)]
struct Backend {
    entries: Mutex<BTreeMap<String, Entry>>,
}

#[async_trait]
impl TaskBackend for Backend {
    async fn send(
        &self,
        subject: &str,
        request: TaskRequest,
    ) -> Result<TaskSnapshot, TaskGatewayError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| TaskGatewayError::Internal)?;
        if let Some(entry) = entries.get(&request.request_id) {
            return if entry.subject == subject {
                Ok(entry.snapshot.clone())
            } else {
                Err(TaskGatewayError::Forbidden)
            };
        }
        let snapshot = TaskSnapshot {
            task_id: request.request_id.clone(),
            context_id: request.context_id,
            state: TaskState::Working,
            artifacts: vec![],
            error_code: None,
        };
        let (updates, _) = broadcast::channel(8);
        entries.insert(
            request.request_id,
            Entry {
                subject: subject.to_string(),
                snapshot: snapshot.clone(),
                updates,
            },
        );
        Ok(snapshot)
    }
    async fn list(
        &self,
        subject: &str,
        request: TaskListRequest,
    ) -> Result<TaskList, TaskGatewayError> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| TaskGatewayError::Internal)?;
        let tasks: Vec<_> = entries
            .values()
            .filter(|entry| entry.subject == subject)
            .map(|entry| entry.snapshot.clone())
            .take(request.page_size as usize)
            .collect();
        Ok(TaskList {
            total_size: u32::try_from(tasks.len()).map_err(|_| TaskGatewayError::Internal)?,
            tasks,
            next_page_token: None,
        })
    }
    async fn get(&self, subject: &str, task_id: &str) -> Result<TaskSnapshot, TaskGatewayError> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| TaskGatewayError::Internal)?;
        let entry = entries.get(task_id).ok_or(TaskGatewayError::NotFound)?;
        if entry.subject != subject {
            return Err(TaskGatewayError::Forbidden);
        }
        Ok(entry.snapshot.clone())
    }
    async fn cancel(&self, subject: &str, task_id: &str) -> Result<TaskSnapshot, TaskGatewayError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| TaskGatewayError::Internal)?;
        let entry = entries.get_mut(task_id).ok_or(TaskGatewayError::NotFound)?;
        if entry.subject != subject {
            return Err(TaskGatewayError::Forbidden);
        }
        entry.snapshot.state = TaskState::Canceled;
        let _ = entry.updates.send(entry.snapshot.clone());
        Ok(entry.snapshot.clone())
    }
    async fn subscribe(
        &self,
        subject: &str,
        task_id: &str,
    ) -> Result<TaskEventStream, TaskGatewayError> {
        let (snapshot, updates) = {
            let entries = self
                .entries
                .lock()
                .map_err(|_| TaskGatewayError::Internal)?;
            let entry = entries.get(task_id).ok_or(TaskGatewayError::NotFound)?;
            if entry.subject != subject {
                return Err(TaskGatewayError::Forbidden);
            }
            (entry.snapshot.clone(), entry.updates.subscribe())
        };
        let tail = stream::unfold(updates, |mut updates| async move {
            match updates.recv().await {
                Ok(snapshot) => Some((Ok(snapshot), updates)),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    Some((Err(TaskGatewayError::Busy), updates))
                }
                Err(broadcast::error::RecvError::Closed) => None,
            }
        });
        Ok(Box::pin(
            stream::once(async move { Ok(snapshot) }).chain(tail),
        ))
    }
}

fn credential(subject: &str, token: &str) -> PeerCredential {
    PeerCredential {
        subject: subject.to_string(),
        bearer_token: token.to_string(),
    }
}
fn config() -> TaskServerConfig {
    TaskServerConfig {
        peer_id: "grpc_fixture".to_string(),
        credentials: vec![
            credential("alice", ALICE_TOKEN),
            credential("bob", BOB_TOKEN),
        ],
    }
}
fn request(id: &str) -> TaskRequest {
    TaskRequest {
        request_id: id.to_string(),
        context_id: "grpc_context".to_string(),
        task_id: None,
        idempotency_key: id.to_string(),
        payload: json!({"fixture": true}),
    }
}
async fn next_event(stream: &mut TaskEventStream) -> TaskSnapshot {
    timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("stream deadline")
        .expect("stream event")
        .expect("valid event")
}

#[tokio::test]
async fn grpc_task_identity_resubscription_and_explicit_cancellation() {
    let backend = Arc::new(Backend::default());
    let server = GrpcTaskServer::start(config(), backend.clone())
        .await
        .expect("server");
    let client = GrpcTaskClient::connect(
        &server.address().to_string(),
        credential("alice", ALICE_TOKEN),
    )
    .await
    .expect("client");
    let TaskReply::Task(created) = client.send(&request("grpc_task")).await.expect("send") else {
        panic!("expected task")
    };
    assert_eq!(created.state, TaskState::Working);
    assert_eq!(created.task_id, "grpc_task");
    assert_eq!(client.get(&created.task_id).await.expect("get"), created);
    let mut first = client.subscribe(&created.task_id).await.expect("subscribe");
    assert_eq!(next_event(&mut first).await, created);
    drop(first);
    assert_eq!(
        client
            .get(&created.task_id)
            .await
            .expect("not canceled by disconnect"),
        created
    );
    let mut resumed = client
        .subscribe(&created.task_id)
        .await
        .expect("resubscribe");
    assert_eq!(next_event(&mut resumed).await, created);
    let canceled = client.cancel(&created.task_id).await.expect("cancel");
    assert_eq!(canceled.state, TaskState::Canceled);
    assert_eq!(next_event(&mut resumed).await, canceled);
    assert!(resumed.next().await.is_none());
    drop(client);
    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn grpc_authentication_and_per_task_authorization_are_shared() {
    let backend = Arc::new(Backend::default());
    let server = GrpcTaskServer::start(config(), backend.clone())
        .await
        .expect("server");
    let alice = GrpcTaskClient::connect(&server.base_url(), credential("alice", ALICE_TOKEN))
        .await
        .expect("alice");
    alice.send(&request("private_task")).await.expect("create");
    let bob = GrpcTaskClient::connect(&server.base_url(), credential("bob", BOB_TOKEN))
        .await
        .expect("bob");
    assert_eq!(
        bob.get("private_task").await,
        Err(TaskGatewayError::Forbidden)
    );
    assert_eq!(
        bob.cancel("private_task").await,
        Err(TaskGatewayError::Forbidden)
    );
    let list_request = TaskListRequest {
        context_id: None,
        state: None,
        page_size: 8,
        page_token: None,
        include_artifacts: false,
    };
    assert_eq!(
        alice
            .list(&list_request)
            .await
            .expect("authorized list")
            .tasks
            .len(),
        1
    );
    assert!(
        bob.list(&list_request)
            .await
            .expect("other subject list")
            .tasks
            .is_empty()
    );
    let bad = GrpcTaskClient::connect(
        &server.base_url(),
        credential("alice", "invalid_fixture_token_0123456789012345"),
    )
    .await
    .expect("transport only");
    assert_eq!(
        bad.send(&request("rejected_task")).await,
        Err(TaskGatewayError::Unauthorized)
    );
    assert_eq!(backend.entries.lock().expect("lock").len(), 1);
    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn grpc_shutdown_closes_live_subscription_without_canceling_backend_task() {
    let backend = Arc::new(Backend::default());
    let server = GrpcTaskServer::start(config(), backend.clone())
        .await
        .expect("server");
    let address = server.address();
    let client = GrpcTaskClient::connect(&server.base_url(), credential("alice", ALICE_TOKEN))
        .await
        .expect("client");
    let mut events = client
        .send_stream(&request("shutdown_task"))
        .await
        .expect("send stream");
    assert_eq!(next_event(&mut events).await.state, TaskState::Working);
    server.shutdown().await.expect("shutdown drains stream");
    let ended = timeout(Duration::from_secs(3), events.next())
        .await
        .expect("stream closed");
    assert!(matches!(ended, None | Some(Err(_))));
    assert_eq!(
        backend
            .get("alice", "shutdown_task")
            .await
            .expect("backend task")
            .state,
        TaskState::Working
    );
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn grpc_rejects_oversized_wire_before_backend_and_non_loopback_endpoint() {
    let backend = Arc::new(Backend::default());
    let server = GrpcTaskServer::start(config(), backend.clone())
        .await
        .expect("server");
    let mut client =
        a2a_pb::proto::a2a_service_client::A2aServiceClient::connect(server.base_url())
            .await
            .expect("client");
    let message = a2a::Message::new(
        a2a::Role::User,
        vec![a2a::Part::text("x".repeat(80 * 1024))],
    );
    let native = a2a::SendMessageRequest {
        message,
        configuration: None,
        metadata: None,
        tenant: None,
    };
    let mut wire = tonic::Request::new(a2a_pb::pbconv::to_proto_send_message_request(&native));
    wire.metadata_mut().insert(
        "authorization",
        format!("Bearer {ALICE_TOKEN}").parse().expect("metadata"),
    );
    wire.metadata_mut()
        .insert("a2a-version", "1.0".parse().expect("metadata"));
    let error = client
        .send_message(wire)
        .await
        .expect_err("oversized wire rejected");
    assert!(matches!(
        error.code(),
        tonic::Code::OutOfRange | tonic::Code::ResourceExhausted
    ));
    assert!(backend.entries.lock().expect("lock").is_empty());
    for endpoint in [
        "http://example.com:8080",
        "http://0.0.0.0:8080",
        "http://localhost:8080",
        "http://127.0.0.1:8080/private",
        "http://user:pass@127.0.0.1:8080",
    ] {
        assert!(matches!(
            GrpcTaskClient::connect(endpoint, credential("alice", ALICE_TOKEN)).await,
            Err(TaskGatewayError::InvalidRequest)
        ));
    }
    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn grpc_reports_official_error_codes_and_error_info() {
    use tonic_types::StatusExt;
    let server = GrpcTaskServer::start(config(), Arc::new(Backend::default()))
        .await
        .expect("server");
    let mut client =
        a2a_pb::proto::a2a_service_client::A2aServiceClient::connect(server.base_url())
            .await
            .expect("client");
    let authenticated = |message| {
        let mut request = tonic::Request::new(message);
        request.metadata_mut().insert(
            "authorization",
            format!("Bearer {ALICE_TOKEN}").parse().expect("metadata"),
        );
        request
            .metadata_mut()
            .insert("a2a-version", "1.0".parse().expect("metadata"));
        request
    };
    let native = a2a::GetTaskRequest {
        id: "nonexistent_task".into(),
        history_length: None,
        tenant: None,
    };
    let error = client
        .get_task(authenticated(a2a_pb::pbconv::to_proto_get_task_request(
            &native,
        )))
        .await
        .expect_err("not found");
    assert_eq!(error.code(), tonic::Code::NotFound);
    let details = error.get_error_details();
    let info = details.error_info().expect("google.rpc.ErrorInfo");
    assert_eq!(info.reason, "TASK_NOT_FOUND");
    assert_eq!(info.domain, "a2a-protocol.org");
    let mut version_request = authenticated(a2a_pb::pbconv::to_proto_get_task_request(&native));
    version_request
        .metadata_mut()
        .insert("a2a-version", "999.0".parse().expect("metadata"));
    let error = client
        .get_task(version_request)
        .await
        .expect_err("unsupported version");
    assert_eq!(error.code(), tonic::Code::Unimplemented);
    assert_eq!(
        error
            .get_error_details()
            .error_info()
            .expect("version details")
            .reason,
        "VERSION_NOT_SUPPORTED"
    );
    let mut push = tonic::Request::new(a2a_pb::proto::TaskPushNotificationConfig::default());
    push.metadata_mut().insert(
        "authorization",
        format!("Bearer {ALICE_TOKEN}").parse().expect("metadata"),
    );
    push.metadata_mut()
        .insert("a2a-version", "1.0".parse().expect("metadata"));
    let error = client
        .create_task_push_notification_config(push)
        .await
        .expect_err("push unsupported");
    assert_eq!(error.code(), tonic::Code::Unimplemented);
    assert_eq!(
        error
            .get_error_details()
            .error_info()
            .expect("push details")
            .reason,
        "PUSH_NOTIFICATION_NOT_SUPPORTED"
    );
    server.shutdown().await.expect("shutdown");
}
