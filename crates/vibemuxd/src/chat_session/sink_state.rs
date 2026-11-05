//! Shared pure per-turn output bookkeeping for chat runtime sinks.

use std::collections::BTreeMap;
use uuid::Uuid;
use vibemux_harness::chat_session::{AssistantText, CommandProgress, ToolObservation};
use vibemux_types::chat::{MAX_CHAT_BYTES, MAX_CHAT_RECORD_BYTES};
use vibemux_types::chat_command_progress::{
    CommandProgressContent, CommandProgressEntry, CommandProgressPhase, CommandProgressSnapshot,
    MAX_COMMAND_PROGRESS_IDENTITIES, MAX_COMMAND_PROGRESS_INTEGER,
    MAX_COMMAND_PROGRESS_MEMORY_BYTES,
};
use vibemux_types::chat_tool::{MAX_TOOL_RECORD_BYTES, MAX_TOOL_RECORDS, ToolRecord};

use super::RuntimeError;

/// The bounded private output state of the one active turn or attempt. A
/// sink holds at most one; the identity fields pin it to one request.
pub(super) struct ActiveOutput {
    pub(super) request_id: Uuid,
    pub(super) turn_id: Uuid,
    pub(super) prompt_bytes: usize,
    pub(super) acknowledged_vendor_turn: Option<String>,
    pub(super) latest: Option<AssistantText>,
    pub(super) record_id: Uuid,
    pub(super) flushed: bool,
    pub(super) content_available: bool,
    tools: BTreeMap<String, ToolWrite>,
    tool_bytes: usize,
    progress: BTreeMap<String, ProgressWrite>,
    progress_bytes: usize,
    progress_revision: u64,
    progress_limited: bool,
}

struct ProgressWrite {
    record_id: Uuid,
    source_revision: u64,
    observed_revision: u64,
    content: Option<CommandProgressContent>,
    phase: CommandProgressPhase,
    terminal_reserved: bool,
}

/// Copy under the sink lock; JSON sizing and truncation happen after release.
pub(super) struct CommandProgressRead {
    canonical_id: Uuid,
    revision: u64,
    entries: Vec<CommandProgressEntry>,
    omitted_entries: u32,
    limited: bool,
}

impl CommandProgressRead {
    pub(super) fn build(self) -> Result<CommandProgressSnapshot, RuntimeError> {
        CommandProgressSnapshot::bounded(
            self.canonical_id,
            self.revision,
            self.entries,
            self.omitted_entries,
            self.limited,
        )
        .map_err(|_| RuntimeError::ContentUnavailable)
    }
}

// Pending bodies and their IDs survive a dropped append waiter. Nothing here
// grants execution or supplies a second append after an unknown response.
struct ToolWrite {
    record_id: Uuid,
    body: String,
    persisted: bool,
}

impl ActiveOutput {
    pub(super) fn new(request_id: Uuid, turn_id: Uuid, prompt_bytes: usize) -> Self {
        Self {
            request_id,
            turn_id,
            prompt_bytes,
            acknowledged_vendor_turn: None,
            latest: None,
            record_id: Uuid::new_v4(),
            flushed: false,
            content_available: true,
            tools: BTreeMap::new(),
            tool_bytes: 0,
            progress: BTreeMap::new(),
            progress_bytes: 0,
            progress_revision: 0,
            progress_limited: false,
        }
    }

    /// Bounded, private, replacement-only merge. A snapshot must match the
    /// active identity, respect the shared record limits, and advance the
    /// revision monotonically; a violated limit leaves content unavailable.
    pub(super) fn merge_snapshot(&mut self, text: AssistantText) -> Result<(), RuntimeError> {
        if self.request_id != text.request_id || self.turn_id != text.turn_id || self.flushed {
            return Err(RuntimeError::WrongTurn);
        }
        if text.text.len() as u64 > MAX_CHAT_RECORD_BYTES
            || text.original_bytes < text.text.len()
            || text.original_bytes as u64 > MAX_CHAT_BYTES
            || text.truncated != (text.original_bytes > text.text.len())
            || text.revision == 0
        {
            self.content_available = false;
            return Err(RuntimeError::ContentUnavailable);
        }
        if let Some(previous) = &self.latest {
            if text.revision < previous.revision
                || (text.revision == previous.revision && text != *previous)
            {
                return Err(RuntimeError::WrongTurn);
            }
            if text.revision == previous.revision {
                return Ok(());
            }
        }
        self.latest = Some(text);
        Ok(())
    }

