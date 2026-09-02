//! Bounded interoperability probe for an independently launched SDK reference peer.
use serde::Deserialize;
use serde_json::json;
use vibemux_a2a::{
    GrpcTaskClient, PeerCredential, TaskBinding, TaskClient, TaskClientConfig, TaskReply,
    TaskRequest, TaskState,
};
const FIXTURE_TOKEN: &str = "vibemux_tck_fixture_token_01234567890123456789";
#[derive(Deserialize)]
struct Reference {
    http_url: String,
    grpc_url: String,
}
fn credential() -> PeerCredential {
    PeerCredential {
        subject: "tck_fixture".into(),
        bearer_token: FIXTURE_TOKEN.into(),
    }
}
fn request(label: &str) -> TaskRequest {
    TaskRequest {
        request_id: format!("interop_{label}"),
        context_id: format!("interop_context_{label}"),
        task_id: None,
        idempotency_key: format!("interop_{label}"),
        payload: json!({"independent_sdk_probe":true}),
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let reference: Reference = serde_json::from_str(&line)?;
    let mut checks = Vec::new();
    for (label, binding) in [
        ("http_json", TaskBinding::HttpJson),
        ("jsonrpc", TaskBinding::JsonRpc),
    ] {
        let client = TaskClient::connect(
            &reference.http_url,
            TaskClientConfig {
                allowed_origins: vec![reference.http_url.clone(), reference.grpc_url.clone()],
                credential: credential(),
                preferred_bindings: vec![binding],
            },
        )
        .await
        .map_err(|error| format!("{label} discovery: {}", error.code()))?;
        let task = client
            .send(&request(label))
            .await
            .map_err(|error| format!("{label} send: {}", error.code()))?;
        let fetched = client
            .get(&task.task_id)
            .await
            .map_err(|error| format!("get: {}", error.code()))?;
        if fetched.task_id != task.task_id || fetched.context_id != task.context_id {
            return Err("identity mismatch".into());
        }
        let canceled = client
            .cancel(&task.task_id)
            .await
            .map_err(|error| format!("cancel: {}", error.code()))?;
        if canceled.state != TaskState::Canceled {
            return Err("cancel incomplete".into());
        }
        checks.push(json!({"binding":label,"discovery":true,"send_get_identity":true,"cancel_terminal":true}));
    }
    let client = GrpcTaskClient::connect(&reference.grpc_url, credential()).await?;
    let TaskReply::Task(task) = client.send(&request("grpc")).await? else {
        return Err("expected task".into());
    };
    let fetched = client
        .get(&task.task_id)
        .await
        .map_err(|error| format!("get: {}", error.code()))?;
    if fetched.task_id != task.task_id || fetched.context_id != task.context_id {
        return Err("identity mismatch".into());
    }
    if client.cancel(&task.task_id).await?.state != TaskState::Canceled {
        return Err("cancel incomplete".into());
    }
    checks.push(json!({"binding":"grpc","send_get_identity":true,"cancel_terminal":true}));
    println!(
        "{}",
        json!({"direction":"vibemux_to_independent_sdk","checks":checks})
    );
    Ok(())
}
