//! Private bounded command observations; never ordinary event or log metadata.

use std::fmt;

use crate::chat_file_tool::FileChangeToolRecord;
use serde::{Deserialize, Deserializer};
use serde_json::json;
use thiserror::Error;

pub const MAX_TOOL_RECORDS: usize = 64;
pub const MAX_TOOL_RECORD_BYTES: usize = 256 * 1024;
const FORMAT: &str = "vibemux_command_tool_record";
const SCHEMA_VERSION: u32 = 1;
const MAX_COMMAND_BYTES: usize = 8 * 1024;
const MAX_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_DIRECTORY_BYTES: usize = 32 * 1024;
const MAX_CALL_ID_BYTES: usize = 512;
const MAX_ORIGINAL_BYTES: u64 = 1024 * 1024;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandToolStatus {
    Completed,
    Failed,
    Declined,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ToolRecordError {
    #[error("command tool record is invalid")]
    InvalidInput,
    #[error("command tool private text exceeds its byte limit")]
    TooLarge,
}

/// One private immutable tool body. Each variant preserves its own versioned codec.
#[derive(Clone, Eq, PartialEq)]
pub enum ToolRecord {
    Command(CommandToolRecord),
    FileChange(FileChangeToolRecord),
}

impl fmt::Debug for ToolRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(record) => record.fmt(formatter),
            Self::FileChange(record) => record.fmt(formatter),
        }
    }
}

impl ToolRecord {
    #[must_use]
    pub fn call_id(&self) -> &str {
        match self {
            Self::Command(record) => &record.call_id,
            Self::FileChange(record) => &record.call_id,
        }
    }

    #[must_use]
    pub fn is_truncated(&self) -> bool {
        match self {
            Self::Command(record) => record.command_truncated || record.output_truncated,
            Self::FileChange(record) => {
                record.changes_truncated
                    || record.changes.iter().any(|change| change.diff_truncated)
            }
        }
    }

    pub fn to_private_text(&self) -> Result<String, ToolRecordError> {
        match self {
            Self::Command(record) => record.to_private_text(),
            Self::FileChange(record) => record.to_private_text(),
        }
    }

    pub fn from_private_text(text: &str) -> Result<Self, ToolRecordError> {
        if text.len() > MAX_TOOL_RECORD_BYTES {
            return Err(ToolRecordError::TooLarge);
        }
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|_| ToolRecordError::InvalidInput)?;
        match value.get("format").and_then(serde_json::Value::as_str) {
            Some("vibemux_command_tool_record") => {
                CommandToolRecord::from_private_text(text).map(Self::Command)
            }
            Some("vibemux_file_change_tool_record") => {
                FileChangeToolRecord::from_private_text(text).map(Self::FileChange)
            }
            _ => Err(ToolRecordError::InvalidInput),
        }
    }
}

/// Private immutable content. Serialization requires the explicit bounded
/// to_private_text boundary; this type deliberately does not implement Serialize.
#[derive(Clone, Deserialize, Eq, PartialEq)]
#[serde(try_from = "PrivateRecord")]
pub struct CommandToolRecord {
    pub format: String,
    pub schema_version: u32,
    pub call_id: String,
    pub status: CommandToolStatus,
    pub command: String,
    pub command_original_bytes: u64,
    pub command_truncated: bool,
    pub working_directory: String,
    pub output: Option<String>,
    pub output_original_bytes: Option<u64>,
    pub output_truncated: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
}

impl fmt::Debug for CommandToolRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandToolRecord")
            .field("status", &self.status)
            .field("command_bytes", &self.command.len())
            .field("output_bytes", &self.output.as_ref().map(String::len))
            .field("command_truncated", &self.command_truncated)
            .field("output_truncated", &self.output_truncated)
            .finish_non_exhaustive()
    }
}

