//! Private reported file changes. Paths and opaque diffs confer no filesystem authority.

use crate::chat_tool::{MAX_TOOL_RECORD_BYTES, ToolRecordError};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use std::fmt;

const FORMAT: &str = "vibemux_file_change_tool_record";
const SCHEMA_VERSION: u32 = 1;
const MAX_CALL_ID_BYTES: usize = 512;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DIFF_BYTES: usize = 16 * 1024;
const MAX_RETAINED_DIFF_BYTES: usize = 32 * 1024;
const MAX_CHANGES: usize = 16;
const MAX_ORIGINAL_CHANGES: usize = 1024;
const MAX_ORIGINAL_DIFF_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeToolStatus {
    Completed,
    Failed,
    Declined,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Add,
    Delete,
    Update,
}

#[derive(Clone, Deserialize, Eq, PartialEq)]
#[serde(try_from = "PrivateChange")]
pub struct FileToolChange {
    pub path: String,
    pub kind: FileChangeKind,
    pub move_path: Option<String>,
    pub diff: String,
    pub diff_original_bytes: u64,
    pub diff_truncated: bool,
}

impl fmt::Debug for FileToolChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileToolChange")
            .field("kind", &self.kind)
            .field("diff_bytes", &self.diff.len())
            .field("diff_truncated", &self.diff_truncated)
            .finish_non_exhaustive()
    }
}

impl FileToolChange {
    pub fn from_observation(
        path: String,
        kind: FileChangeKind,
        move_path: Option<String>,
        mut diff: String,
    ) -> Result<Self, ToolRecordError> {
        let original = diff.len() as u64;
        let truncated = diff.len() > MAX_DIFF_BYTES;
        if truncated {
            let mut end = MAX_DIFF_BYTES;
            while !diff.is_char_boundary(end) {
                end -= 1;
            }
            diff.truncate(end);
        }
        let change = Self {
            path,
            kind,
            move_path,
            diff,
            diff_original_bytes: original,
            diff_truncated: truncated,
        };
        change.validate()?;
        Ok(change)
    }

    pub fn validate(&self) -> Result<(), ToolRecordError> {
        let prefix_valid = self.diff.len() <= MAX_DIFF_BYTES
            && self.diff_original_bytes <= MAX_ORIGINAL_DIFF_BYTES
            && if self.diff_truncated {
                self.diff_original_bytes > MAX_DIFF_BYTES as u64
                    && self.diff.len() >= MAX_DIFF_BYTES - 3
            } else {
                self.diff_original_bytes == self.diff.len() as u64
            };
        if !label(&self.path, MAX_PATH_BYTES)
            || !prefix_valid
            || self.move_path.as_ref().is_some_and(|path| {
                self.kind != FileChangeKind::Update || !label(path, MAX_PATH_BYTES)
            })
        {
            return Err(ToolRecordError::InvalidInput);
        }
        Ok(())
    }

    fn private_value(&self) -> Value {
        json!({"path":self.path,"kind":self.kind,"move_path":self.move_path,"diff":self.diff,
            "diff_original_bytes":self.diff_original_bytes,"diff_truncated":self.diff_truncated})
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq)]
#[serde(try_from = "PrivateRecord")]
pub struct FileChangeToolRecord {
    pub format: String,
    pub schema_version: u32,
    pub call_id: String,
    pub status: FileChangeToolStatus,
    pub changes: Vec<FileToolChange>,
    pub original_change_count: u32,
    pub changes_truncated: bool,
}

impl fmt::Debug for FileChangeToolRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileChangeToolRecord")
            .field("status", &self.status)
            .field("change_count", &self.changes.len())
            .field("changes_truncated", &self.changes_truncated)
            .finish_non_exhaustive()
    }
}