    /// Records the outcome of one append attempt; the fixed `record_id` keeps
    /// an unknown append response safe to reconcile without another record.
    pub(super) fn record_flush(&mut self, persisted: bool, complete: bool) {
        self.flushed = persisted;
        self.content_available = complete && persisted;
    }

    pub(super) fn output_available(&self) -> bool {
        self.content_available && self.tools.values().all(|write| write.persisted)
    }

    fn advance_progress(&mut self) -> Result<u64, RuntimeError> {
        let revision = self
            .progress_revision
            .checked_add(1)
            .filter(|revision| *revision <= MAX_COMMAND_PROGRESS_INTEGER)
            .ok_or(RuntimeError::ContentUnavailable)?;
        self.progress_revision = revision;
        Ok(revision)
    }

    fn limit_progress(&mut self) -> Result<(), RuntimeError> {
        if !self.progress_limited {
            self.advance_progress()?;
            self.progress_limited = true;
        }
        Ok(())
    }

    /// Optional memory observations cannot affect terminal completeness.
    pub(super) fn merge_command_progress(
        &mut self,
        request_id: Uuid,
        turn_id: Uuid,
        updates: Vec<CommandProgress>,
        limited: bool,
    ) -> Result<(), RuntimeError> {
        if self.request_id != request_id
            || self.turn_id != turn_id
            || self.acknowledged_vendor_turn.is_none()
            || self.flushed
        {
            return Err(RuntimeError::WrongTurn);
        }
        if limited {
            self.limit_progress()?;
        }
        if updates.len() > MAX_COMMAND_PROGRESS_IDENTITIES {
            self.limit_progress()?;
        }
        for update in updates.into_iter().take(MAX_COMMAND_PROGRESS_IDENTITIES) {
            if update.request_id != request_id || update.turn_id != turn_id {
                self.limit_progress()?;
                continue;
            }
            // The terminal ledger is also a tombstone for calls that never
            // had progress, including terminal payloads trimmed elsewhere.
            if self.tools.contains_key(&update.call_id)
                || self
                    .progress
                    .get(&update.call_id)
                    .is_some_and(|row| row.terminal_reserved)
            {
                continue;
            }
            let content = CommandProgressContent {
                command: update.command,
                command_original_bytes: update.command_original_bytes,
                command_truncated: update.command_truncated,
                working_directory: update.working_directory,
                directory_original_bytes: update.directory_original_bytes,
                directory_truncated: update.directory_truncated,
                output_tail: update.output_tail,
                output_original_bytes: update.output_original_bytes,
                output_omitted_bytes: update.output_omitted_bytes,
            };
            if update.call_id.is_empty()
                || update.call_id.len() > 512
                || update.call_id.chars().any(char::is_control)
                || update.revision == 0
                || update.revision > MAX_COMMAND_PROGRESS_INTEGER
                || content.validate().is_err()
            {
                self.limit_progress()?;
                continue;
            }
            if let Some(previous) = self.progress.get(&update.call_id) {
                if update.revision < previous.source_revision {
                    continue;
                }
                if update.revision == previous.source_revision {
                    if previous.content.as_ref() != Some(&content) {
                        self.limit_progress()?;
                    }
                    continue;
                }
            } else if self.progress.len() >= MAX_COMMAND_PROGRESS_IDENTITIES {
                self.limit_progress()?;
                continue;
            }
            let bytes = content
                .retained_bytes()
                .map_err(|_| RuntimeError::ContentUnavailable)?;
            let old_bytes = self
                .progress
                .get(&update.call_id)
                .and_then(|row| row.content.as_ref())
                .map_or(Ok(0), CommandProgressContent::retained_bytes)
                .map_err(|_| RuntimeError::ContentUnavailable)?;
            // Refuse only the optional replacement. Identity accounting is
            // cumulative and never borrows the terminal ledger's capacity.
            let retained = self
                .progress_bytes
                .saturating_sub(old_bytes)
                .checked_add(bytes);
            if retained.is_none_or(|bytes| bytes > MAX_COMMAND_PROGRESS_MEMORY_BYTES) {
                self.limit_progress()?;
                continue;
            }
            let revision = self.advance_progress()?;
            self.progress_limited |= content.command_truncated
                || content.directory_truncated
                || content.output_omitted_bytes > 0;
            let record_id = self
                .progress
                .get(&update.call_id)
                .map_or_else(Uuid::new_v4, |row| row.record_id);
            self.progress.insert(
                update.call_id,
                ProgressWrite {
                    record_id,
                    source_revision: update.revision,
                    observed_revision: revision,
                    content: Some(content),
                    phase: CommandProgressPhase::InProgress,
                    terminal_reserved: false,
                },
            );
            self.progress_bytes = retained.ok_or(RuntimeError::ContentUnavailable)?;
        }
        Ok(())
    }

