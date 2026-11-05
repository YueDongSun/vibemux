//! Optional bounded command observations, independent of terminal tool capture.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{CommandProgress, SessionMachine, SessionTransition, session::identifier};
use crate::dispatch::Sha256Digest;

const MAX_PROGRESS_IDENTITIES: usize = 64;
const MAX_PROGRESS_BYTES: usize = 256 * 1024;
const MAX_COMMAND_BYTES: usize = 8 * 1024;
const MAX_DIRECTORY_BYTES: usize = 32 * 1024;
const MAX_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Default)]
pub(super) struct CommandProgressState {
    // Tombstones retain cumulative identity accounting after payload retirement.
    entries: BTreeMap<String, ProgressEntry>,
    bytes: usize,
    revision: u64,
    pub(super) limited: bool,
}

struct ProgressEntry {
    // Compare complete start metadata, including discarded tails. Deltas are
    // ordered occurrences and never use a content fingerprint for deduplication.
    start_fingerprint: Sha256Digest,
    snapshot: Option<CommandProgress>,
}

impl CommandProgressState {
    pub(super) fn retire(&mut self, id: &str) {
        if let Some(entry) = self.entries.get_mut(id)
            && let Some(snapshot) = entry.snapshot.take()
        {
            self.bytes -= retained_bytes(&snapshot);
        }
    }

    pub(super) fn text_collision(&mut self, id: &str) {
        if self.entries.contains_key(id) {
            self.limited = true;
            self.retire(id);
        }
    }

    fn next_revision(&mut self) -> Option<u64> {
        let revision = self.revision.checked_add(1)?;
        if revision > MAX_SAFE_INTEGER {
            return None;
        }
        self.revision = revision;
        Some(revision)
    }

    fn transition(&self, snapshot: Option<CommandProgress>) -> SessionTransition {
        SessionTransition {
            command_progress: snapshot.into_iter().collect(),
            command_progress_limited: self.limited,
            ..Default::default()
        }
    }

    fn limit(&mut self) -> SessionTransition {
        self.limited = true;
        self.transition(None)
    }
}

impl SessionMachine {
    pub(super) fn malformed_command_progress(&mut self) -> SessionTransition {
        self.codex.command_progress.limit()
    }

    pub(super) fn codex_command_started(&mut self, item: &Value) -> SessionTransition {
        let Ok(id) = identifier(item.get("id")) else {
            return self.malformed_command_progress();
        };
        if self.codex.tools.contains_id(&id) {
            return self.codex.command_progress.transition(None);
        }
        if self.codex.items.contains_key(&id) {
            self.codex.command_progress.text_collision(&id);
            return self.malformed_command_progress();
        }
        let Some(active) = self.active.as_ref() else {
            return self.malformed_command_progress();
        };
        let Some((command, directory, output)) = start_fields(item) else {
            self.codex.command_progress.retire(&id);
            return self.malformed_command_progress();
        };
        let fingerprint =
            Sha256Digest::of(json!([command, directory, output]).to_string().as_bytes());
        let state = &mut self.codex.command_progress;
        if let Some(previous) = state.entries.get(&id) {
            if previous.start_fingerprint != fingerprint {
                state.retire(&id);
                return state.limit();
            }
            return state.transition(None);
        }
        if state.entries.len() >= MAX_PROGRESS_IDENTITIES {
            return state.limit();
        }
        let Some(mut snapshot) = start_snapshot(
            active.summary.request_id,
            active.summary.turn_id,
            id.clone(),
            command,
            directory,
            output,
        ) else {
            return state.limit();
        };
        // Allocate the identity once even when its retained payload cannot fit.
        state.entries.insert(
            id.clone(),
            ProgressEntry {
                start_fingerprint: fingerprint,
                snapshot: None,
            },
        );
        let bytes = retained_bytes(&snapshot);
        if state
            .bytes
            .checked_add(bytes)
            .is_none_or(|sum| sum > MAX_PROGRESS_BYTES)
        {
            return state.limit();
        }
        let Some(revision) = state.next_revision() else {
            return state.limit();
        };
        snapshot.revision = revision;
        state.bytes += bytes;
        if let Some(entry) = state.entries.get_mut(&id) {
            entry.snapshot = Some(snapshot.clone());
        }
        state.transition(Some(snapshot))
    }

    pub(super) fn codex_command_delta(&mut self, params: &Value) -> SessionTransition {
        let Ok(id) = identifier(params.get("itemId")) else {
            return self.malformed_command_progress();
        };
        if self.codex.tools.contains_id(&id) {
            return self.codex.command_progress.transition(None);
        }
        let state = &mut self.codex.command_progress;
        let Some(entry) = state.entries.get(&id) else {
            return state.limit();
        };
        let Some(previous) = entry.snapshot.as_ref() else {
            return state.transition(None);
        };
        let Some(delta) = params.get("delta").and_then(Value::as_str) else {
            state.retire(&id);
            return state.limit();
        };
        let Some(delta_bytes) = checked_bytes(delta) else {
            state.retire(&id);
            return state.limit();
        };
        let Some(original_bytes) = previous
            .output_original_bytes
            .unwrap_or(0)
            .checked_add(delta_bytes)
            .filter(|count| *count <= MAX_SAFE_INTEGER)
        else {
            state.retire(&id);
            return state.limit();
        };
        let mut snapshot = previous.clone();
        let old_bytes = retained_bytes(previous);
        let output = if delta.len() >= MAX_OUTPUT_BYTES {
            suffix(delta, MAX_OUTPUT_BYTES).to_owned()
        } else {
            let mut output = snapshot.output_tail.take().unwrap_or_default();
            output.push_str(delta);
            suffix(&output, MAX_OUTPUT_BYTES).to_owned()
        };
        let Some(retained_output_bytes) = checked_bytes(&output) else {
            state.retire(&id);
            return state.limit();
        };
        snapshot.output_original_bytes = Some(original_bytes);
        snapshot.output_omitted_bytes = original_bytes - retained_output_bytes;
        snapshot.output_tail = Some(output);
        let bytes = retained_bytes(&snapshot);
        let Some(total) = state
            .bytes
            .checked_sub(old_bytes)
            .and_then(|sum| sum.checked_add(bytes))
            .filter(|sum| *sum <= MAX_PROGRESS_BYTES)
        else {
            state.retire(&id);
            return state.limit();
        };
        let Some(revision) = state.next_revision() else {
            state.retire(&id);
            return state.limit();
        };
        snapshot.revision = revision;
        state.bytes = total;
        if let Some(entry) = state.entries.get_mut(&id) {
            entry.snapshot = Some(snapshot.clone());
        }
        state.transition(Some(snapshot))
    }
}

