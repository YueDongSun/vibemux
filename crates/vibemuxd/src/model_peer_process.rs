//! Process ownership for explicitly configured out-of-process A2A model peers.
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    task::JoinHandle,
    time::timeout,
};
use vibemux_a2a::{
    task_client::{TaskClient, TaskClientConfig},
    task_contract::{PeerCredential, TaskBinding, TaskGatewayError},
};
use vibemux_model_peer::provider::ProviderSelection;

const PEER_START_DEADLINE: Duration = Duration::from_secs(10);
const PEER_STOP_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPeerConfig {
    pub selection: ProviderSelection,
    pub binding: TaskBinding,
}
#[derive(Deserialize)]
struct Ready {
    ready: bool,
    peer_id: String,
    base_url: String,
    process_id: u32,
}
pub struct ModelPeerProcess {
    pub peer_id: String,
    pub client: TaskClient,
    pub process_id: u32,
    child: Child,
    input: ChildStdin,
    diagnostics: Option<JoinHandle<()>>,
}

impl ModelPeerProcess {
    pub async fn start(
        executable: &Path,
        database: &Path,
        role: &str,
        config: &ModelPeerConfig,
    ) -> Result<Self, TaskGatewayError> {
        if !executable.is_absolute()
            || !database.is_absolute()
            || !matches!(role, "planner" | "worker" | "reviewer")
        {
            return Err(TaskGatewayError::InvalidRequest);
        }
        let peer_id = format!("{}_{}", role, uuid::Uuid::new_v4().simple());
        let token =
            uuid::Uuid::new_v4().simple().to_string() + &uuid::Uuid::new_v4().simple().to_string();
        let mut command = Command::new(executable);
        command
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command.spawn().map_err(|_| TaskGatewayError::Server)?;
        let mut input = child.stdin.take().ok_or(TaskGatewayError::Server)?;
        let stdout = child.stdout.take().ok_or(TaskGatewayError::Server)?;
        let mut stderr = child.stderr.take().ok_or(TaskGatewayError::Server)?;
        let diagnostics = tokio::spawn(async move {
            let mut buffer = [0; 4096];
            loop {
                match stderr.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
        let request = json!({"mode":"serve","cc_switch_database":database,"selection":config.selection,"peer_id":peer_id,"bearer_token":token});
        let bootstrap = async {
            let mut bytes =
                serde_json::to_vec(&request).map_err(|_| TaskGatewayError::InvalidRequest)?;
            bytes.push(b'\n');
            input
                .write_all(&bytes)
                .await
                .map_err(|_| TaskGatewayError::Server)?;
            input.flush().await.map_err(|_| TaskGatewayError::Server)?;
            let mut line = String::new();
            BufReader::new(stdout.take(4097))
                .read_line(&mut line)
                .await
                .map_err(|_| TaskGatewayError::Server)?;
            if line.len() > 4096 {
                return Err(TaskGatewayError::Protocol);
            }
            let ready: Ready =
                serde_json::from_str(&line).map_err(|_| TaskGatewayError::Protocol)?;
            if !ready.ready || ready.peer_id != peer_id || Some(ready.process_id) != child.id() {
                return Err(TaskGatewayError::Protocol);
            }
            let client = TaskClient::connect(
                &ready.base_url,
                TaskClientConfig {
                    allowed_origins: vec![ready.base_url.clone()],
                    credential: PeerCredential {
                        subject: "supervisor".into(),
                        bearer_token: token,
                    },
                    preferred_bindings: vec![config.binding],
                },
            )
            .await?;
            Ok((client, ready.process_id))
        };
        match timeout(PEER_START_DEADLINE, bootstrap).await {
            Ok(Ok((client, process_id))) => Ok(Self {
                peer_id,
                client,
                process_id,
                child,
                input,
                diagnostics: Some(diagnostics),
            }),
            result => {
                let _ = child.start_kill();
                let _ = timeout(PEER_STOP_DEADLINE, child.wait()).await;
                diagnostics.abort();
                let _ = diagnostics.await;
                Err(match result {
                    Ok(Err(e)) => e,
                    _ => TaskGatewayError::Deadline,
                })
            }
        }
    }
    pub async fn shutdown(mut self) -> Result<(), TaskGatewayError> {
        let _ = self.input.write_all(b"shutdown\n").await;
        let _ = self.input.flush().await;
        let result = match timeout(PEER_STOP_DEADLINE, self.child.wait()).await {
            Ok(Ok(status)) if status.success() => Ok(()),
            _ => {
                let _ = self.child.start_kill();
                match timeout(PEER_STOP_DEADLINE, self.child.wait()).await {
                    Ok(Ok(_)) => Err(TaskGatewayError::Deadline),
                    _ => Err(TaskGatewayError::Server),
                }
            }
        };
        if let Some(diagnostics) = self.diagnostics.take() {
            diagnostics.abort();
            let _ = diagnostics.await;
        }
        result
    }
}
impl Drop for ModelPeerProcess {
    fn drop(&mut self) {
        if let Some(task) = self.diagnostics.take() {
            task.abort();
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorConfig {
    pub project_id: vibemux_types::ProjectId,
    pub project_root: PathBuf,
    pub git_executable: PathBuf,
    pub base_commit: String,
    pub model_peer_executable: PathBuf,
    pub cc_switch_database: PathBuf,
    pub planner: ModelPeerConfig,
    pub worker: ModelPeerConfig,
    pub reviewer: ModelPeerConfig,
}