    pub(super) fn command_progress(&self) -> Option<CommandProgressRead> {
        if self.progress_revision == 0 {
            return None;
        }
        let mut rows: Vec<_> = self
            .progress
            .values()
            .filter_map(|row| {
                row.content.as_ref().map(|content| {
                    (
                        row.observed_revision,
                        CommandProgressEntry {
                            record_id: row.record_id,
                            phase: row.phase,
                            content: content.clone(),
                        },
                    )
                })
            })
            .collect();
        rows.sort_by_key(|(revision, _)| *revision);
        let omitted_entries = self.progress.len().saturating_sub(rows.len()) as u32;
        Some(CommandProgressRead {
            canonical_id: self.turn_id,
            revision: self.progress_revision,
            entries: rows.into_iter().map(|(_, row)| row).collect(),
            omitted_entries,
            limited: self.progress_limited || omitted_entries > 0,
        })
    }

    /// Reserve exactly one immutable private body before entering the writer.
    /// A matching persisted replay is inert; a pending replay stays uncertain.
    pub(super) fn reserve_tool(
        &mut self,
        observation: &ToolObservation,
    ) -> Result<Option<(Uuid, String)>, RuntimeError> {
        if observation.request_id != self.request_id
            || observation.turn_id != self.turn_id
            || self.acknowledged_vendor_turn.is_none()
            || self.flushed
        {
            return Err(RuntimeError::WrongTurn);
        }
        let body = observation
            .record
            .to_private_text()
            .map_err(|_| RuntimeError::ContentUnavailable)?;
        if let Some(previous) = self.tools.get(observation.record.call_id()) {
            if previous.body != body {
                return Err(RuntimeError::WrongTurn);
            }
            return if previous.persisted {
                Ok(None)
            } else {
                Err(RuntimeError::ContentUnavailable)
            };
        }
        if self.tools.len() >= MAX_TOOL_RECORDS
            || self.tool_bytes.saturating_add(body.len()) > MAX_TOOL_RECORD_BYTES
        {
            self.content_available = false;
            return Err(RuntimeError::ContentUnavailable);
        }
        self.content_available &= !observation.record.is_truncated();
        let record_id = self
            .progress
            .get(observation.record.call_id())
            .map_or_else(Uuid::new_v4, |row| row.record_id);
        // Freeze the terminal identity before the append await. Progress
        // revisions are optional, so exhaustion cannot refuse a final body.
        let revision = if self.progress.contains_key(observation.record.call_id()) {
            self.advance_progress().ok()
        } else {
            None
        };
        if let Some(row) = self.progress.get_mut(observation.record.call_id()) {
            row.terminal_reserved = true;
        }
        // A terminal write must proceed even when the optional observation
        // counter is exhausted. Only the hidden tombstone changes then: the
        // same published revision must retain byte-identical snapshot data.
        if let Some(revision) = revision
            && let Some(row) = self.progress.get_mut(observation.record.call_id())
        {
            row.phase = CommandProgressPhase::Finalizing;
            row.observed_revision = revision;
            if matches!(&observation.record, ToolRecord::FileChange(_)) {
                if let Some(content) = row.content.take() {
                    self.progress_bytes = self
                        .progress_bytes
                        .saturating_sub(content.retained_bytes().unwrap_or(0));
                }
            } else if let ToolRecord::Command(record) = &observation.record
                && let Some(content) = &mut row.content
            {
                self.progress_bytes = self
                    .progress_bytes
                    .saturating_sub(content.retained_bytes().unwrap_or(0));
                content.command = record.command.clone();
                content.command_original_bytes = record.command_original_bytes;
                content.command_truncated = record.command_truncated;
                content.working_directory = record.working_directory.clone();
                content.directory_original_bytes = record.working_directory.len() as u64;
                content.directory_truncated = false;
                if !record.output_truncated {
                    content.output_tail = record.output.clone();
                    content.output_original_bytes = record.output_original_bytes;
                    content.output_omitted_bytes = 0;
                }
                let bytes = content
                    .retained_bytes()
                    .unwrap_or(MAX_COMMAND_PROGRESS_MEMORY_BYTES);
                if self.progress_bytes.saturating_add(bytes) <= MAX_COMMAND_PROGRESS_MEMORY_BYTES {
                    self.progress_bytes += bytes;
                } else {
                    row.content = None;
                    self.progress_limited = true;
                }
            }
        }
        self.tool_bytes += body.len();
        self.tools.insert(
            observation.record.call_id().to_owned(),
            ToolWrite {
                record_id,
                body: body.clone(),
                persisted: false,
            },
        );
        Ok(Some((record_id, body)))
    }