impl CommandToolRecord {
    pub fn from_observation(
        call_id: String,
        status: CommandToolStatus,
        command: String,
        working_directory: String,
        output: Option<String>,
        exit_code: Option<i32>,
        duration_ms: Option<u64>,
    ) -> Result<Self, ToolRecordError> {
        let command_original_bytes = command.len() as u64;
        let output_original_bytes = output.as_ref().map(|value| value.len() as u64);
        if command_original_bytes > MAX_ORIGINAL_BYTES
            || output_original_bytes.is_some_and(|bytes| bytes > MAX_ORIGINAL_BYTES)
        {
            return Err(ToolRecordError::InvalidInput);
        }
        let record = Self {
            format: FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            call_id,
            status,
            command_truncated: command.len() > MAX_COMMAND_BYTES,
            command: prefix(command, MAX_COMMAND_BYTES),
            command_original_bytes,
            working_directory,
            output_truncated: output
                .as_ref()
                .is_some_and(|value| value.len() > MAX_OUTPUT_BYTES),
            output: output.map(|value| prefix(value, MAX_OUTPUT_BYTES)),
            output_original_bytes,
            exit_code,
            duration_ms,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), ToolRecordError> {
        if self.format != FORMAT
            || self.schema_version != SCHEMA_VERSION
            || !valid_label(&self.call_id, MAX_CALL_ID_BYTES)
            || !valid_label(&self.working_directory, MAX_DIRECTORY_BYTES)
            || !valid_prefix(
                &self.command,
                self.command_original_bytes,
                self.command_truncated,
                MAX_COMMAND_BYTES,
            )
            || self
                .duration_ms
                .is_some_and(|value| value > MAX_SAFE_INTEGER)
            || match (&self.output, self.output_original_bytes) {
                (Some(output), Some(original)) => {
                    !valid_prefix(output, original, self.output_truncated, MAX_OUTPUT_BYTES)
                }
                (None, None) => self.output_truncated,
                _ => true,
            }
        {
            return Err(ToolRecordError::InvalidInput);
        }
        Ok(())
    }

    pub fn to_private_text(&self) -> Result<String, ToolRecordError> {
        self.validate()?;
        let text = serde_json::to_string(&json!({
            "format":self.format,"schema_version":self.schema_version,"call_id":self.call_id,
            "status":self.status,"command":self.command,"command_original_bytes":self.command_original_bytes,
            "command_truncated":self.command_truncated,"working_directory":self.working_directory,
            "output":self.output,"output_original_bytes":self.output_original_bytes,
            "output_truncated":self.output_truncated,"exit_code":self.exit_code,"duration_ms":self.duration_ms,
        })).map_err(|_| ToolRecordError::InvalidInput)?;
        if text.len() > MAX_TOOL_RECORD_BYTES {
            return Err(ToolRecordError::TooLarge);
        }
        Ok(text)
    }

    pub fn from_private_text(text: &str) -> Result<Self, ToolRecordError> {
        if text.len() > MAX_TOOL_RECORD_BYTES {
            return Err(ToolRecordError::TooLarge);
        }
        serde_json::from_str(text).map_err(|_| ToolRecordError::InvalidInput)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateRecord {
    format: String,
    schema_version: u32,
    call_id: String,
    status: CommandToolStatus,
    command: String,
    command_original_bytes: u64,
    command_truncated: bool,
    working_directory: String,
    #[serde(deserialize_with = "required_nullable")]
    output: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    output_original_bytes: Option<u64>,
    output_truncated: bool,
    #[serde(deserialize_with = "required_nullable")]
    exit_code: Option<i32>,
    #[serde(deserialize_with = "required_nullable")]
    duration_ms: Option<u64>,
}

impl TryFrom<PrivateRecord> for CommandToolRecord {
    type Error = ToolRecordError;
    fn try_from(value: PrivateRecord) -> Result<Self, Self::Error> {
        let record = Self {
            format: value.format,
            schema_version: value.schema_version,
            call_id: value.call_id,
            status: value.status,
            command: value.command,
            command_original_bytes: value.command_original_bytes,
            command_truncated: value.command_truncated,
            working_directory: value.working_directory,
            output: value.output,
            output_original_bytes: value.output_original_bytes,
            output_truncated: value.output_truncated,
            exit_code: value.exit_code,
            duration_ms: value.duration_ms,
        };
        record.validate()?;
        Ok(record)
    }
}

// Nullable fields remain mandatory members of the closed private envelope.
fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

fn valid_label(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn prefix(mut value: String, limit: usize) -> String {
    if value.len() > limit {
        let mut end = limit;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    value
}

fn valid_prefix(value: &str, original: u64, truncated: bool, limit: usize) -> bool {
    value.len() <= limit
        && original <= MAX_ORIGINAL_BYTES
        && if truncated {
            original > limit as u64 && value.len() >= limit.saturating_sub(3)
        } else {
            original == value.len() as u64
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn record() -> CommandToolRecord {
        CommandToolRecord::from_observation(
            "private_call".into(),
            CommandToolStatus::Completed,
            "private command".into(),
            "C:/private/cwd".into(),
            Some("private output".into()),
            Some(0),
            Some(12),
        )
        .expect("observation")
    }

    #[test]
    fn private_envelope_round_trips_with_exact_fields_and_redacted_debug() {
        let record = record();
        let body = record.to_private_text().expect("private body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(value.as_object().expect("object").len(), 13);
        assert_eq!(value["format"], "vibemux_command_tool_record");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(
            CommandToolRecord::from_private_text(&body).expect("decode"),
            record
        );
        assert_eq!(
            serde_json::from_str::<CommandToolRecord>(&body).expect("strict Deserialize"),
            record
        );
        let debug = format!("{record:?}");
        for private in [
            "private_call",
            "private command",
            "private/cwd",
            "private output",
        ] {
            assert!(!debug.contains(private));
        }
    }

    #[test]
    fn private_envelope_rejects_unknown_missing_nullable_and_wrong_discriminators() {
        let body = record().to_private_text().expect("body");
        let original: Value = serde_json::from_str(&body).expect("JSON");
        for field in [
            "output",
            "output_original_bytes",
            "exit_code",
            "duration_ms",
        ] {
            let mut value = original.clone();
            value.as_object_mut().expect("object").remove(field);
            assert!(
                serde_json::from_value::<CommandToolRecord>(value).is_err(),
                "{field}"
            );
        }
        for (field, value) in [
            ("unknown", json!(true)),
            ("format", json!("other")),
            ("schema_version", json!(2)),
            ("status", json!("inProgress")),
        ] {
            let mut invalid = original.clone();
            invalid[field] = value;
            assert!(
                CommandToolRecord::from_private_text(&invalid.to_string()).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn unicode_truncation_retains_boundaries_original_counts_and_explicit_flags() {
        let record = CommandToolRecord::from_observation(
            "call".into(),
            CommandToolStatus::Failed,
            "界".repeat(4000),
            "C:/cwd".into(),
            Some("😀".repeat(6000)),
            Some(-1),
            None,
        )
        .expect("bounded record");
        assert!(record.command.len() <= 8 * 1024);
        assert!(record.output.as_ref().expect("output").len() <= 16 * 1024);
        assert_eq!(record.command_original_bytes, 12_000);
        assert_eq!(record.output_original_bytes, Some(24_000));
        assert!(record.command_truncated && record.output_truncated);
        assert!(!record.command.contains('\u{fffd}'));
        assert!(record.validate().is_ok());
    }

    #[test]
    fn nullable_declined_record_and_validation_limits_are_explicit() {
        let record = CommandToolRecord::from_observation(
            "call".into(),
            CommandToolStatus::Declined,
            String::new(),
            "C:/cwd".into(),
            None,
            None,
            None,
        )
        .expect("declined nullable result");
        assert!(
            record.output.is_none()
                && record.output_original_bytes.is_none()
                && !record.output_truncated
        );
        for field in ["call", "directory", "count", "flag", "duration"] {
            let mut invalid = record.clone();
            match field {
                "call" => invalid.call_id = "x".repeat(513),
                "directory" => invalid.working_directory = "bad\npath".into(),
                "count" => invalid.command_original_bytes = 1_048_577,
                "flag" => invalid.command_truncated = true,
                "duration" => invalid.duration_ms = Some(9_007_199_254_740_992),
                _ => unreachable!(),
            }
            assert!(invalid.validate().is_err(), "{field}");
        }
        assert!(
            CommandToolRecord::from_private_text(&" ".repeat(MAX_TOOL_RECORD_BYTES + 1)).is_err()
        );
        assert!(
            CommandToolRecord::from_observation(
                "call".into(),
                CommandToolStatus::Completed,
                "x".repeat(1_048_577),
                "C:/cwd".into(),
                None,
                None,
                None
            )
            .is_err()
        );
    }
}
