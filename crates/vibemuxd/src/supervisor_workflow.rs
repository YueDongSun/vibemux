//! Supervisor-owned plan, delegation, independent review and verifier gate.
use crate::{
    WriterHandle,
    model_peer_process::{ModelPeerProcess, SupervisorConfig},
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio::{
    sync::watch,
    time::{Duration, Instant, sleep},
};
use vibemux_a2a::{
    task_contract::*,
    task_runtime::{TaskExecution, TaskExecutor},
};
use vibemux_types::{Run, RunSpec, RunStatus, Task, TaskId, TaskSpec, a2a::*};
use vibemux_workspace::{WorkspaceManager, sha256};

const WORKFLOW_DEADLINE: Duration = Duration::from_secs(300);
/// Remote task polling cadence. Model completions take seconds, so 250ms
/// keeps cancellation responsive while cutting poll RPCs by ~60% versus
/// 100ms. Event-driven waiting over `TaskClient::subscribe` supersedes this.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
struct RoleSubmission {
    role: &'static str,
    binding: TaskBinding,
    payload: Value,
    correlation: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorWorkOrder {
    #[serde(deserialize_with = "deserialize_u32_number")]
    pub schema_version: u32,
    pub task_description: String,
    pub input: Value,
    pub expected_result: Value,
    #[serde(default, deserialize_with = "deserialize_u32_number")]
    pub max_repairs: u32,
}
impl SupervisorWorkOrder {
    pub fn validate(&self) -> Result<(), TaskGatewayError> {
        if self.schema_version != 1
            || self.task_description.is_empty()
            || self.task_description.len() > 2048
            || self.max_repairs > 2
            || !portable_json(&self.input)
            || !portable_json(&self.expected_result)
            || serde_json::to_vec(&self.input)
                .map_err(|_| TaskGatewayError::InvalidRequest)?
                .len()
                > 4096
            || serde_json::to_vec(&self.expected_result)
                .map_err(|_| TaskGatewayError::InvalidRequest)?
                .len()
                > 4096
        {
            return Err(TaskGatewayError::InvalidRequest);
        }
        Ok(())
    }
}

pub struct SupervisorExecutor {
    writer: WriterHandle,
    config: SupervisorConfig,
    workspace: WorkspaceManager,
}
impl SupervisorExecutor {
    pub async fn new(
        writer: WriterHandle,
        config: SupervisorConfig,
    ) -> Result<Self, TaskGatewayError> {
        if config.worker.selection.provider_id == config.reviewer.selection.provider_id {
            return Err(TaskGatewayError::InvalidRequest);
        }
        let workspace = WorkspaceManager::new(
            config.git_executable.clone(),
            config.project_root.clone(),
            config.base_commit.clone(),
        )
        .await
        .map_err(|_| TaskGatewayError::Internal)?;
        Ok(Self {
            writer,
            config,
            workspace,
        })
    }
    async fn start_record(
        &self,
        task: Task,
        run: Run,
        peer: &ModelPeerProcess,
        external: &TaskSnapshot,
        workspace: RunWorkspace,
        binding: TaskBinding,
    ) -> Result<A2aRunRecord, TaskGatewayError> {
        let writer = self.writer.clone();
        let start = A2aRunStart {
            task,
            run,
            peer_id: peer.peer_id.clone(),
            external_task_id: external.task_id.clone(),
            transport: binding_name(binding).into(),
            protocol_version: "1.0".into(),
            workspace,
            timestamp: OffsetDateTime::now_utc(),
            idempotency_key: uuid::Uuid::new_v4().to_string(),
        };
        tokio::task::spawn_blocking(move || writer.start_a2a_run(start))
            .await
            .map_err(|_| TaskGatewayError::Internal)?
            .map(|o| o.record)
            .map_err(|_| TaskGatewayError::Internal)
    }
    async fn update(
        &self,
        record: &A2aRunRecord,
        action: A2aRunAction,
    ) -> Result<A2aRunRecord, TaskGatewayError> {
        let writer = self.writer.clone();
        let update = A2aRunUpdate {
            run_id: record.run.run_id(),
            expected_version: record.version,
            timestamp: OffsetDateTime::now_utc(),
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            action,
        };
        tokio::task::spawn_blocking(move || writer.update_a2a_run(update))
            .await
            .map_err(|_| TaskGatewayError::Internal)?
            .map(|o| o.record)
            .map_err(|_| TaskGatewayError::Internal)
    }
    async fn fail_open_runs(&self, task_id: TaskId) -> Result<(), TaskGatewayError> {
        let writer = self.writer.clone();
        let records = tokio::task::spawn_blocking(move || writer.a2a_runs(task_id))
            .await
            .map_err(|_| TaskGatewayError::Internal)?
            .map_err(|_| TaskGatewayError::Internal)?;
        for record in records {
            if matches!(
                record.run.status(),
                RunStatus::Preparing | RunStatus::Running | RunStatus::Stale
            ) {
                self.update(
                    &record,
                    A2aRunAction::Fail {
                        code: "supervisor_not_verified".into(),
                    },
                )
                .await?;
            }
        }
        Ok(())
    }
    async fn finalize_cancelled_task(&self, task_id: TaskId) -> Result<(), TaskGatewayError> {
        self.fail_open_runs(task_id).await?;
        let writer = self.writer.clone();
        let records = tokio::task::spawn_blocking(move || writer.a2a_runs(task_id))
            .await
            .map_err(|_| TaskGatewayError::Internal)?
            .map_err(|_| TaskGatewayError::Internal)?;
        if let Some(record) = records.first() {
            if record.task.status() != vibemux_types::TaskStatus::Cancelled {
                self.update(record, A2aRunAction::FinalizeCancellation)
                    .await?;
            }
        }
        Ok(())
    }
    async fn role_run(
        &self,
        task: &Task,
        peer: &ModelPeerProcess,
        submission: RoleSubmission,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<(A2aRunRecord, Value, ArtifactReference), TaskGatewayError> {
        let RoleSubmission {
            role,
            binding,
            payload,
            correlation,
        } = submission;
        let run = Run::new(RunSpec {
            project_id: task.project_id(),
            task_id: task.task_id(),
            harness: "a2a_model_peer".into(),
            role: role.into(),
            protocol: binding_name(binding).into(),
            base_commit: self.config.base_commit.clone(),
        })
        .map_err(|_| TaskGatewayError::Internal)?;
        let workspace = self
            .workspace
            .create(run.run_id())
            .await
            .map_err(|_| TaskGatewayError::Internal)?;
        let request = role_request(payload, &correlation);
        let submitted = match peer.client.send(&request).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = self.workspace.cleanup(&workspace).await;
                return Err(error);
            }
        };
        let mut record = match self
            .start_record(
                task.clone(),
                run,
                peer,
                &submitted,
                workspace.clone(),
                binding,
            )
            .await
        {
            Ok(record) => record,
            Err(error) => {
                let _ = peer.client.cancel(&submitted.task_id).await;
                let _ = self.workspace.cleanup(&workspace).await;
                return Err(error);
            }
        };
        record = self.update(&record, A2aRunAction::Start).await?;
        let deadline = Instant::now() + WORKFLOW_DEADLINE;
        let result = loop {
            if *cancellation.borrow() || Instant::now() >= deadline {
                record = self
                    .update(&record, A2aRunAction::RequestCancellation)
                    .await?;
                break peer.client.cancel(&submitted.task_id).await?;
            }
            let snapshot = peer.client.get(&submitted.task_id).await?;
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::select! {_=cancellation.changed()=>{},_=sleep(POLL_INTERVAL)=>{}}
        };
        if result.state == TaskState::Canceled {
            if record.binding.cancellation_state != A2aCancellationState::Requested {
                return Err(TaskGatewayError::Protocol);
            }
            record = self
                .update(
                    &record,
                    A2aRunAction::Observe {
                        state: A2aRemoteState::Cancelled,
                        artifacts: vec![],
                    },
                )
                .await?;
            self.update(&record, A2aRunAction::ConfirmCancellation)
                .await?;
            return Err(TaskGatewayError::Unsupported);
        }
        if result.state != TaskState::Completed {
            return Err(TaskGatewayError::Internal);
        }
        let data = artifact_data(&result)?;
        let name = if role == "worker" {
            "result.json"
        } else {
            "review.json"
        };
        let artifact = self
            .workspace
            .write_artifact(&record.workspace, name, &data)
            .await
            .map_err(|_| TaskGatewayError::Internal)?;
        record = self
            .update(
                &record,
                A2aRunAction::Observe {
                    state: A2aRemoteState::Completed,
                    artifacts: vec![artifact.clone()],
                },
            )
            .await?;
        Ok((record, data, artifact))
    }
    async fn run_workflow(
        &self,
        task: &Task,
        order: &SupervisorWorkOrder,
        correlation: &str,
        peers: &[ModelPeerProcess],
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<(A2aRunRecord, VerificationReceipt, Value), TaskGatewayError> {
        let planner_request = role_request(
            json!({"role":"planner","task_description":order.task_description,"input":order.input}),
            correlation,
        );
        let submitted = peers[0].client.send(&planner_request).await?;
        let planned = wait_remote(&peers[0], &submitted.task_id, &mut cancellation).await?;
        if planned.state != TaskState::Completed {
            return Err(TaskGatewayError::Internal);
        }
        let plan = artifact_data(&planned)?;
        let mut attempts = Vec::new();
        let mut feedback = Value::Null;
        for attempt in 0..=order.max_repairs {
            if *cancellation.borrow() {
                return Err(TaskGatewayError::Unsupported);
            }
            let(worker,candidate,artifact)=self.role_run(task,&peers[1],RoleSubmission{role:"worker",binding:self.config.worker.binding,payload:json!({"role":"worker","task_description":order.task_description,"input":order.input,"plan":plan["output"],"previous_feedback":feedback}),correlation:correlation.to_string()},cancellation.clone()).await?;
            let candidate_result = candidate
                .pointer("/output/result")
                .ok_or(TaskGatewayError::Protocol)?
                .clone();
            let(reviewer,review,review_artifact)=self.role_run(task,&peers[2],RoleSubmission{role:"reviewer",binding:self.config.reviewer.binding,payload:json!({"role":"reviewer","task_description":order.task_description,"input":order.input,"candidate_result":candidate_result}),correlation:correlation.to_string()},cancellation.clone()).await?;
            let review_approved = review
                .pointer("/output/approved")
                .and_then(Value::as_bool)
                .ok_or(TaskGatewayError::Protocol)?;
            let result_matches = portable_json(&candidate_result)
                && json_equivalent(&candidate_result, &order.expected_result);
            let accepted = review_approved && result_matches;
            let verification = json!({"schema_version":1,"accepted":accepted,"independent_review_approved":review_approved,"expected_result_matches":result_matches,"worker_artifact_sha256":artifact.sha256,"review_sha256":review_artifact.sha256,"expected_result_sha256":sha256(&serde_json::to_vec(&order.expected_result).map_err(|_|TaskGatewayError::Protocol)?)});
            let verification_artifact = self
                .workspace
                .write_artifact(&reviewer.workspace, "verification.json", &verification)
                .await
                .map_err(|_| TaskGatewayError::Internal)?;
            let receipt = VerificationReceipt {
                reviewer_run_id: reviewer.run.run_id(),
                reviewer_expected_version: reviewer.version,
                artifact_sha256: artifact.sha256.clone(),
                review_sha256: review_artifact.sha256.clone(),
                verification_sha256: verification_artifact.sha256.clone(),
                accepted,
            };
            attempts.push(json!({"attempt":attempt,"worker_run_id":worker.run.run_id(),"reviewer_run_id":reviewer.run.run_id(),"worker_peer_id":worker.binding.peer_id,"reviewer_peer_id":reviewer.binding.peer_id,"worker_artifact":artifact,"review_artifact":review_artifact,"verification_artifact":verification_artifact,"accepted":accepted,"worker_model":candidate["model"],"reviewer_model":review["model"]}));
            if accepted {
                return Ok((
                    worker,
                    receipt,
                    json!({"schema_version":1,"mode":"supervisor","canonical_task_id":task.task_id(),"planner_peer_id":peers[0].peer_id,"planner_model":plan["model"],"result":candidate_result,"attempts":attempts,"verified":true}),
                ));
            }
            self.update(&worker, A2aRunAction::Verify { receipt })
                .await?;
            feedback = json!({"review":review["output"],"verification":verification});
        }
        Err(TaskGatewayError::Protocol)
    }
}
#[async_trait]
impl TaskExecutor for SupervisorExecutor {
    async fn execute(
        &self,
        _subject: &str,
        request: TaskRequest,
        cancellation: watch::Receiver<bool>,
    ) -> Result<TaskExecution, TaskGatewayError> {
        let order: SupervisorWorkOrder = serde_json::from_value(request.payload)
            .map_err(|_| TaskGatewayError::InvalidRequest)?;
        order.validate()?;
        let task = Task::new(TaskSpec {
            project_id: self.config.project_id,
            title: "A2A supervisor workflow".into(),
            description: String::new(),
        })
        .map_err(|_| TaskGatewayError::Internal)?;
        let mut peers = Vec::new();
        for (role, config) in [
            ("planner", &self.config.planner),
            ("worker", &self.config.worker),
            ("reviewer", &self.config.reviewer),
        ] {
            match ModelPeerProcess::start(
                &self.config.model_peer_executable,
                &self.config.cc_switch_database,
                role,
                config,
            )
            .await
            {
                Ok(peer) => peers.push(peer),
                Err(error) => {
                    for peer in peers {
                        let _ = peer.shutdown().await;
                    }
                    return Err(error);
                }
            }
        }
        let outcome = self
            .run_workflow(
                &task,
                &order,
                &request.context_id,
                &peers,
                cancellation.clone(),
            )
            .await;
        let mut cleanup_error = None;
        for peer in peers {
            if let Err(error) = peer.shutdown().await {
                cleanup_error.get_or_insert(error);
            }
        }
        if *cancellation.borrow() {
            self.finalize_cancelled_task(task.task_id()).await?;
            return Ok(TaskExecution::Canceled);
        }
        if let Some(error) = cleanup_error {
            self.fail_open_runs(task.task_id()).await?;
            return Err(error);
        }
        match outcome {
            Ok((worker, receipt, mut report)) => {
                let verified = self
                    .update(
                        &worker,
                        A2aRunAction::Verify {
                            receipt: receipt.clone(),
                        },
                    )
                    .await?;
                report["canonical_task_status"] = json!(verified.task.status());
                report["verification_receipt"] =
                    serde_json::to_value(receipt).map_err(|_| TaskGatewayError::Internal)?;
                Ok(TaskExecution::Completed(vec![TaskArtifact::data(
                    uuid::Uuid::new_v4().to_string(),
                    report,
                )]))
            }
            Err(error) => {
                self.fail_open_runs(task.task_id()).await?;
                Err(error)
            }
        }
    }
}
fn role_request(payload: Value, correlation: &str) -> TaskRequest {
    TaskRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        context_id: correlation.to_string(),
        task_id: None,
        idempotency_key: uuid::Uuid::new_v4().to_string(),
        payload,
    }
}
fn binding_name(binding: TaskBinding) -> &'static str {
    match binding {
        TaskBinding::HttpJson => "http_json",
        TaskBinding::JsonRpc => "json_rpc",
    }
}
fn artifact_data(snapshot: &TaskSnapshot) -> Result<Value, TaskGatewayError> {
    if snapshot.artifacts.len() != 1 || snapshot.artifacts[0].parts.len() != 1 {
        return Err(TaskGatewayError::Protocol);
    }
    match &snapshot.artifacts[0].parts[0] {
        TaskPart::Data(value) => Ok(value.clone()),
        _ => Err(TaskGatewayError::Protocol),
    }
}
async fn wait_remote(
    peer: &ModelPeerProcess,
    task_id: &str,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<TaskSnapshot, TaskGatewayError> {
    if *cancellation.borrow() {
        return peer.client.cancel(task_id).await;
    }
    // Fast path: the SSE event stream eliminates polling entirely. The
    // gateway rejects subscriptions for already-terminal tasks, and streams
    // can end on transport errors, so any failure degrades to bounded
    // polling below without changing the wait contract.
    if let Ok(mut events) = peer.client.subscribe(task_id).await {
        let deadline = Instant::now() + WORKFLOW_DEADLINE;
        let mut cancelled = false;
        loop {
            tokio::select! {
                biased;
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow_and_update() {
                        cancelled = true;
                        break;
                    }
                }
                event = events.next() => match event {
                    Some(Ok(snapshot)) => {
                        if snapshot.state.is_terminal() {
                            return Ok(snapshot);
                        }
                    }
                    // None: the stream ended without a terminal event.
                    Some(Err(_)) | None => break,
                },
                _ = tokio::time::sleep_until(deadline) => {
                    let _ = peer.client.cancel(task_id).await;
                    return Err(TaskGatewayError::Deadline);
                }
            }
        }
        if cancelled {
            return peer.client.cancel(task_id).await;
        }
    }
    let deadline = Instant::now() + WORKFLOW_DEADLINE;
    loop {
        if *cancellation.borrow() {
            return peer.client.cancel(task_id).await;
        }
        let snapshot = peer.client.get(task_id).await?;
        if snapshot.state.is_terminal() {
            return Ok(snapshot);
        }
        if Instant::now() >= deadline {
            let _ = peer.client.cancel(task_id).await;
            return Err(TaskGatewayError::Deadline);
        }
        tokio::select! {_=cancellation.changed()=>{},_=sleep(POLL_INTERVAL)=>{}}
    }
}

