//! One bounded ledger for terminal command and file-change observations.
use serde_json::{Value, json};
use vibemux_types::chat_file_tool::{
    FileChangeKind, FileChangeToolRecord, FileChangeToolStatus, FileToolChange,
};
use vibemux_types::chat_tool::{
    CommandToolRecord, CommandToolStatus, MAX_TOOL_RECORD_BYTES, MAX_TOOL_RECORDS, ToolRecord,
};

use super::{
    IgnoredReason, SessionError, SessionMachine, SessionTransition, ToolObservation,
    session::identifier,
};
use crate::dispatch::Sha256Digest;

const MAX_SOURCE_FILE_CHANGES: usize = 1024;

#[derive(Default)]
pub(super) struct ToolState {
    entries: Vec<TerminalTool>,
    bytes: usize,
    pub(super) incomplete: bool,
}

struct TerminalTool {
    record: ToolRecord,
    /// Internal replay identity includes discarded tails; no digest is published.
    fingerprint: Sha256Digest,
}

impl ToolState {
    pub(super) fn contains_id(&self, id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.record.call_id() == id)
    }
}

impl SessionMachine {
    fn incomplete_tools(&mut self) -> SessionTransition {
        self.codex.tools.incomplete = true;
        SessionTransition {
            tool_capture_incomplete: true,
            ..Default::default()
        }
    }

    pub(super) fn codex_tool_item(
        &mut self,
        item: &Value,
    ) -> Result<SessionTransition, SessionError> {
        let active = self.active.as_ref().ok_or(SessionError::UncorrelatedTurn)?;
        let request_id = active.summary.request_id;
        let turn_id = active.summary.turn_id;
        if self.codex.tools.incomplete {
            return Ok(self.incomplete_tools());
        }
        let projection = match item.get("type").and_then(Value::as_str) {
            Some("commandExecution") => command_record(item)
                .map(|(record, fingerprint)| (ToolRecord::Command(record), fingerprint)),
            Some("fileChange") => file_record(item)
                .map(|(record, fingerprint)| (ToolRecord::FileChange(record), fingerprint)),
            _ => return Ok(Self::ignored(IgnoredReason::UnknownNotification)),
        };
        let (record, fingerprint) = match projection {
            Ok(record) => record,
            Err(_) => return Ok(self.incomplete_tools()),
        };
        if self.codex.items.contains_key(record.call_id()) {
            return Ok(self.incomplete_tools());
        }
        if let Some(previous) = self
            .codex
            .tools
            .entries
            .iter()
            .find(|entry| entry.record.call_id() == record.call_id())
        {
            return if previous.record == record && previous.fingerprint == fingerprint {
                Ok(Self::ignored(IgnoredReason::Duplicate))
            } else {
                Ok(self.incomplete_tools())
            };
        }
        let bytes = match record.to_private_text() {
            Ok(text) => text.len(),
            Err(_) => return Ok(self.incomplete_tools()),
        };
        if self.codex.tools.entries.len() >= MAX_TOOL_RECORDS
            || self.codex.tools.entries.len() >= self.limits.max_text_items
            || self.codex.tools.bytes.saturating_add(bytes) > MAX_TOOL_RECORD_BYTES
        {
            return Ok(self.incomplete_tools());
        }
        self.codex.tools.bytes += bytes;
        self.codex.command_progress.retire(record.call_id());
        self.codex.tools.entries.push(TerminalTool {
            record: record.clone(),
            fingerprint,
        });
        Ok(SessionTransition {
            tools: vec![ToolObservation {
                request_id,
                turn_id,
                record,
            }],
            ..Default::default()
        })
    }
}

fn file_record(item: &Value) -> Result<(FileChangeToolRecord, Sha256Digest), SessionError> {
    let id = identifier(item.get("id"))?;
    let status = match item.get("status").and_then(Value::as_str) {
        Some("completed") => FileChangeToolStatus::Completed,
        Some("failed") => FileChangeToolStatus::Failed,
        Some("declined") => FileChangeToolStatus::Declined,
        _ => return Err(SessionError::InvalidFrame),
    };
    let source = item
        .get("changes")
        .and_then(Value::as_array)
        .ok_or(SessionError::InvalidFrame)?;
    if source.len() > MAX_SOURCE_FILE_CHANGES {
        return Err(SessionError::InvalidFrame);
    }
    let mut changes = Vec::with_capacity(source.len());
    let mut normalized = Vec::with_capacity(source.len());
    for change in source {
        let path = change
            .get("path")
            .and_then(Value::as_str)
            .ok_or(SessionError::InvalidFrame)?;
        let diff = change
            .get("diff")
            .and_then(Value::as_str)
            .ok_or(SessionError::InvalidFrame)?;
        let kind = change
            .get("kind")
            .filter(|kind| kind.is_object())
            .ok_or(SessionError::InvalidFrame)?;
        let change_kind = match kind.get("type").and_then(Value::as_str) {
            Some("add") => FileChangeKind::Add,
            Some("delete") => FileChangeKind::Delete,
            Some("update") => FileChangeKind::Update,
            _ => return Err(SessionError::InvalidFrame),
        };
        let move_path = match kind.get("move_path") {
            None | Some(Value::Null) => None,
            Some(Value::String(path)) => Some(path.as_str()),
            _ => return Err(SessionError::InvalidFrame),
        };
        changes.push(
            FileToolChange::from_observation(
                path.into(),
                change_kind,
                move_path.map(str::to_owned),
                diff.into(),
            )
            .map_err(|_| SessionError::InvalidFrame)?,
        );
        // Retain only an internal replay fingerprint of complete known source
        // fields, including entries and diff tails omitted by the projection.
        normalized.push(json!([path, change_kind, move_path, diff]));
    }
    let record = FileChangeToolRecord::from_observation(id.clone(), status, changes)
        .map_err(|_| SessionError::InvalidFrame)?;
    let bytes = serde_json::to_vec(&json!([id, status, normalized]))
        .map_err(|_| SessionError::InvalidFrame)?;
    Ok((record, Sha256Digest::of(&bytes)))
}

fn command_record(item: &Value) -> Result<(CommandToolRecord, Sha256Digest), SessionError> {
    let id = identifier(item.get("id"))?;
    let command = item
        .get("command")
        .and_then(Value::as_str)
        .ok_or(SessionError::InvalidFrame)?;
    let cwd = item
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or(SessionError::InvalidFrame)?;
    let status = match item.get("status").and_then(Value::as_str) {
        Some("completed") => CommandToolStatus::Completed,
        Some("failed") => CommandToolStatus::Failed,
        Some("declined") => CommandToolStatus::Declined,
        _ => return Err(SessionError::InvalidFrame),
    };
    let output = match item.get("aggregatedOutput") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.as_str()),
        _ => return Err(SessionError::InvalidFrame),
    };
    let exit_code = match item.get("exitCode") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or(SessionError::InvalidFrame)?,
        ),
    };
    let duration = match item.get("durationMs") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().ok_or(SessionError::InvalidFrame)?),
    };
    let record = CommandToolRecord::from_observation(
        id.clone(),
        status,
        command.into(),
        cwd.into(),
        output.map(str::to_owned),
        exit_code,
        duration,
    )
    .map_err(|_| SessionError::InvalidFrame)?;
    let full_record = json!([id, status, command, cwd, output, exit_code, duration]);
    let bytes = serde_json::to_vec(&full_record).map_err(|_| SessionError::InvalidFrame)?;
    Ok((record, Sha256Digest::of(&bytes)))
}