impl FileChangeToolRecord {
    pub fn from_observation(
        call_id: String,
        status: FileChangeToolStatus,
        changes: Vec<FileToolChange>,
    ) -> Result<Self, ToolRecordError> {
        // Validate the complete source, including entries beyond every display
        // budget, before choosing a retained prefix.
        if changes.len() > MAX_ORIGINAL_CHANGES || !label(&call_id, MAX_CALL_ID_BYTES) {
            return Err(ToolRecordError::InvalidInput);
        }
        let mut original_diff_bytes = 0_u64;
        for change in &changes {
            change.validate()?;
            original_diff_bytes = original_diff_bytes
                .checked_add(change.diff_original_bytes)
                .ok_or(ToolRecordError::InvalidInput)?;
            if original_diff_bytes > MAX_ORIGINAL_DIFF_BYTES {
                return Err(ToolRecordError::InvalidInput);
            }
        }
        let original_change_count =
            u32::try_from(changes.len()).map_err(|_| ToolRecordError::InvalidInput)?;
        let mut record = Self {
            format: FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            call_id,
            status,
            changes: Vec::new(),
            original_change_count,
            changes_truncated: original_change_count != 0,
        };
        let mut retained_diff_bytes = 0_usize;
        for change in changes {
            if record.changes.len() == MAX_CHANGES
                || retained_diff_bytes + change.diff.len() > MAX_RETAINED_DIFF_BYTES
            {
                break;
            }
            let diff_bytes = change.diff.len();
            record.changes.push(change);
            record.changes_truncated = record.original_change_count as usize > record.changes.len();
            if record.encode()?.len() > MAX_TOOL_RECORD_BYTES {
                record.changes.pop();
                record.changes_truncated = true;
                break;
            }
            retained_diff_bytes += diff_bytes;
        }
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), ToolRecordError> {
        if self.format != FORMAT
            || self.schema_version != SCHEMA_VERSION
            || !label(&self.call_id, MAX_CALL_ID_BYTES)
            || self.changes.len() > MAX_CHANGES
            || self.original_change_count as usize > MAX_ORIGINAL_CHANGES
            || (self.original_change_count as usize) < self.changes.len()
            || self.changes_truncated != (self.original_change_count as usize > self.changes.len())
        {
            return Err(ToolRecordError::InvalidInput);
        }
        let mut original = 0_u64;
        let mut retained = 0_usize;
        for change in &self.changes {
            change.validate()?;
            original = original
                .checked_add(change.diff_original_bytes)
                .ok_or(ToolRecordError::InvalidInput)?;
            retained += change.diff.len();
        }
        if original > MAX_ORIGINAL_DIFF_BYTES || retained > MAX_RETAINED_DIFF_BYTES {
            return Err(ToolRecordError::InvalidInput);
        }
        if self.encode()?.len() > MAX_TOOL_RECORD_BYTES {
            return Err(ToolRecordError::TooLarge);
        }
        Ok(())
    }

    fn encode(&self) -> Result<String, ToolRecordError> {
        serde_json::to_string(&json!({"format":self.format,"schema_version":self.schema_version,"call_id":self.call_id,
            "status":self.status,"changes":self.changes.iter().map(FileToolChange::private_value).collect::<Vec<_>>(),
            "original_change_count":self.original_change_count,"changes_truncated":self.changes_truncated}))
            .map_err(|_| ToolRecordError::InvalidInput)
    }

