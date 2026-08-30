//! One-shot local supervisor acceptance runner using the actual daemon/A2A path.
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use tokio::time::{Instant, sleep};
use vibemux_a2a::{
    task_client::{TaskClient, TaskClientConfig},
    task_contract::{PeerCredential, TaskBinding, TaskRequest, TaskState},
};
use vibemuxd::{
    control::DaemonControlServer, plugin_configuration::PluginStartup, process::DaemonPaths,
    supervisor_service::SupervisorServiceConfig, supervisor_workflow::SupervisorWorkOrder,
};

#[tokio::main]
async fn main() {
    if let Err(code) = run().await {
        eprintln!("{{\"ok\":false,\"error_code\":\"{code}\"}}");
        std::process::exit(4);
    }
}
async fn read_json<T: serde::de::DeserializeOwned + Send + 'static>(
    path: PathBuf,
) -> Result<T, String> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let file =
            std::fs::File::open(path).map_err(|_| "supervisor_input_unavailable".to_string())?;
        let mut bytes = Vec::new();
        file.take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| "supervisor_input_unavailable".to_string())?;
        if bytes.len() > 65536 {
            return Err("supervisor_input_too_large".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "supervisor_invalid_input".into())
    })
    .await
    .map_err(|_| "supervisor_input_failed".to_string())?
}
async fn run() -> Result<(), String> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 4 || args[0] != "--config" || args[2] != "--job" {
        return Err("supervisor_invalid_arguments".into());
    }
    let config: SupervisorServiceConfig = read_json(PathBuf::from(&args[1])).await?;
    let job: SupervisorWorkOrder = read_json(PathBuf::from(&args[3])).await?;
    job.validate().map_err(|e| e.code().to_string())?;
    let paths = DaemonPaths::from_project_root(&config.supervisor.project_root)
        .map_err(|e| e.code().to_string())?;
    paths
        .ensure_runtime_dir()
        .map_err(|e| e.code().to_string())?;
    let credential = PeerCredential {
        subject: "operator".into(),
        bearer_token: config.bearer_token.clone(),
    };
    let server = DaemonControlServer::start_for_paths_with_supervisor(
        &paths,
        PluginStartup::default(),
        config,
    )
    .await
    .map_err(|e| e.code().to_string())?;
    let outcome = async {
        let base = server
            .a2a_base_url()
            .ok_or_else(|| "supervisor_gateway_unavailable".to_string())?;
        let client = TaskClient::connect(
            base,
            TaskClientConfig {
                allowed_origins: vec![
                    base.to_string(),
                    server
                        .a2a_grpc_base_url()
                        .ok_or_else(|| "supervisor_grpc_unavailable".to_string())?
                        .to_string(),
                ],
                credential,
                preferred_bindings: vec![TaskBinding::HttpJson],
            },
        )
        .await
        .map_err(|e| e.code().to_string())?;
        let request = TaskRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            context_id: uuid::Uuid::new_v4().to_string(),
            task_id: None,
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            payload: serde_json::to_value(job)
                .map_err(|_| "supervisor_encoding_failed".to_string())?,
        };
        let mut snapshot = client
            .send(&request)
            .await
            .map_err(|e| e.code().to_string())?;
        let deadline = Instant::now() + Duration::from_secs(600);
        while !snapshot.state.is_terminal() {
            if Instant::now() >= deadline {
                let _ = client.cancel(&snapshot.task_id).await;
                return Err("supervisor_acceptance_deadline".to_string());
            }
            sleep(Duration::from_millis(100)).await;
            snapshot = client
                .get(&snapshot.task_id)
                .await
                .map_err(|e| e.code().to_string())?;
        }
        Ok::<_, String>(snapshot)
    }
    .await;
    let shutdown = server.shutdown().await.map_err(|e| e.code().to_string());
    match outcome {
        Ok(snapshot) => {
            let success = snapshot.state == TaskState::Completed;
            println!(
                "{}",
                json!({"schema_version":1,"ok":success,"daemon_shutdown_joined":shutdown.is_ok(),"task":snapshot})
            );
            shutdown?;
            if !success {
                return Err("supervisor_verification_failed".into());
            }
            Ok(())
        }
        Err(error) => {
            let _ = shutdown;
            Err(error)
        }
    }
}