fn deserialize_u32_number<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u32, D::Error> {
    let value = Value::deserialize(deserializer)?;
    if let Some(integer) = value.as_u64() {
        return u32::try_from(integer).map_err(serde::de::Error::custom);
    }
    let number = value
        .as_f64()
        .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n <= f64::from(u32::MAX))
        .ok_or_else(|| serde::de::Error::custom("expected exact unsigned integer"))?;
    format!("{number:.0}")
        .parse::<u32>()
        .map_err(serde::de::Error::custom)
}
fn portable_json(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.as_f64().is_some_and(|n| {
            n.is_finite() && (n.fract() != 0.0 || n.abs() <= 9_007_199_254_740_991.0)
        }),
        Value::Array(values) => values.iter().all(portable_json),
        Value::Object(values) => values.values().all(portable_json),
        _ => true,
    }
}
fn json_equivalent(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| json_equivalent(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| json_equivalent(a, b)))
        }
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protobuf_json_integral_numbers_preserve_work_order_validation() {
        let order:SupervisorWorkOrder=serde_json::from_str(r#"{"schema_version":1.0,"task_description":"sort","input":[1.0],"expected_result":[1.0],"max_repairs":0.0}"#).expect("A2A protobuf Struct numbers");
        order.validate().expect("valid");
        assert!(json_equivalent(
            &json!({"n":[1,2]}),
            &json!({"n":[1.0,2.0]})
        ));
        assert!(!json_equivalent(&json!(1), &json!(1.5)));
        assert!(!portable_json(&json!(9_007_199_254_740_992_u64)));
        assert!(serde_json::from_str::<SupervisorWorkOrder>(r#"{"schema_version":1.5,"task_description":"sort","input":[],"expected_result":[]}"#).is_err());
    }
}
