use vibemux_a2a::task_contract::{TaskList, TaskListRequest};
// Official TCK executor fixture; never invokes a model or opens the core store.
use async_trait::async_trait;
use futures::stream;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::broadcast,
    time::sleep,
};
use vibemux_a2a::{
    GrpcTaskServer, PeerCredential, TaskArtifact, TaskBackend, TaskEventStream, TaskGatewayError,
    TaskMessage, TaskPart, TaskReply, TaskRequest, TaskServer, TaskServerConfig, TaskSnapshot,
    TaskState,
};

const FIXTURE_TOKEN: &str = "vibemux_tck_fixture_token_01234567890123456789";
const MAX_FIXTURE_TASKS: usize = 4096;
struct Entry {
    subject: String,
    snapshot: TaskSnapshot,
    updates: broadcast::Sender<TaskSnapshot>,
    finish_on_subscribe: bool,
}
#[derive(Default, Clone)]
struct FixtureBackend {
    entries: Arc<Mutex<BTreeMap<String, Entry>>>,
}

fn artifacts(prefix: &str) -> Vec<TaskArtifact> {
    let part = if prefix.starts_with("tck-artifact-file-url") {
        TaskPart::Url {
            url: "https://example.com/output.txt".into(),
            media_type: Some("text/plain".into()),
            filename: Some("output.txt".into()),
        }
    } else if prefix.starts_with("tck-artifact-file")
        || prefix.starts_with("tck-stream-artifact-file")
    {
        TaskPart::Raw {
            bytes: b"Fixture file content".to_vec(),
            media_type: Some("text/plain".into()),
            filename: Some("output.txt".into()),
        }
    } else if prefix.starts_with("tck-artifact-data") {
        TaskPart::Data(json!({"key": "value", "count": 42}))
    } else {
        TaskPart::Text(
            if prefix.starts_with("tck-artifact-text") {
                "Generated text content"
            } else {
                "Hello from TCK"
            }
            .into(),
        )
    };
    vec![TaskArtifact {
        artifact_id: "fixture_artifact".into(),
        name: Some("output.txt".into()),
        parts: vec![part],
    }]
}

