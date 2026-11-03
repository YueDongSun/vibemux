//! Strict per-command request bodies. Unknown renderer fields fail closed.
use crate::{
    command_error::CommandError, desktop_settings::SettingsPatch, ui_contract::AskHarness,
};
use serde::{Deserialize, de::DeserializeOwned};
use tauri::ipc::{InvokeBody, Request};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyArgs {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectArgs {
    pub project_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskArgs {
    pub ask_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskStatusArgs {
    pub ask_id: String,
    #[serde(default)]
    pub reconcile: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitArgs {
    pub project_key: String,
    pub harness: AskHarness,
    pub prompt: String,
    pub submission_key: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsArgs {
    pub patch: SettingsPatch,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TasksArgs {
    pub project_key: String,
    pub query: crate::read_contract::TaskReadQuery,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWindowArgs {
    pub project_key: String,
    pub task_id: vibemux_types::TaskId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorArgs {
    pub project_key: String,
    pub task_id: vibemux_types::TaskId,
    pub run_id: vibemux_types::RunId,
    pub request: crate::terminal_bridge::TerminalRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkArgs {
    pub request: crate::link_contract::OpenLinkRequest,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanupArgs {
    pub request: crate::workspace_cleanup_bridge::NativeCleanupRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleArgs {
    pub project_key: String,
    pub query: crate::read_contract::ConsoleQuery,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeArgs {
    pub project_key: String,
    pub request: crate::probe_contract::ProbeRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatArgs {
    pub project_key: String,
    pub request: vibemuxd::control::ChatRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatOwnerArgs {
    pub project_key: String,
    pub request: vibemuxd::control::ChatOwnerRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatExecutionArgs {
    pub project_key: String,
    pub request: vibemuxd::control::ChatExecutionRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatApprovalArgs {
    pub project_key: String,
    pub request: vibemuxd::control::ChatApprovalRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatExportArgs {
    pub project_key: String,
    pub request: vibemux_client::chat_export::ExportRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatSearchArgs {
    pub project_key: String,
    pub request: vibemux_client::chat_search::SearchRequest,
}

pub fn parse<T: DeserializeOwned>(request: Request<'_>) -> Result<T, CommandError> {
    parse_body(request.body())
}
pub fn parse_body<T: DeserializeOwned>(body: &InvokeBody) -> Result<T, CommandError> {
    let InvokeBody::Json(value) = body else {
        return Err(invalid());
    };
    serde_json::from_value(value.clone()).map_err(|_| invalid())
}
fn invalid() -> CommandError {
    CommandError::new(
        "invalid_arguments",
        "The command arguments do not match its supported contract.",
    )
}
