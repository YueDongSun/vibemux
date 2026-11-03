//! Closed metadata-only contract for app-owned local diagnostics.
use crate::command_error::CommandError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const PROBE_VIEW_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProbeRequest {
    Start { operation_id: Uuid },
    Status { operation_id: Option<Uuid> },
    Cancel { operation_id: Uuid },
}
impl ProbeRequest {
    pub fn validate(&self) -> Result<(), CommandError> {
        let id = match self {
            Self::Start { operation_id } | Self::Cancel { operation_id } => Some(*operation_id),
            Self::Status { operation_id } => *operation_id,
        };
        if id.is_some_and(|id| id.is_nil()) {
            return Err(failure("probe_invalid_request"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbePhase {
    Running,
    Completed,
    Failed,
    Cancelled,
    Unconfirmed,
}
#[derive(Clone, Debug, Serialize)]
pub struct ProbeJobView {
    pub operation_id: Uuid,
    pub phase: ProbePhase,
    pub error_code: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct ProbeResultView {
    pub operation_id: Uuid,
    pub report: Value,
}
#[derive(Clone, Serialize)]
pub struct ProbeSnapshot {
    pub schema_version: u32,
    pub project_key: String,
    pub job: Option<ProbeJobView>,
    pub result: Option<ProbeResultView>,
    pub can_start: bool,
    pub blocking_code: Option<String>,
}

pub fn failure(code: &'static str) -> CommandError {
    CommandError::new(
        code,
        match code {
            "probe_busy" => "Another local check is still owned by the app.",
            "probe_history_full" => "Local check history is full. Reopen the app to clear it.",
            "probe_stopping" => "The app is stopping its local checks.",
            "probe_unsupported" => "Local checks are unavailable on this platform.",
            "probe_caller_denied" => "Local checks are available only in the main window.",
            "probe_scope_changed" => {
                "The selected project or window changed. Check the original operation status."
            }
            "probe_operation_conflict" => "This local check belongs to another registration.",
            "probe_operation_not_found" => "This local check has no recorded receipt yet.",
            "probe_cancel_unconfirmed" => {
                "Cancellation is unconfirmed. Check the operation status."
            }
            "probe_invalid_request" => "The local check request is invalid.",
            _ => "Local check state is unavailable.",
        },
    )
}