#[async_trait]
impl TaskBackend for FixtureBackend {
    async fn send(
        &self,
        subject: &str,
        request: TaskRequest,
    ) -> Result<TaskSnapshot, TaskGatewayError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| TaskGatewayError::Internal)?;
        if let Some(task_id) = request.task_id {
            let entry = entries
                .get_mut(&task_id)
                .ok_or(TaskGatewayError::NotFound)?;
            if entry.subject != subject {
                return Err(TaskGatewayError::Forbidden);
            }
            if entry.snapshot.context_id != request.context_id || entry.snapshot.state.is_terminal()
            {
                return Err(TaskGatewayError::InvalidRequest);
            }
            entry.snapshot.state = TaskState::Completed;
            entry.snapshot.artifacts = artifacts(&request.request_id);
            let _ = entry.updates.send(entry.snapshot.clone());
            return Ok(entry.snapshot.clone());
        }
        if entries.len() >= MAX_FIXTURE_TASKS {
            return Err(TaskGatewayError::Busy);
        }
        let id = format!("fixture_{}", uuid::Uuid::new_v4().simple());
        let long_running = request
            .request_id
            .starts_with("test-resubscribe-message-id");
        let streaming = request.request_id.starts_with("tck-stream");
        let state = if request.request_id.starts_with("tck-input-required") {
            TaskState::InputRequired
        } else if request.request_id.starts_with("tck-reject-task") {
            TaskState::Rejected
        } else if streaming || long_running {
            TaskState::Working
        } else {
            TaskState::Completed
        };
        let snapshot = TaskSnapshot {
            task_id: id.clone(),
            context_id: request.context_id,
            state,
            artifacts: if state == TaskState::Completed {
                artifacts(&request.request_id)
            } else {
                vec![]
            },
            error_code: None,
        };
        let (updates, _) = broadcast::channel(32);
        entries.insert(
            id,
            Entry {
                subject: subject.to_string(),
                snapshot: snapshot.clone(),
                updates,
                finish_on_subscribe: streaming,
            },
        );
        Ok(snapshot)
    }
    async fn send_reply(
        &self,
        subject: &str,
        request: TaskRequest,
    ) -> Result<TaskReply, TaskGatewayError> {
        if request.request_id.starts_with("tck-message-response") {
            return Ok(TaskReply::Message(TaskMessage {
                message_id: format!("fixture_{}", uuid::Uuid::new_v4().simple()),
                context_id: request.context_id,
                parts: vec![TaskPart::Text("Direct message response".into())],
            }));
        }
        self.send(subject, request).await.map(TaskReply::Task)
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
        let mut tasks = entries
            .values()
            .filter(|entry| entry.subject == subject)
            .filter(|entry| {
                request
                    .context_id
                    .as_ref()
                    .is_none_or(|context| *context == entry.snapshot.context_id)
            })
            .filter(|entry| {
                request
                    .state
                    .is_none_or(|state| state == entry.snapshot.state)
            })
            .map(|entry| entry.snapshot.clone())
            .collect::<Vec<_>>();
        let total_size = u32::try_from(tasks.len()).map_err(|_| TaskGatewayError::Internal)?;
        let offset = request
            .page_token
            .as_deref()
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| TaskGatewayError::InvalidRequest)?;
        if offset > tasks.len() {
            return Err(TaskGatewayError::InvalidRequest);
        }
        let end = (offset + request.page_size as usize).min(tasks.len());
        let next_page_token = (end < tasks.len()).then(|| end.to_string());
        tasks = tasks[offset..end].to_vec();
        if !request.include_artifacts {
            for task in &mut tasks {
                task.artifacts.clear();
            }
        }
        Ok(TaskList {
            tasks,
            next_page_token,
            total_size,
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
        if entry.snapshot.state.is_terminal() {
            return Err(TaskGatewayError::Unsupported);
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
        let (snapshot, updates, finish) = {
            let entries = self
                .entries
                .lock()
                .map_err(|_| TaskGatewayError::Internal)?;
            let entry = entries.get(task_id).ok_or(TaskGatewayError::NotFound)?;
            if entry.subject != subject {
                return Err(TaskGatewayError::Forbidden);
            }
            (
                entry.snapshot.clone(),
                entry.updates.subscribe(),
                entry.finish_on_subscribe,
            )
        };
        let entries = self.entries.clone();
        let task_id = task_id.to_string();
        let state = (Some(snapshot), updates, finish, entries, task_id, false);
        Ok(Box::pin(stream::unfold(
            state,
            |(mut first, mut updates, finish, entries, task_id, done)| async move {
                if done {
                    return None;
                }
                if let Some(snapshot) = first.take() {
                    let done = snapshot.state.is_terminal();
                    return Some((
                        Ok(snapshot),
                        (first, updates, finish, entries, task_id, done),
                    ));
                }
                if finish {
                    sleep(Duration::from_millis(25)).await;
                    let next = (|| {
                        let mut entries = entries.lock().map_err(|_| TaskGatewayError::Internal)?;
                        let entry = entries
                            .get_mut(&task_id)
                            .ok_or(TaskGatewayError::NotFound)?;
                        if !entry.snapshot.state.is_terminal() {
                            entry.snapshot.state = TaskState::Completed;
                            entry.snapshot.artifacts = artifacts("tck-stream-artifact-text");
                            let _ = entry.updates.send(entry.snapshot.clone());
                        }
                        Ok(entry.snapshot.clone())
                    })();
                    return Some((next, (first, updates, false, entries, task_id, true)));
                }
                let next = match updates.recv().await {
                    Ok(snapshot) => Ok(snapshot),
                    Err(broadcast::error::RecvError::Lagged(_)) => Err(TaskGatewayError::Busy),
                    Err(broadcast::error::RecvError::Closed) => return None,
                };
                let done = next
                    .as_ref()
                    .map_or(true, |snapshot| snapshot.state.is_terminal());
                Some((next, (first, updates, false, entries, task_id, done)))
            },
        )))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = Arc::new(FixtureBackend::default());
    let config = TaskServerConfig {
        peer_id: "tck_fixture".into(),
        credentials: vec![PeerCredential {
            subject: "tck_fixture".into(),
            bearer_token: FIXTURE_TOKEN.into(),
        }],
    };
    let grpc = GrpcTaskServer::start(config.clone(), backend.clone()).await?;
    let http = TaskServer::start_with_grpc(config, backend, grpc.base_url()).await?;
    println!(
        "{}",
        json!({"fixture":"vibemux_task_tck_v1","http_url":http.base_url(),"grpc_url":grpc.base_url()})
    );
    let mut command = String::new();
    BufReader::new(tokio::io::stdin())
        .read_line(&mut command)
        .await?;
    http.shutdown().await?;
    grpc.shutdown().await?;
    Ok(())
}
