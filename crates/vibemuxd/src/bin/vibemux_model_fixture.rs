//! Test-only deterministic child implementing the real model-peer bootstrap.
//! This binary is feature-gated and never reads provider databases or API keys.
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    sync::watch,
};
use vibemux_a2a::{
    PeerCredential, TaskArtifact, TaskGatewayError, TaskRequest,
    task_runtime::{TaskExecution, TaskExecutor, TaskRuntime},
    task_server::{TaskServer, TaskServerConfig},
};
use vibemux_model_peer::provider::ProviderSelection;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    mode: String,
    cc_switch_database: PathBuf,
    selection: ProviderSelection,
    peer_id: String,
    bearer_token: String,
}
struct FixtureExecutor {
    role: String,
    mode: String,
    receipts: PathBuf,
    peer_id: String,
    calls: AtomicUsize,
    active: Arc<AtomicUsize>,
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait]
impl TaskExecutor for FixtureExecutor {
    async fn execute(
        &self,
        _subject: &str,
        request: TaskRequest,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<TaskExecution, TaskGatewayError> {
        let role = request
            .payload
            .get("role")
            .and_then(Value::as_str)
            .ok_or(TaskGatewayError::InvalidRequest)?;
        if role != self.role {
            return Err(TaskGatewayError::InvalidRequest);
        }
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(self.active.clone());
        receipt(
            &self.receipts,
            format!("{}_call_{call}.json", self.peer_id),
            json!({"peer_id":self.peer_id,"role":role,"call":call,"process_id":std::process::id()}),
        )
        .await?;
        if self.mode == "hang" {
            if !*cancellation.borrow() {
                cancellation
                    .changed()
                    .await
                    .map_err(|_| TaskGatewayError::Server)?;
            }
            return Ok(TaskExecution::Canceled);
        }
        let output = match role {
            "planner" => {
                json!({"steps":["Sort the supplied numeric values ascending","Independently recompute and verify the candidate"]})
            }
            "worker" if self.mode == "repair_once" && call == 1 => {
                json!({"result":{"sorted":[999]}})
            }
            "worker" => json!({"result":sorted_result(&request.payload)?}),
            "reviewer" => {
                let expected = sorted_result(&request.payload)?;
                json!({"approved":request.payload.get("candidate_result")==Some(&expected),"reason":"Deterministic independent recomputation"})
            }
            _ => return Err(TaskGatewayError::InvalidRequest),
        };
        let encoded = serde_json::to_vec(&output).map_err(|_| TaskGatewayError::Protocol)?;
        let artifact = TaskArtifact::data(
            uuid::Uuid::new_v4().to_string(),
            json!({"role":role,"output":output,"model":format!("fixture_{}",self.mode),"elapsed_ms":0,"input_tokens":0,"output_tokens":0,"response_sha256":vibemux_workspace::sha256(&encoded)}),
        );
        artifact.validate()?;
        Ok(TaskExecution::Completed(vec![artifact]))
    }
}
fn sorted_result(payload: &Value) -> Result<Value, TaskGatewayError> {
    let values = payload
        .pointer("/input/values")
        .and_then(Value::as_array)
        .ok_or(TaskGatewayError::InvalidRequest)?;
    if values.is_empty() || values.len() > 64 {
        return Err(TaskGatewayError::InvalidRequest);
    }
    let mut numbers = values
        .iter()
        .map(|value| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or(TaskGatewayError::InvalidRequest)
        })
        .collect::<Result<Vec<_>, _>>()?;
    numbers.sort_by(f64::total_cmp);
    Ok(json!({"sorted":numbers}))
}
async fn receipt(directory: &Path, name: String, value: Value) -> Result<(), TaskGatewayError> {
    let path = directory.join(name);
    tokio::task::spawn_blocking(move || {
        let bytes = serde_json::to_vec(&value).map_err(|_| TaskGatewayError::Internal)?;
        // Publish atomically: the workflow tests poll the receipts
        // directory every 20ms, so a direct write to the final path can
        // be observed as a zero-byte file (JSON EOF) on slow runners.
        // Write to a sibling temp name, sync, then rename into place —
        // rename within a directory is atomic on both POSIX and Windows,
        // and it keeps the create_new semantics of failing when the
        // receipt already exists (Windows rename refuses to replace).
        let temp = path.with_extension(format!(
            "{}.tmp-{}",
            path.extension()
                .map(|ext| ext.to_string_lossy().into_owned())
                .unwrap_or_default(),
            std::process::id()
        ));
        let mut file = std::fs::File::create(&temp).map_err(|_| TaskGatewayError::Internal)?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| TaskGatewayError::Internal)?;
        std::fs::rename(&temp, &path).map_err(|_| TaskGatewayError::Internal)
    })
    .await
    .map_err(|_| TaskGatewayError::Internal)?
}
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{{\"ok\":false,\"error_code\":\"{}\"}}", error.code());
        std::process::exit(4);
    }
}
async fn run() -> Result<(), TaskGatewayError> {
    let mut input = BufReader::new(tokio::io::stdin().take(32 * 1024 + 1));
    let mut line = String::new();
    input
        .read_line(&mut line)
        .await
        .map_err(|_| TaskGatewayError::Server)?;
    if line.len() > 16 * 1024 {
        return Err(TaskGatewayError::InvalidRequest);
    }
    let bootstrap: Bootstrap =
        serde_json::from_str(&line).map_err(|_| TaskGatewayError::InvalidRequest)?;
    if bootstrap.mode != "serve"
        || !bootstrap.cc_switch_database.is_absolute()
        || !bootstrap.cc_switch_database.is_dir()
    {
        return Err(TaskGatewayError::InvalidRequest);
    }
    let role = bootstrap
        .peer_id
        .split('_')
        .next()
        .ok_or(TaskGatewayError::InvalidRequest)?
        .to_string();
    if !matches!(role.as_str(), "planner" | "worker" | "reviewer") {
        return Err(TaskGatewayError::InvalidRequest);
    }
    let mode =
        if role == "worker" || (role == "reviewer" && bootstrap.selection.provider_id == "hang") {
            bootstrap.selection.provider_id.clone()
        } else {
            "valid".to_string()
        };
    if !matches!(mode.as_str(), "valid" | "repair_once" | "hang") {
        return Err(TaskGatewayError::InvalidRequest);
    }
    let active = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(FixtureExecutor {
        role: role.clone(),
        mode,
        receipts: bootstrap.cc_switch_database.clone(),
        peer_id: bootstrap.peer_id.clone(),
        calls: AtomicUsize::new(0),
        active: active.clone(),
    });
    let runtime = TaskRuntime::start(executor.clone(), 2)?;
    let server = TaskServer::start(
        TaskServerConfig {
            peer_id: bootstrap.peer_id.clone(),
            credentials: vec![PeerCredential {
                subject: "supervisor".to_string(),
                bearer_token: bootstrap.bearer_token,
            }],
        },
        runtime.backend(),
    )
    .await?;
    receipt(&bootstrap.cc_switch_database,format!("{}_ready.json",bootstrap.peer_id),json!({"peer_id":bootstrap.peer_id,"role":role,"process_id":std::process::id(),"ready":true})).await?;
    println!(
        "{}",
        json!({"ready":true,"peer_id":bootstrap.peer_id,"base_url":server.base_url(),"process_id":std::process::id()})
    );
    let mut stop = String::new();
    let _ = input.read_line(&mut stop).await;
    let server_result = server.shutdown().await;
    let runtime_result = runtime.shutdown().await;
    server_result?;
    runtime_result?;
    receipt(&bootstrap.cc_switch_database,format!("{}_shutdown.json",bootstrap.peer_id),json!({"peer_id":bootstrap.peer_id,"role":role,"process_id":std::process::id(),"shutdown_joined":true,"active_calls":active.load(Ordering::SeqCst),"call_count":executor.calls.load(Ordering::SeqCst)})).await?;
    Ok(())
}
