use serde::Deserialize;
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use vibemux_a2a::{
    task_contract::PeerCredential,
    task_runtime::TaskRuntime,
    task_server::{TaskServer, TaskServerConfig},
};
use vibemux_model_peer::provider::{ProviderSelection, inventory, load_provider};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    mode: String,
    cc_switch_database: PathBuf,
    #[serde(default)]
    selection: Option<ProviderSelection>,
    #[serde(default)]
    peer_id: Option<String>,
    #[serde(default)]
    bearer_token: Option<String>,
}

#[tokio::main]
async fn main() {
    if let Err(code) = run().await {
        eprintln!("{{\"ok\":false,\"error_code\":\"{code}\"}}");
        std::process::exit(4);
    }
}
async fn run() -> Result<(), String> {
    let mut line = String::new();
    let mut input = BufReader::new(tokio::io::stdin().take(32 * 1024 + 1));
    input
        .read_line(&mut line)
        .await
        .map_err(|_| "peer_input_failed".to_string())?;
    if line.len() > 16 * 1024 {
        return Err("peer_input_too_large".to_string());
    }
    let bootstrap: Bootstrap =
        serde_json::from_str(&line).map_err(|_| "peer_invalid_bootstrap".to_string())?;
    match bootstrap.mode.as_str() {
        "inventory" => {
            let items =
                tokio::task::spawn_blocking(move || inventory(&bootstrap.cc_switch_database))
                    .await
                    .map_err(|_| "peer_worker_failed".to_string())?
                    .map_err(|e| e.code().to_string())?;
            println!(
                "{}",
                serde_json::to_string(&items).map_err(|_| "peer_encoding_failed".to_string())?
            );
        }
        "probe" => {
            let selection = bootstrap
                .selection
                .ok_or_else(|| "peer_provider_required".to_string())?;
            let provider = tokio::task::spawn_blocking(move || {
                load_provider(&bootstrap.cc_switch_database, &selection)
            })
            .await
            .map_err(|_| "peer_worker_failed".to_string())?
            .map_err(|e| e.code().to_string())?;
            match provider
                .complete_json(
                    "Return only valid JSON. Do not include reasoning or markdown.",
                    "Return exactly {\"ok\":true}.",
                    1024,
                )
                .await
            {
                Ok(result) => println!(
                    "{}",
                    json!({"ok":result.value==json!({"ok":true}),"provider":provider.label,"configured_model":provider.configured_model,"wire_model":result.model,"elapsed_ms":result.elapsed_ms,"input_tokens":result.input_tokens,"output_tokens":result.output_tokens,"response_sha256":result.response_sha256})
                ),
                Err(error) => println!(
                    "{}",
                    json!({"ok":false,"provider":provider.label,"configured_model":provider.configured_model,"wire_model":provider.wire_model,"error_code":error.code(),"http_status":match error {vibemux_model_peer::provider::ProviderError::Http(status)=>Some(status),_=>None}})
                ),
            }
        }
        "serve" => {
            let selection = bootstrap
                .selection
                .ok_or_else(|| "peer_provider_required".to_string())?;
            let provider = tokio::task::spawn_blocking(move || {
                load_provider(&bootstrap.cc_switch_database, &selection)
            })
            .await
            .map_err(|_| "peer_worker_failed".to_string())?
            .map_err(|e| e.code().to_string())?;
            let peer_id = bootstrap
                .peer_id
                .ok_or_else(|| "peer_identity_required".to_string())?;
            let bearer_token = bootstrap
                .bearer_token
                .ok_or_else(|| "peer_auth_required".to_string())?;
            let runtime = TaskRuntime::start(
                Arc::new(vibemux_model_peer::executor::ModelExecutor {
                    provider: Arc::new(provider),
                }),
                2,
            )
            .map_err(|e| e.code().to_string())?;
            let server = TaskServer::start(
                TaskServerConfig {
                    peer_id: peer_id.clone(),
                    credentials: vec![PeerCredential {
                        subject: "supervisor".to_string(),
                        bearer_token,
                    }],
                },
                runtime.backend(),
            )
            .await
            .map_err(|e| e.code().to_string())?;
            println!(
                "{}",
                json!({"ready":true,"peer_id":peer_id,"base_url":server.base_url(),"process_id":std::process::id()})
            );
            let mut stop_line = String::new();
            let _ = input.read_line(&mut stop_line).await;
            // EOF or any owner stop record closes acceptance, then joins all model requests.
            let server_result = server.shutdown().await;
            let runtime_result = runtime.shutdown().await;
            server_result.map_err(|e| e.code().to_string())?;
            runtime_result.map_err(|e| e.code().to_string())?;
        }
        _ => return Err("peer_unknown_mode".to_string()),
    }
    Ok(())
}
