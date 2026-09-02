//! Daemon-owned A2A supervisor service; all canonical writes use its WriterHandle.
use crate::{
    WriterHandle, model_peer_process::SupervisorConfig, supervisor_workflow::SupervisorExecutor,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use vibemux_a2a::{
    task_contract::{PeerCredential, TaskGatewayError},
    task_grpc::GrpcTaskServer,
    task_runtime::TaskRuntime,
    task_server::{TaskServer, TaskServerConfig},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorServiceConfig {
    pub supervisor: SupervisorConfig,
    pub bearer_token: String,
}
pub struct SupervisorService {
    server: TaskServer,
    grpc_server: GrpcTaskServer,
    runtime: TaskRuntime,
}
impl SupervisorService {
    pub async fn start(
        writer: WriterHandle,
        config: SupervisorServiceConfig,
    ) -> Result<Self, TaskGatewayError> {
        let credential = PeerCredential {
            subject: "operator".into(),
            bearer_token: config.bearer_token,
        };
        credential.validate()?;
        let executor = SupervisorExecutor::new(writer, config.supervisor).await?;
        let runtime = TaskRuntime::start(Arc::new(executor), 1)?;
        let server_config = TaskServerConfig {
            peer_id: "vibemux_supervisor".into(),
            credentials: vec![credential],
        };
        let grpc_server =
            match GrpcTaskServer::start(server_config.clone(), runtime.backend()).await {
                Ok(server) => server,
                Err(error) => {
                    let _ = runtime.shutdown().await;
                    return Err(error);
                }
            };
        match TaskServer::start_with_grpc(server_config, runtime.backend(), grpc_server.base_url())
            .await
        {
            Ok(server) => Ok(Self {
                server,
                grpc_server,
                runtime,
            }),
            Err(error) => {
                let _ = grpc_server.shutdown().await;
                let _ = runtime.shutdown().await;
                Err(error)
            }
        }
    }
    pub fn base_url(&self) -> &str {
        self.server.base_url()
    }
    pub fn grpc_base_url(&self) -> String {
        self.grpc_server.base_url()
    }
    pub async fn shutdown(self) -> Result<(), TaskGatewayError> {
        // Close both acceptance/stream surfaces before cancelling executor work.
        // Each transport owns its join path; errors do not skip later cleanup.
        let (server, grpc_server) =
            tokio::join!(self.server.shutdown(), self.grpc_server.shutdown());
        let runtime = self.runtime.shutdown().await;
        server.and(grpc_server).and(runtime)
    }
}

impl SupervisorServiceConfig {
    pub fn from_path(path: &std::path::Path) -> Result<Self, &'static str> {
        use std::io::Read;
        let metadata =
            std::fs::symlink_metadata(path).map_err(|_| "supervisor_configuration_unavailable")?;
        if !metadata.file_type().is_file() || metadata.len() > 65536 {
            return Err("supervisor_configuration_invalid");
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| "supervisor_configuration_unavailable")?
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| "supervisor_configuration_unavailable")?;
        if bytes.len() > 65536 {
            return Err("supervisor_configuration_invalid");
        }
        serde_json::from_slice(&bytes).map_err(|_| "supervisor_configuration_invalid")
    }
}
