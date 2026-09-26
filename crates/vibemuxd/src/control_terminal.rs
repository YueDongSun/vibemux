//! Control v4 observation-only terminal operations.
use super::*;
use crate::terminal_observer::{TerminalLinkRequest, TerminalQuery};

impl ControlClient {
    pub async fn terminal_inspect(
        &self,
        query: TerminalQuery,
    ) -> Result<TerminalSnapshot, ControlError> {
        match self
            .terminal_request(ControlOperation::TerminalInspect, &query)
            .await?
        {
            ControlPayload::TerminalSnapshot(snapshot) => Ok(snapshot),
            _ => Err(ControlError::InvalidFrame),
        }
    }
    pub async fn terminal_link(
        &self,
        request: TerminalLinkRequest,
    ) -> Result<TerminalBinding, ControlError> {
        match self
            .terminal_request(ControlOperation::TerminalLink, &request)
            .await?
        {
            ControlPayload::TerminalBinding(binding) => Ok(binding),
            _ => Err(ControlError::InvalidFrame),
        }
    }
    pub async fn terminal_focus(&self, binding_id: &str) -> Result<(), ControlError> {
        match self
            .terminal_request(ControlOperation::TerminalFocus, &binding_id)
            .await?
        {
            ControlPayload::TerminalFocused => Ok(()),
            _ => Err(ControlError::InvalidFrame),
        }
    }
    pub async fn terminal_unlink(&self, binding_id: &str) -> Result<(), ControlError> {
        match self
            .terminal_request(ControlOperation::TerminalUnlink, &binding_id)
            .await?
        {
            ControlPayload::TerminalUnlinked => Ok(()),
            _ => Err(ControlError::InvalidFrame),
        }
    }
    async fn terminal_request(
        &self,
        operation: ControlOperation,
        value: &impl Serialize,
    ) -> Result<ControlPayload, ControlError> {
        self.require_operation(operation.clone())?;
        let encoded = serde_json::to_string(value).map_err(|_| ControlError::InvalidRequest)?;
        self.request_with_argument(operation, Some(encoded), CONTROL_DEADLINE)
            .await
    }
}

pub(super) async fn dispatch(
    state: Arc<ServerState>,
    operation: ControlOperation,
    argument: Option<String>,
) -> Result<ControlPayload, ControlError> {
    let argument = argument.ok_or(ControlError::InvalidRequest)?;
    if argument.len() > 2048 {
        return Err(ControlError::InvalidRequest);
    }
    let writer = state
        .writer
        .lock()
        .map_err(|_| ControlError::ServerTerminated)?
        .as_ref()
        .ok_or(ControlError::ServerTerminated)?
        .handle()?;
    let result = match operation {
        ControlOperation::TerminalInspect => {
            let query: TerminalQuery =
                serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
            state
                .terminal
                .inspect(writer, query)
                .await
                .map(ControlPayload::TerminalSnapshot)
        }
        ControlOperation::TerminalLink => {
            let request: TerminalLinkRequest =
                serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
            state
                .terminal
                .link(writer, request)
                .await
                .map(ControlPayload::TerminalBinding)
        }
        ControlOperation::TerminalFocus => {
            let binding_id: String =
                serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
            state
                .terminal
                .focus(writer, &binding_id)
                .await
                .map(|()| ControlPayload::TerminalFocused)
        }
        ControlOperation::TerminalUnlink => {
            let binding_id: String =
                serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
            state
                .terminal
                .unlink(&binding_id)
                .map(|()| ControlPayload::TerminalUnlinked)
        }
        _ => return Err(ControlError::InvalidRequest),
    };
    let payload = result.map_err(|code| ControlError::Remote {
        code: code.to_string(),
    })?;
    // Leave headroom for the response/request-id envelope, including escaped
    // strings. A typed error is returned instead of dropping an oversized frame.
    if serde_json::to_vec(&payload)
        .map_err(|_| ControlError::InvalidFrame)?
        .len()
        > MAX_CONTROL_FRAME_BYTES - 1024
    {
        return Err(ControlError::FrameTooLarge);
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maximally_escaped_terminal_snapshot_fits_the_control_frame() {
        use vibemux_plugin_protocol::{
            terminal::{MAX_CWD_BYTES, MAX_TERMINAL_PANES},
            wire::{TerminalInventory, TerminalPane},
        };
        let query = TerminalQuery {
            project_id: vibemux_types::ProjectId::new(),
            task_id: vibemux_types::TaskId::new(),
            run_id: vibemux_types::RunId::new(),
            plugin_id: "p".repeat(128),
        };
        let pane = TerminalPane {
            pane_id: "1".repeat(20),
            cwd: "\\".repeat(MAX_CWD_BYTES),
            workspace: "\"".repeat(256),
            window_id: "2".repeat(20),
            tab_id: "3".repeat(20),
        };
        let inventory = TerminalInventory {
            instance_id: "x".repeat(128),
            panes: vec![pane.clone(); MAX_TERMINAL_PANES],
        };
        let binding = TerminalBinding {
            binding_id: uuid::Uuid::new_v4().to_string(),
            query,
            instance_id: inventory.instance_id.clone(),
            pane,
        };
        let response = ControlResponse::success(
            "r".repeat(128),
            ControlPayload::TerminalSnapshot(TerminalSnapshot {
                inventory,
                binding: Some(binding),
                checked_at_epoch_ms: u64::MAX,
            }),
        );
        assert!(serde_json::to_vec(&response).unwrap().len() < MAX_CONTROL_FRAME_BYTES);
    }
}