    pub(super) fn complete_tool(
        &mut self,
        call_id: &str,
        record_id: Uuid,
        persisted: bool,
    ) -> Result<(), RuntimeError> {
        let write = self
            .tools
            .get_mut(call_id)
            .filter(|write| write.record_id == record_id)
            .ok_or(RuntimeError::WrongTurn)?;
        write.persisted = persisted;
        self.content_available &= persisted;
        let revision = if self.progress.contains_key(call_id) {
            self.advance_progress().ok()
        } else {
            None
        };
        if let Some(revision) = revision
            && let Some(row) = self.progress.get_mut(call_id)
        {
            row.phase = if persisted {
                CommandProgressPhase::Recorded
            } else {
                CommandProgressPhase::Unconfirmed
            };
            row.observed_revision = revision;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use vibemux_types::chat_tool::{CommandToolRecord, CommandToolStatus, ToolRecord};

    fn tool(active: &ActiveOutput, id: &str) -> ToolObservation {
        ToolObservation {
            request_id: active.request_id,
            turn_id: active.turn_id,
            record: ToolRecord::Command(
                CommandToolRecord::from_observation(
                    id.to_owned(),
                    CommandToolStatus::Completed,
                    "private command".to_owned(),
                    "C:\\private".to_owned(),
                    Some("output".to_owned()),
                    Some(0),
                    None,
                )
                .expect("record"),
            ),
        }
    }
    fn new_active() -> ActiveOutput {
        let mut active = ActiveOutput::new(Uuid::new_v4(), Uuid::new_v4(), 4);
        active.acknowledged_vendor_turn = Some("vendor_turn".to_owned());
        active
    }

    fn progress(active: &ActiveOutput, id: &str, revision: u64) -> CommandProgress {
        CommandProgress {
            request_id: active.request_id,
            turn_id: active.turn_id,
            call_id: id.to_owned(),
            revision,
            command: "provisional".to_owned(),
            command_original_bytes: 11,
            command_truncated: false,
            working_directory: "C:\\private".to_owned(),
            directory_original_bytes: 10,
            directory_truncated: false,
            output_tail: Some(format!("output {revision}")),
            output_original_bytes: Some(format!("output {revision}").len() as u64),
            output_omitted_bytes: 0,
        }
    }

    fn merge(active: &mut ActiveOutput, update: CommandProgress) {
        active
            .merge_command_progress(active.request_id, active.turn_id, vec![update], false)
            .expect("progress");
    }

    fn view(active: &ActiveOutput) -> CommandProgressSnapshot {
        active
            .command_progress()
            .expect("parts")
            .build()
            .expect("bounded snapshot")
    }

    #[test]
    fn progress_updates_and_changed_terminal_metadata_keep_one_generic_tool_identity() {
        let mut active = new_active();
        let update = progress(&active, "call", 1);
        merge(&mut active, update);
        let first = view(&active);
        let record_id = first.entries[0].record_id;
        let update = progress(&active, "call", 2);
        merge(&mut active, update.clone());
        let second = view(&active);
        assert_eq!(second.entries[0].record_id, record_id);
        assert!(second.revision > first.revision);
        merge(&mut active, update);
        assert_eq!(view(&active), second, "exact duplicate is inert");
        assert!(
            active.output_available(),
            "progress is not a pending durable write"
        );
        let observation = tool(&active, "call");
        let (id, body) = active
            .reserve_tool(&observation)
            .expect("terminal")
            .expect("one reservation");
        assert_eq!(id, record_id);
        assert_eq!(
            body,
            observation
                .record
                .to_private_text()
                .expect("original codec")
        );
        let finalizing = view(&active);
        assert_eq!(
            finalizing.entries[0].phase,
            CommandProgressPhase::Finalizing
        );
        assert_eq!(finalizing.entries[0].content.command, "private command");
        assert_eq!(
            active.reserve_tool(&observation),
            Err(RuntimeError::ContentUnavailable)
        );
        let late = progress(&active, "call", 3);
        merge(&mut active, late);
        assert_eq!(
            view(&active),
            finalizing,
            "reserved terminal cannot be resurrected"
        );
        active
            .complete_tool("call", id, true)
            .expect("known append");
        assert_eq!(
            view(&active).entries[0].phase,
            CommandProgressPhase::Recorded
        );
        assert_eq!(active.reserve_tool(&observation), Ok(None));
        assert!(active.output_available());
    }

    #[test]
    fn command_progress_to_file_handoff_retires_payload_without_freeing_identity() {
        use vibemux_types::chat_file_tool::{FileChangeToolRecord, FileChangeToolStatus};
        let mut active = new_active();
        let update = progress(&active, "call", 1);
        merge(&mut active, update);
        let id = view(&active).entries[0].record_id;
        let observation = ToolObservation {
            request_id: active.request_id,
            turn_id: active.turn_id,
            record: ToolRecord::FileChange(
                FileChangeToolRecord::from_observation(
                    "call".to_owned(),
                    FileChangeToolStatus::Completed,
                    vec![],
                )
                .expect("file"),
            ),
        };
        assert_eq!(
            active
                .reserve_tool(&observation)
                .expect("reserve")
                .expect("new")
                .0,
            id
        );
        assert!(
            view(&active).entries.is_empty(),
            "advancing empty snapshot retires command card"
        );
        assert_eq!(active.progress.len(), 1, "identity capacity stays consumed");
        assert_eq!(active.progress_bytes, 0);
        let revision = view(&active).revision;
        let late = progress(&active, "call", 2);
        merge(&mut active, late);
        assert_eq!(view(&active).revision, revision);
        active.complete_tool("call", id, true).expect("append");
        assert!(view(&active).entries.is_empty());
        assert_eq!(active.reserve_tool(&observation), Ok(None));
    }

    #[test]
    fn uncertain_and_failed_terminal_append_preserve_body_uuid_and_refuse_conflicting_replay() {
        let mut active = new_active();
        let update = progress(&active, "call", 1);
        merge(&mut active, update);
        let observation = tool(&active, "call");
        let (id, body) = active
            .reserve_tool(&observation)
            .expect("reserve")
            .expect("new");
        assert_eq!(
            active.reserve_tool(&observation),
            Err(RuntimeError::ContentUnavailable)
        );
        let mut conflict = observation.clone();
        if let ToolRecord::Command(record) = &mut conflict.record {
            record.working_directory = "C:\\changed".to_owned();
        }
        assert_eq!(active.reserve_tool(&conflict), Err(RuntimeError::WrongTurn));
        active
            .complete_tool("call", id, false)
            .expect("known failure");
        assert_eq!(
            view(&active).entries[0].phase,
            CommandProgressPhase::Unconfirmed
        );
        assert_eq!(
            active.reserve_tool(&observation),
            Err(RuntimeError::ContentUnavailable)
        );
        assert_eq!(active.tools["call"].record_id, id);
        assert_eq!(active.tools["call"].body, body);
        assert!(!active.output_available());
    }

    #[test]
    fn progress_capacity_loss_and_conflict_leave_all_terminal_capacity_available() {
        let mut active = new_active();
        for index in 0..MAX_COMMAND_PROGRESS_IDENTITIES {
            let update = progress(&active, &format!("progress_{index}"), 1);
            merge(&mut active, update);
        }
        let overflow = progress(&active, "overflow", 1);
        merge(&mut active, overflow);
        assert!(view(&active).capture_limited);
        assert!(active.tools.is_empty());
        assert!(active.output_available());
        let mut conflict = progress(&active, "progress_0", 1);
        conflict.command = "conflicting".into();
        merge(&mut active, conflict);
        assert_eq!(
            active.progress["progress_0"]
                .content
                .as_ref()
                .expect("retained")
                .command,
            "provisional"
        );
        for index in 0..MAX_TOOL_RECORDS {
            let observation = tool(&active, &format!("terminal_{index}"));
            let (id, _) = active
                .reserve_tool(&observation)
                .expect("independent capacity")
                .expect("new");
            active
                .complete_tool(observation.record.call_id(), id, true)
                .expect("persisted");
        }
        assert!(active.output_available());
        assert_eq!(active.tools.len(), MAX_TOOL_RECORDS);
    }

    #[test]
    fn progress_raw_budget_and_binding_are_independent_of_output_completeness() {
        let mut active = new_active();
        for index in 0..8 {
            let mut update = progress(&active, &format!("large_{index}"), 1);
            update.command = "x".repeat(8192);
            update.command_original_bytes = 8192;
            update.working_directory = "x".repeat(32768);
            update.directory_original_bytes = 32768;
            merge(&mut active, update);
        }
        assert!(active.progress_bytes <= MAX_COMMAND_PROGRESS_MEMORY_BYTES);
        assert!(view(&active).capture_limited);
        assert!(active.output_available());
        assert_eq!(
            active.merge_command_progress(Uuid::new_v4(), active.turn_id, vec![], false),
            Err(RuntimeError::WrongTurn)
        );
        let mut later = new_active();
        let same_call = progress(&later, "large_0", 1);
        merge(&mut later, same_call);
        assert_ne!(
            view(&later).entries[0].record_id,
            active.progress["large_0"].record_id
        );
        active.progress_revision = MAX_COMMAND_PROGRESS_INTEGER;
        let next = progress(&active, "large_0", 2);
        assert_eq!(
            active.merge_command_progress(active.request_id, active.turn_id, vec![next], false),
            Err(RuntimeError::ContentUnavailable)
        );
        assert!(active.output_available());
    }

    #[test]
    fn exhausted_progress_revision_freezes_wire_snapshot_but_allows_terminal_handoffs() {
        use vibemux_types::chat_file_tool::{FileChangeToolRecord, FileChangeToolStatus};
        for persisted in [true, false] {
            let mut active = new_active();
            for call_id in ["command", "file"] {
                let update = progress(&active, call_id, 1);
                merge(&mut active, update);
            }
            let command = tool(&active, "command");
            let file = ToolObservation {
                request_id: active.request_id,
                turn_id: active.turn_id,
                record: ToolRecord::FileChange(
                    FileChangeToolRecord::from_observation(
                        "file".to_owned(),
                        FileChangeToolStatus::Completed,
                        vec![],
                    )
                    .expect("file completion"),
                ),
            };
            active.progress_revision = MAX_COMMAND_PROGRESS_INTEGER;
            let before = serde_json::to_vec(&view(&active)).expect("initial wire snapshot");
            assert!(!view(&active).capture_limited);
            for observation in [command, file] {
                let call_id = observation.record.call_id();
                let reserved_id = active.progress[call_id].record_id;
                let original_body = observation.record.to_private_text().expect("original body");
                let (record_id, body) = active
                    .reserve_tool(&observation)
                    .expect("terminal survives progress counter exhaustion")
                    .expect("new reservation");
                assert_eq!(record_id, reserved_id);
                assert_eq!(body, original_body);
                assert!(active.progress[call_id].terminal_reserved);
                assert!(
                    !active.output_available(),
                    "pending append remains incomplete"
                );
                assert_eq!(
                    serde_json::to_vec(&view(&active)).expect("pending wire"),
                    before
                );
                active
                    .complete_tool(call_id, record_id, persisted)
                    .expect("known append outcome");
                assert_eq!(active.tools[call_id].record_id, reserved_id);
                assert_eq!(active.tools[call_id].body, original_body);
                assert_eq!(active.tools[call_id].persisted, persisted);
                assert_eq!(
                    serde_json::to_vec(&view(&active)).expect("completed wire"),
                    before
                );
                let late = progress(&active, call_id, 2);
                merge(&mut active, late);
                assert_eq!(
                    serde_json::to_vec(&view(&active)).expect("late wire"),
                    before
                );
                assert_eq!(
                    active.reserve_tool(&observation),
                    if persisted {
                        Ok(None)
                    } else {
                        Err(RuntimeError::ContentUnavailable)
                    }
                );
            }
            assert_eq!(active.tools.len(), 2);
            assert_eq!(active.output_available(), persisted);
            assert_eq!(
                active.merge_command_progress(active.request_id, active.turn_id, vec![], true),
                Err(RuntimeError::ContentUnavailable)
            );
            let mut invalid = progress(&active, "optional_invalid", 1);
            invalid.command_original_bytes = 0;
            assert_eq!(
                active.merge_command_progress(
                    active.request_id,
                    active.turn_id,
                    vec![invalid],
                    false
                ),
                Err(RuntimeError::ContentUnavailable)
            );
            let overflow = vec![
                progress(&active, "optional_overflow", 1);
                MAX_COMMAND_PROGRESS_IDENTITIES + 1
            ];
            assert_eq!(
                active.merge_command_progress(active.request_id, active.turn_id, overflow, false),
                Err(RuntimeError::ContentUnavailable)
            );
            assert_eq!(
                serde_json::to_vec(&view(&active)).expect("limited wire"),
                before
            );
            assert!(!active.progress_limited);
        }
    }

    #[test]
    fn mixed_tool_records_share_one_capacity_and_call_id_namespace() {
        use vibemux_types::chat_file_tool::{FileChangeToolRecord, FileChangeToolStatus};
        let mut active = new_active();
        for index in 0..MAX_TOOL_RECORDS {
            let id = format!("mixed_{index}");
            let observation = if index % 2 == 0 {
                tool(&active, &id)
            } else {
                ToolObservation {
                    request_id: active.request_id,
                    turn_id: active.turn_id,
                    record: ToolRecord::FileChange(
                        FileChangeToolRecord::from_observation(
                            id,
                            FileChangeToolStatus::Completed,
                            vec![],
                        )
                        .expect("file record"),
                    ),
                }
            };
            let (record_id, _) = active
                .reserve_tool(&observation)
                .expect("shared reservation")
                .expect("new");
            active
                .complete_tool(observation.record.call_id(), record_id, true)
                .expect("persisted");
        }
        let conflict = ToolObservation {
            request_id: active.request_id,
            turn_id: active.turn_id,
            record: ToolRecord::FileChange(
                FileChangeToolRecord::from_observation(
                    "mixed_0".to_owned(),
                    FileChangeToolStatus::Completed,
                    vec![],
                )
                .expect("file record"),
            ),
        };
        assert_eq!(active.reserve_tool(&conflict), Err(RuntimeError::WrongTurn));
        assert_eq!(
            active.reserve_tool(&tool(&active, "overflow")),
            Err(RuntimeError::ContentUnavailable)
        );
        assert_eq!(active.tools.len(), MAX_TOOL_RECORDS);
        assert!(!active.output_available());
    }

    #[test]
    fn pending_command_append_cannot_be_replaced_or_treated_as_complete() {
        let mut active = new_active();
        let tool = tool(&active, "command");
        let (id, body) = active
            .reserve_tool(&tool)
            .expect("reservation")
            .expect("new");
        assert!(!active.output_available());
        assert_eq!(
            active.reserve_tool(&tool),
            Err(RuntimeError::ContentUnavailable)
        );
        assert_eq!(active.tools.len(), 1);
        let retained = active.tools.get("command").expect("receipt");
        assert_eq!(retained.record_id, id);
        assert_eq!(retained.body, body);
        active
            .complete_tool("command", id, true)
            .expect("confirmed write");
        assert!(active.output_available());
        assert_eq!(active.reserve_tool(&tool), Ok(None));
    }

    #[test]
    fn command_record_capacity_and_failed_append_remain_incomplete() {
        let mut active = new_active();
        for index in 0..MAX_TOOL_RECORDS {
            let tool = tool(&active, &format!("command_{index}"));
            let (id, _) = active.reserve_tool(&tool).expect("reserve").expect("new");
            active
                .complete_tool(tool.record.call_id(), id, true)
                .expect("persisted");
        }
        assert!(active.output_available());
        assert_eq!(
            active.reserve_tool(&tool(&active, "overflow")),
            Err(RuntimeError::ContentUnavailable)
        );
        assert!(!active.output_available());
        assert_eq!(active.tools.len(), MAX_TOOL_RECORDS);
        let mut failed = new_active();
        let tool = tool(&failed, "failed_write");
        let (id, _) = failed.reserve_tool(&tool).expect("reserve").expect("new");
        failed
            .complete_tool(tool.record.call_id(), id, false)
            .expect("failed result");
        assert!(!failed.output_available());
        assert_eq!(
            failed.reserve_tool(&tool),
            Err(RuntimeError::ContentUnavailable)
        );
    }
}