fn start_fields(item: &Value) -> Option<(&str, &str, Option<&str>)> {
    if item.get("status")?.as_str()? != "inProgress" {
        return None;
    }
    let output = match item.get("aggregatedOutput")? {
        Value::Null => None,
        Value::String(output) => Some(output.as_str()),
        _ => return None,
    };
    Some((
        item.get("command")?.as_str()?,
        item.get("cwd")?.as_str()?,
        output,
    ))
}

fn start_snapshot(
    request_id: uuid::Uuid,
    turn_id: uuid::Uuid,
    call_id: String,
    command: &str,
    directory: &str,
    output: Option<&str>,
) -> Option<CommandProgress> {
    let command_original_bytes = checked_bytes(command)?;
    let directory_original_bytes = checked_bytes(directory)?;
    let output_original_bytes = match output {
        Some(output) => Some(checked_bytes(output)?),
        None => None,
    };
    let output_tail = output.map(|output| suffix(output, MAX_OUTPUT_BYTES).to_owned());
    let retained_output_bytes = match output_tail.as_deref() {
        Some(output) => checked_bytes(output)?,
        None => 0,
    };
    Some(CommandProgress {
        request_id,
        turn_id,
        call_id,
        revision: 0,
        command: prefix(command, MAX_COMMAND_BYTES).to_owned(),
        command_original_bytes,
        command_truncated: command.len() > MAX_COMMAND_BYTES,
        working_directory: prefix(directory, MAX_DIRECTORY_BYTES).to_owned(),
        directory_original_bytes,
        directory_truncated: directory.len() > MAX_DIRECTORY_BYTES,
        output_omitted_bytes: output_original_bytes.unwrap_or(0) - retained_output_bytes,
        output_tail,
        output_original_bytes,
    })
}

fn checked_bytes(text: &str) -> Option<u64> {
    u64::try_from(text.len())
        .ok()
        .filter(|count| *count <= MAX_SAFE_INTEGER)
}

fn retained_bytes(snapshot: &CommandProgress) -> usize {
    snapshot.command.len()
        + snapshot.working_directory.len()
        + snapshot.output_tail.as_ref().map_or(0, String::len)
}

fn prefix(text: &str, maximum: usize) -> &str {
    let mut end = text.len().min(maximum);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn suffix(text: &str, maximum: usize) -> &str {
    let mut start = text.len().saturating_sub(maximum);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_session::{SessionConfig, SessionLimits, SessionTarget};
    use uuid::Uuid;

    #[test]
    fn revision_stops_at_the_javascript_safe_integer_ceiling() {
        let mut state = CommandProgressState {
            revision: MAX_SAFE_INTEGER - 1,
            ..Default::default()
        };
        assert_eq!(state.next_revision(), Some(MAX_SAFE_INTEGER));
        assert_eq!(state.next_revision(), None);
        assert_eq!(state.revision, MAX_SAFE_INTEGER);
        state.revision = u64::MAX;
        assert_eq!(state.next_revision(), None);
    }

    #[test]
    fn output_count_overflow_retires_only_the_optional_payload() {
        let mut machine = SessionMachine::new(
            SessionConfig {
                target: SessionTarget::Codex {
                    resume_thread_id: None,
                },
                generation_id: Uuid::from_u128(1),
                working_directory: if cfg!(windows) {
                    "C:/synthetic"
                } else {
                    "/synthetic"
                }
                .into(),
                model: None,
            },
            SessionLimits::default(),
        )
        .expect("machine");
        let mut snapshot = start_snapshot(
            Uuid::from_u128(2),
            Uuid::from_u128(3),
            "call".into(),
            "command",
            "directory",
            Some("x"),
        )
        .expect("snapshot");
        snapshot.output_original_bytes = Some(MAX_SAFE_INTEGER);
        snapshot.output_omitted_bytes = MAX_SAFE_INTEGER - 1;
        machine.codex.command_progress.bytes = retained_bytes(&snapshot);
        machine.codex.command_progress.entries.insert(
            "call".into(),
            ProgressEntry {
                start_fingerprint: Sha256Digest::of(b"synthetic start"),
                snapshot: Some(snapshot),
            },
        );
        let transition = machine.codex_command_delta(&json!({"itemId":"call","delta":"x"}));
        assert!(transition.command_progress_limited && transition.command_progress.is_empty());
        assert!(!transition.tool_capture_incomplete);
        assert_eq!(machine.codex.command_progress.bytes, 0);
        assert!(
            machine.codex.command_progress.entries["call"]
                .snapshot
                .is_none()
        );
        assert_eq!(machine.codex.command_progress.entries.len(), 1);
    }
}