    pub fn to_private_text(&self) -> Result<String, ToolRecordError> {
        self.validate()?;
        self.encode()
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
struct PrivateChange {
    path: String,
    kind: FileChangeKind,
    #[serde(deserialize_with = "required_nullable")]
    move_path: Option<String>,
    diff: String,
    diff_original_bytes: u64,
    diff_truncated: bool,
}
impl TryFrom<PrivateChange> for FileToolChange {
    type Error = ToolRecordError;
    fn try_from(value: PrivateChange) -> Result<Self, Self::Error> {
        let change = Self {
            path: value.path,
            kind: value.kind,
            move_path: value.move_path,
            diff: value.diff,
            diff_original_bytes: value.diff_original_bytes,
            diff_truncated: value.diff_truncated,
        };
        change.validate()?;
        Ok(change)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateRecord {
    format: String,
    schema_version: u32,
    call_id: String,
    status: FileChangeToolStatus,
    changes: Vec<FileToolChange>,
    original_change_count: u32,
    changes_truncated: bool,
}
impl TryFrom<PrivateRecord> for FileChangeToolRecord {
    type Error = ToolRecordError;
    fn try_from(value: PrivateRecord) -> Result<Self, Self::Error> {
        let record = Self {
            format: value.format,
            schema_version: value.schema_version,
            call_id: value.call_id,
            status: value.status,
            changes: value.changes,
            original_change_count: value.original_change_count,
            changes_truncated: value.changes_truncated,
        };
        record.validate()?;
        Ok(record)
    }
}
fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
fn label(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_tool::{CommandToolRecord, CommandToolStatus, ToolRecord};
    use serde_json::{Value, json};

    fn change(path: &str, diff: &str) -> FileToolChange {
        FileToolChange::from_observation(
            path.into(),
            FileChangeKind::Update,
            Some("moved.rs".into()),
            diff.into(),
        )
        .expect("change")
    }
    fn record(changes: Vec<FileToolChange>) -> FileChangeToolRecord {
        FileChangeToolRecord::from_observation(
            "private_call".into(),
            FileChangeToolStatus::Completed,
            changes,
        )
        .expect("record")
    }

    #[test]
    fn command_v1_body_is_byte_identical_through_the_generic_codec() {
        let command = CommandToolRecord::from_observation(
            "call".into(),
            CommandToolStatus::Completed,
            "echo hi".into(),
            "C:/cwd".into(),
            None,
            Some(0),
            None,
        )
        .expect("command");
        let expected = r#"{"call_id":"call","command":"echo hi","command_original_bytes":7,"command_truncated":false,"duration_ms":null,"exit_code":0,"format":"vibemux_command_tool_record","output":null,"output_original_bytes":null,"output_truncated":false,"schema_version":1,"status":"completed","working_directory":"C:/cwd"}"#;
        assert_eq!(command.to_private_text().expect("v1 body"), expected);
        let generic = ToolRecord::Command(command);
        assert_eq!(generic.to_private_text().expect("generic body"), expected);
        assert_eq!(
            ToolRecord::from_private_text(expected).expect("decode"),
            generic
        );
        assert_eq!(generic.call_id(), "call");
        assert!(!generic.is_truncated());
    }

    #[test]
    fn file_envelope_and_inner_members_are_closed_required_and_redacted() {
        let original = record(vec![change("private_path.rs", "private diff")]);
        let body = original.to_private_text().expect("body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(value.as_object().expect("outer").len(), 7);
        assert_eq!(value["changes"][0].as_object().expect("inner").len(), 6);
        assert_eq!(
            FileChangeToolRecord::from_private_text(&body).expect("decode"),
            original
        );
        assert_eq!(
            ToolRecord::from_private_text(&body).expect("generic decode"),
            ToolRecord::FileChange(original.clone())
        );
        for (field, bad) in [
            ("format", json!("wrong")),
            ("schema_version", json!(2)),
            ("unknown", json!(true)),
        ] {
            let mut value = value.clone();
            value[field] = bad;
            assert!(FileChangeToolRecord::from_private_text(&value.to_string()).is_err());
        }
        let mut absent = value.clone();
        absent["changes"][0]
            .as_object_mut()
            .expect("inner")
            .remove("move_path");
        assert!(FileChangeToolRecord::from_private_text(&absent.to_string()).is_err());
        let mut unknown = value;
        unknown["changes"][0]["unknown"] = json!(true);
        assert!(FileChangeToolRecord::from_private_text(&unknown.to_string()).is_err());
        let duplicate = body.replacen(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
            1,
        );
        assert!(FileChangeToolRecord::from_private_text(&duplicate).is_err());
        assert!(ToolRecord::from_private_text(&duplicate).is_err());
        for private in ["private_call", "private_path", "private diff", "moved.rs"] {
            assert!(!format!("{original:?}").contains(private));
        }
    }

    #[test]
    fn unicode_prefix_list_and_total_diff_budgets_keep_explicit_original_counts() {
        let unicode = change("unicode.rs", &"界".repeat(6000));
        assert_eq!(unicode.diff_original_bytes, 18_000);
        assert!(unicode.diff_truncated && unicode.diff.len() <= 16 * 1024);
        let aggregate = record(vec![unicode.clone(), unicode.clone(), unicode]);
        assert_eq!(aggregate.original_change_count, 3);
        assert_eq!(aggregate.changes.len(), 2);
        assert!(aggregate.changes_truncated);
        assert!(
            aggregate
                .changes
                .iter()
                .map(|change| change.diff.len())
                .sum::<usize>()
                <= 32 * 1024
        );
        assert!(ToolRecord::FileChange(aggregate).is_truncated());
        let list = record(
            (0..17)
                .map(|index| change(&format!("{index}.rs"), ""))
                .collect(),
        );
        assert_eq!(list.changes.len(), 16);
        assert_eq!(list.original_change_count, 17);
        assert!(list.changes_truncated);
        let long_list = record(
            (0..17)
                .map(|index| change(&format!("long_{index}.rs"), &"x".repeat(16 * 1024)))
                .collect(),
        );
        assert_eq!(long_list.changes.len(), 2);
        assert_eq!(long_list.original_change_count, 17);
        assert!(long_list.changes_truncated);
        assert!(record(vec![]).changes.is_empty());
    }

    #[test]
    fn encoded_budget_stops_before_an_entry_and_all_source_entries_are_validated() {
        let escaped = "\"".repeat(4096);
        let heavy = FileToolChange::from_observation(
            escaped.clone(),
            FileChangeKind::Update,
            Some(escaped),
            String::new(),
        )
        .expect("heavy change");
        let body = record(vec![heavy; 16]);
        assert_eq!(
            body.changes.len(),
            15,
            "JSON escaping reaches the body cap before entry 16"
        );
        assert!(body.changes_truncated);
        assert!(body.to_private_text().expect("body").len() <= 256 * 1024);
        let mut source = (0..17)
            .map(|index| change(&format!("{index}.rs"), ""))
            .collect::<Vec<_>>();
        source[16].path = "bad\npath".into();
        assert!(
            FileChangeToolRecord::from_observation(
                "call".into(),
                FileChangeToolStatus::Completed,
                source
            )
            .is_err()
        );
        assert!(
            FileToolChange::from_observation(
                "file".into(),
                FileChangeKind::Add,
                Some("moved".into()),
                "".into()
            )
            .is_err()
        );
        let source = (0..65)
            .map(|index| change(&format!("{index}.rs"), &"x".repeat(16 * 1024)))
            .collect();
        assert!(
            FileChangeToolRecord::from_observation(
                "call".into(),
                FileChangeToolStatus::Completed,
                source
            )
            .is_err()
        );
    }
}
