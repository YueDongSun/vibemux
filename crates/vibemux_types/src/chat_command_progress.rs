//! Private transient command display data; never an execution grant or history row.
use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::chat_tool::{MAX_TOOL_RECORD_BYTES, MAX_TOOL_RECORDS};

pub const COMMAND_PROGRESS_SCHEMA_VERSION: u32 = 1;
pub const MAX_COMMAND_PROGRESS_IDENTITIES: usize = MAX_TOOL_RECORDS;
pub const MAX_COMMAND_PROGRESS_MEMORY_BYTES: usize = MAX_TOOL_RECORD_BYTES;
pub const MAX_COMMAND_PROGRESS_ENTRIES: usize = 8;
pub const MAX_COMMAND_PROGRESS_SNAPSHOT_BYTES: usize = 8 * 1024;
pub const MAX_COMMAND_PROGRESS_COMMAND_BYTES: usize = 8 * 1024;
pub const MAX_COMMAND_PROGRESS_DIRECTORY_BYTES: usize = 32 * 1024;
pub const MAX_COMMAND_PROGRESS_OUTPUT_BYTES: usize = 16 * 1024;
pub const MAX_COMMAND_PROGRESS_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandProgressScope {
    Saved,
    Workspace,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CommandProgressError {
    #[error("command progress is invalid")]
    InvalidInput,
    #[error("command progress exceeds its byte limit")]
    TooLarge,
    #[error("command progress cannot be encoded")]
    Encoding,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandProgressPhase {
    InProgress,
    Finalizing,
    Recorded,
    Unconfirmed,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandProgressContent {
    pub command: String,
    pub command_original_bytes: u64,
    pub command_truncated: bool,
    pub working_directory: String,
    pub directory_original_bytes: u64,
    pub directory_truncated: bool,
    pub output_tail: Option<String>,
    pub output_original_bytes: Option<u64>,
    pub output_omitted_bytes: u64,
}

impl fmt::Debug for CommandProgressContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandProgressContent")
            .field("command_bytes", &self.command.len())
            .field("directory_bytes", &self.working_directory.len())
            .field("output_bytes", &self.output_tail.as_ref().map(String::len))
            .finish_non_exhaustive()
    }
}

impl CommandProgressContent {
    pub fn validate(&self) -> Result<(), CommandProgressError> {
        for (text, original, truncated, maximum) in [
            (
                &self.command,
                self.command_original_bytes,
                self.command_truncated,
                MAX_COMMAND_PROGRESS_COMMAND_BYTES,
            ),
            (
                &self.working_directory,
                self.directory_original_bytes,
                self.directory_truncated,
                MAX_COMMAND_PROGRESS_DIRECTORY_BYTES,
            ),
        ] {
            let bytes = u64::try_from(text.len()).map_err(|_| CommandProgressError::TooLarge)?;
            if text.len() > maximum
                || original > MAX_COMMAND_PROGRESS_INTEGER
                || original < bytes
                || truncated != (original > bytes)
            {
                return Err(CommandProgressError::InvalidInput);
            }
        }
        match (&self.output_tail, self.output_original_bytes) {
            (None, None) if self.output_omitted_bytes == 0 => {}
            (Some(text), Some(original)) => {
                let bytes =
                    u64::try_from(text.len()).map_err(|_| CommandProgressError::TooLarge)?;
                if text.len() > MAX_COMMAND_PROGRESS_OUTPUT_BYTES
                    || original > MAX_COMMAND_PROGRESS_INTEGER
                    || original.checked_sub(bytes) != Some(self.output_omitted_bytes)
                {
                    return Err(CommandProgressError::InvalidInput);
                }
            }
            _ => return Err(CommandProgressError::InvalidInput),
        }
        Ok(())
    }

    pub fn retained_bytes(&self) -> Result<usize, CommandProgressError> {
        self.command
            .len()
            .checked_add(self.working_directory.len())
            .and_then(|count| count.checked_add(self.output_tail.as_ref().map_or(0, String::len)))
            .ok_or(CommandProgressError::TooLarge)
    }

    fn largest_field(&self) -> usize {
        self.command
            .len()
            .max(self.working_directory.len())
            .max(self.output_tail.as_ref().map_or(0, String::len))
    }

    fn reduce_largest(&mut self) -> Result<bool, CommandProgressError> {
        let maximum = self.largest_field();
        if maximum == 0 {
            return Ok(false);
        }
        if self.command.len() == maximum {
            truncate_prefix(&mut self.command, maximum / 2);
            self.command_truncated = self.command_original_bytes
                > u64::try_from(self.command.len()).map_err(|_| CommandProgressError::TooLarge)?;
        } else if self.working_directory.len() == maximum {
            truncate_prefix(&mut self.working_directory, maximum / 2);
            self.directory_truncated = self.directory_original_bytes
                > u64::try_from(self.working_directory.len())
                    .map_err(|_| CommandProgressError::TooLarge)?;
        } else if let (Some(output), Some(original)) =
            (&mut self.output_tail, self.output_original_bytes)
        {
            truncate_suffix(output, maximum / 2);
            self.output_omitted_bytes = original
                .checked_sub(
                    u64::try_from(output.len()).map_err(|_| CommandProgressError::TooLarge)?,
                )
                .ok_or(CommandProgressError::InvalidInput)?;
        }
        Ok(true)
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandProgressEntry {
    pub record_id: Uuid,
    pub phase: CommandProgressPhase,
    pub content: CommandProgressContent,
}

impl fmt::Debug for CommandProgressEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandProgressEntry")
            .field("phase", &self.phase)
            .field("content", &self.content)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandProgressSnapshot {
    pub schema_version: u32,
    /// The daemon-derived Saved turn or Workspace attempt, interpreted by scope.
    pub canonical_id: Uuid,
    pub revision: u64,
    /// Input and output order is oldest to newest observation.
    pub entries: Vec<CommandProgressEntry>,
    pub omitted_entries: u32,
    pub capture_limited: bool,
}

impl fmt::Debug for CommandProgressSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandProgressSnapshot")
            .field("revision", &self.revision)
            .field("entries", &self.entries.len())
            .field("omitted_entries", &self.omitted_entries)
            .field("capture_limited", &self.capture_limited)
            .finish_non_exhaustive()
    }
}

impl CommandProgressSnapshot {
    /// Build one whole latest-observation reply. No revision-bound paging lease
    /// is needed, so fast output cannot continually invalidate partial pages.
    pub fn bounded(
        canonical_id: Uuid,
        revision: u64,
        mut entries: Vec<CommandProgressEntry>,
        omitted_entries: u32,
        capture_limited: bool,
    ) -> Result<Self, CommandProgressError> {
        validate_entries(&entries)?;
        let known = entries
            .len()
            .checked_add(
                usize::try_from(omitted_entries).map_err(|_| CommandProgressError::TooLarge)?,
            )
            .ok_or(CommandProgressError::TooLarge)?;
        if known > MAX_COMMAND_PROGRESS_IDENTITIES {
            return Err(CommandProgressError::TooLarge);
        }
        let raw_bytes = entries.iter().try_fold(0usize, |count, entry| {
            count
                .checked_add(entry.content.retained_bytes()?)
                .ok_or(CommandProgressError::TooLarge)
        })?;
        if raw_bytes > MAX_COMMAND_PROGRESS_MEMORY_BYTES {
            return Err(CommandProgressError::TooLarge);
        }
        let dropped = entries.len().saturating_sub(MAX_COMMAND_PROGRESS_ENTRIES);
        if dropped > 0 {
            entries = entries.split_off(dropped);
        }
        let mut snapshot = Self {
            schema_version: COMMAND_PROGRESS_SCHEMA_VERSION,
            canonical_id,
            revision,
            entries,
            omitted_entries: omitted_entries
                .checked_add(u32::try_from(dropped).map_err(|_| CommandProgressError::TooLarge)?)
                .ok_or(CommandProgressError::TooLarge)?,
            capture_limited: capture_limited || dropped > 0,
        };
        snapshot.validate_header()?;
        while snapshot.encoded_len()? > MAX_COMMAND_PROGRESS_SNAPSHOT_BYTES {
            let largest = snapshot
                .entries
                .iter()
                .enumerate()
                .max_by_key(|(_, entry)| entry.content.largest_field())
                .map(|(index, entry)| (index, entry.content.largest_field()));
            if let Some((index, _)) = largest.filter(|(_, bytes)| *bytes > 0) {
                snapshot.entries[index].content.reduce_largest()?;
            } else if !snapshot.entries.is_empty() {
                snapshot.entries.remove(0);
                snapshot.omitted_entries = snapshot
                    .omitted_entries
                    .checked_add(1)
                    .ok_or(CommandProgressError::TooLarge)?;
            } else {
                return Err(CommandProgressError::TooLarge);
            }
            snapshot.capture_limited = true;
        }
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn encoded_len(&self) -> Result<usize, CommandProgressError> {
        serde_json::to_vec(self)
            .map(|bytes| bytes.len())
            .map_err(|_| CommandProgressError::Encoding)
    }

    fn validate_header(&self) -> Result<(), CommandProgressError> {
        if self.schema_version != COMMAND_PROGRESS_SCHEMA_VERSION
            || self.canonical_id.is_nil()
            || self.revision == 0
            || self.revision > MAX_COMMAND_PROGRESS_INTEGER
            || self.entries.len() > MAX_COMMAND_PROGRESS_ENTRIES
            || usize::try_from(self.omitted_entries)
                .map_err(|_| CommandProgressError::TooLarge)?
                .checked_add(self.entries.len())
                .is_none_or(|count| count > MAX_COMMAND_PROGRESS_IDENTITIES)
            || (self.omitted_entries > 0 && !self.capture_limited)
        {
            return Err(CommandProgressError::InvalidInput);
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), CommandProgressError> {
        self.validate_header()?;
        validate_entries(&self.entries)?;
        if self.encoded_len()? > MAX_COMMAND_PROGRESS_SNAPSHOT_BYTES {
            return Err(CommandProgressError::TooLarge);
        }
        Ok(())
    }
}

fn validate_entries(entries: &[CommandProgressEntry]) -> Result<(), CommandProgressError> {
    if entries.len() > MAX_COMMAND_PROGRESS_IDENTITIES {
        return Err(CommandProgressError::TooLarge);
    }
    let mut ids = BTreeSet::new();
    for entry in entries {
        if entry.record_id.is_nil() || !ids.insert(entry.record_id) {
            return Err(CommandProgressError::InvalidInput);
        }
        entry.content.validate()?;
    }
    Ok(())
}

fn truncate_prefix(text: &mut String, maximum: usize) {
    let mut end = maximum.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

fn truncate_suffix(text: &mut String, maximum: usize) {
    let mut start = text.len().saturating_sub(maximum);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.drain(..start);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(id: u128) -> CommandProgressEntry {
        CommandProgressEntry {
            record_id: Uuid::from_u128(id),
            phase: CommandProgressPhase::InProgress,
            content: CommandProgressContent {
                command: "synthetic command".into(),
                command_original_bytes: 17,
                command_truncated: false,
                working_directory: "C:\\synthetic".into(),
                directory_original_bytes: 12,
                directory_truncated: false,
                output_tail: None,
                output_original_bytes: None,
                output_omitted_bytes: 0,
            },
        }
    }
    #[test]
    fn snapshot_is_closed_validated_and_debug_redacted() {
        let snapshot =
            CommandProgressSnapshot::bounded(Uuid::from_u128(99), 1, vec![entry(1)], 0, false)
                .expect("bounded");
        let value = serde_json::to_value(&snapshot).expect("serialized");
        assert_eq!(
            serde_json::from_value::<CommandProgressSnapshot>(value.clone()).expect("decoded"),
            snapshot
        );
        let mut extra = value;
        extra["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<CommandProgressSnapshot>(extra).is_err());
        assert!(!format!("{snapshot:?} {:?}", snapshot.entries).contains("synthetic"));
    }
    #[test]
    fn recent_entries_omit_old_payload_without_reusing_ids() {
        let snapshot = CommandProgressSnapshot::bounded(
            Uuid::from_u128(99),
            2,
            (1..=12).map(entry).collect(),
            0,
            false,
        )
        .expect("bounded");
        assert_eq!(snapshot.entries.len(), 8);
        assert_eq!(snapshot.omitted_entries, 4);
        assert!(snapshot.capture_limited);
        assert_eq!(snapshot.entries[0].record_id, Uuid::from_u128(5));
        assert!(
            CommandProgressSnapshot::bounded(
                Uuid::from_u128(99),
                1,
                vec![entry(1), entry(1)],
                0,
                false
            )
            .is_err()
        );
    }
    #[test]
    fn escaping_budget_keeps_utf8_prefixes_and_output_suffix_counts() {
        let mut entries = Vec::new();
        for id in 1..=4 {
            let mut row = entry(id);
            row.content.command = "\0".repeat(8 * 1024);
            row.content.command_original_bytes = 8 * 1024;
            row.content.working_directory = "\u{1}".repeat(32 * 1024);
            row.content.directory_original_bytes = 32 * 1024;
            row.content.output_tail = Some("é".repeat(8 * 1024));
            row.content.output_original_bytes = Some(16 * 1024);
            entries.push(row);
        }
        let snapshot = CommandProgressSnapshot::bounded(Uuid::from_u128(99), 3, entries, 0, false)
            .expect("bounded escaped");
        assert!(snapshot.capture_limited);
        assert!(snapshot.encoded_len().expect("size") <= MAX_COMMAND_PROGRESS_SNAPSHOT_BYTES);
        for row in snapshot.entries {
            row.content.validate().expect("counts");
            assert!(
                row.content
                    .output_tail
                    .as_deref()
                    .is_some_and(|text| "é".repeat(8 * 1024).ends_with(text))
            );
        }
    }
    #[test]
    fn invalid_counts_ids_revisions_and_unavailable_output_are_refused() {
        let mut row = entry(1);
        row.content.output_tail = Some(String::new());
        assert!(row.content.validate().is_err());
        let mut row = entry(1);
        row.content.command_truncated = true;
        assert!(row.content.validate().is_err());
        for (id, revision) in [
            (Uuid::nil(), 1),
            (Uuid::from_u128(99), 0),
            (Uuid::from_u128(99), MAX_COMMAND_PROGRESS_INTEGER + 1),
        ] {
            assert!(CommandProgressSnapshot::bounded(id, revision, vec![], 0, false).is_err());
        }
        let snapshot = CommandProgressSnapshot::bounded(Uuid::from_u128(99), 4, vec![], 0, false)
            .expect("explicit empty");
        assert!(snapshot.entries.is_empty());
        snapshot.validate().expect("valid retirement");
    }
}
