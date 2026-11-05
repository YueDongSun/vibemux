//! Bounded private export encoders. Formatting creates no files or authority.

use std::{
    collections::HashSet,
    fmt,
    io::{self, Write},
};
use time::format_description::well_known::Rfc3339;
use vibemux_types::chat::{
    ChatFailureCode, ChatRecordKind, ChatTurnOutcome, MAX_CHAT_BYTES, MAX_CHAT_RECORD_BYTES,
    MAX_CHAT_TITLE_BYTES,
};
use vibemuxd::chat_session::api::ChatContentError;

use super::{
    ConversationSnapshot, EXPORT_SCHEMA_VERSION, ExportError, ExportFormat, ExportKind,
    MAX_EXPORT_BYTES, MAX_EXPORT_CONTENT_BYTES, MAX_EXPORT_RECORDS, MAX_EXPORT_TURNS,
};

/// The serde contract carries the version; text labels keep independent
/// completion facts separate rather than deriving a successful outcome.
pub fn format_export(
    snapshot: &ConversationSnapshot,
    format: ExportFormat,
) -> Result<Vec<u8>, ExportError> {
    validate_snapshot(snapshot)?;
    let mut output = ExportBuffer::new(MAX_EXPORT_BYTES)?;
    match format {
        ExportFormat::Json => {
            if serde_json::to_writer_pretty(&mut output, snapshot).is_err() {
                return Err(if output.limit_hit {
                    ExportError::Limit
                } else {
                    ExportError::InvalidResponse
                });
            }
            output.write_all(b"\n").map_err(|_| ExportError::Limit)?;
        }
        ExportFormat::Text => write_text(&mut output, snapshot)?,
    }
    Ok(output.bytes)
}

fn validate_snapshot(snapshot: &ConversationSnapshot) -> Result<(), ExportError> {
    if snapshot.schema_version != EXPORT_SCHEMA_VERSION
        || snapshot.session.schema_version != 1
        || snapshot.session.generation == 0
        || snapshot.session.generation > i64::MAX as u64
        || snapshot.session.version == 0
        || snapshot.session.version > i64::MAX as u64
        || snapshot.record_sequence_cutoff != snapshot.session.last_record_sequence
    {
        return Err(ExportError::InvalidResponse);
    }
    if snapshot.records.len() > MAX_EXPORT_RECORDS
        || snapshot.turns.len() > MAX_EXPORT_TURNS
        || snapshot.session.record_count > MAX_EXPORT_RECORDS as u64
    {
        return Err(ExportError::Limit);
    }
    if u64::try_from(snapshot.records.len()).map_err(|_| ExportError::Limit)?
        != snapshot.session.record_count
        || snapshot.records.last().map_or(0, |record| record.sequence)
            != snapshot.record_sequence_cutoff
    {
        return Err(ExportError::InvalidResponse);
    }
    if snapshot
        .title
        .as_ref()
        .is_some_and(|title| title.len() > MAX_CHAT_TITLE_BYTES as usize)
    {
        return Err(ExportError::Limit);
    }
    if (snapshot.title.is_some() && (snapshot.title_error.is_some() || !snapshot.session.has_title))
        || (snapshot.session.has_title
            && snapshot.title.is_none()
            && snapshot.title_error.is_none())
        || (snapshot.session.content_purged
            && (snapshot.title.is_some() || snapshot.session.has_title))
    {
        return Err(ExportError::InvalidResponse);
    }
    let mut correlations = HashSet::with_capacity(snapshot.turns.len());
    let mut requests = HashSet::with_capacity(snapshot.turns.len());
    let mut required_partial = snapshot.session.content_purged || snapshot.title_error.is_some();
    for turn in &snapshot.turns {
        if turn.correlation_id.is_nil()
            || turn.request_id.is_nil()
            || turn.generation == 0
            || turn.generation > snapshot.session.generation
            || !correlations.insert(turn.correlation_id)
            || !requests.insert(turn.request_id)
        {
            return Err(ExportError::InvalidResponse);
        }
        required_partial |= !turn.output_complete;
    }
    if snapshot
        .session
        .active_request_id
        .is_some_and(|request| request.is_nil() || !requests.contains(&request))
    {
        return Err(ExportError::InvalidResponse);
    }
    let mut ids = HashSet::with_capacity(snapshot.records.len());
    let mut previous_sequence = 0;
    let mut retained_bytes = u64::try_from(snapshot.title.as_ref().map_or(0, String::len))
        .map_err(|_| ExportError::Limit)?;
    for record in &snapshot.records {
        if record.record_id.is_nil()
            || !ids.insert(record.record_id)
            || !correlations.contains(&record.correlation_id)
            || record.sequence == 0
            || record.sequence <= previous_sequence
            || record.sequence > snapshot.record_sequence_cutoff
        {
            return Err(ExportError::InvalidResponse);
        }
        previous_sequence = record.sequence;
        if record.original_byte_count > MAX_CHAT_BYTES
            || record
                .retained_byte_count
                .is_some_and(|count| count > MAX_CHAT_RECORD_BYTES)
        {
            return Err(ExportError::Limit);
        }
        if record
            .retained_byte_count
            .is_some_and(|count| count > record.original_byte_count)
            || record.truncated
                != record
                    .retained_byte_count
                    .is_some_and(|count| count < record.original_byte_count)
            || (snapshot.session.content_purged && record.text.is_some())
        {
            return Err(ExportError::InvalidResponse);
        }
        match (&record.text, record.content_error) {
            (Some(text), None) => {
                let bytes = u64::try_from(text.len()).map_err(|_| ExportError::Limit)?;
                if bytes > MAX_CHAT_RECORD_BYTES {
                    return Err(ExportError::Limit);
                }
                if record.retained_byte_count != Some(bytes) {
                    return Err(ExportError::InvalidResponse);
                }
            }
            (None, Some(_)) => {}
            _ => return Err(ExportError::InvalidResponse),
        }
        retained_bytes = retained_bytes
            .checked_add(record.retained_byte_count.unwrap_or(0))
            .ok_or(ExportError::Limit)?;
        if retained_bytes > MAX_EXPORT_CONTENT_BYTES {
            return Err(ExportError::Limit);
        }
        required_partial |= record.truncated || record.content_error.is_some();
    }
    // Record sequences are global event cursors: positive gaps are normal.
    if snapshot.partial != required_partial {
        return Err(ExportError::InvalidResponse);
    }
    Ok(())
}

/// Every escaped UTF-8 payload byte uses at most six output bytes. Each
/// record/turn has fixed bounded typed metadata below 2048 bytes; the session
/// envelope is below 4096 bytes. MAX_EXPORT_BYTES covers those three terms.
struct ExportBuffer {
    bytes: Vec<u8>,
    limit: usize,
    limit_hit: bool,
}

impl ExportBuffer {
    fn new(limit: u64) -> Result<Self, ExportError> {
        Ok(Self {
            bytes: Vec::new(),
            limit: usize::try_from(limit).map_err(|_| ExportError::Limit)?,
            limit_hit: false,
        })
    }
}

impl fmt::Debug for ExportBuffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExportBuffer")
            .field("byte_count", &self.bytes.len())
            .field("limit", &self.limit)
            .field("limit_hit", &self.limit_hit)
            .finish()
    }
}

impl Write for ExportBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(next) = self.bytes.len().checked_add(bytes.len()) else {
            self.limit_hit = true;
            return Err(io::Error::other("chat_export_limit"));
        };
        if next > self.limit {
            self.limit_hit = true;
            return Err(io::Error::other("chat_export_limit"));
        }
        if next > self.bytes.capacity() {
            let growth = self
                .bytes
                .capacity()
                .checked_mul(2)
                .unwrap_or(self.limit)
                .min(self.limit);
            let target = next.max(growth);
            if self
                .bytes
                .try_reserve_exact(target - self.bytes.len())
                .is_err()
            {
                self.limit_hit = true;
                return Err(io::Error::other("chat_export_limit"));
            }
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_text(
    output: &mut ExportBuffer,
    snapshot: &ConversationSnapshot,
) -> Result<(), ExportError> {
    write_text_inner(output, snapshot).map_err(|error| match error {
        TextError::Invalid => ExportError::InvalidResponse,
        TextError::Limit => ExportError::Limit,
    })
}

enum TextError {
    Invalid,
    Limit,
}
impl From<io::Error> for TextError {
    fn from(_: io::Error) -> Self {
        Self::Limit
    }
}

fn time_text(value: time::OffsetDateTime) -> Result<String, TextError> {
    value.format(&Rfc3339).map_err(|_| TextError::Invalid)
}

fn write_text_inner(
    output: &mut ExportBuffer,
    snapshot: &ConversationSnapshot,
) -> Result<(), TextError> {
    let session = &snapshot.session;
    writeln!(
        output,
        "VibeMux conversation export\nSchema version: {}\nKind: {}\nCaptured at: {}\nPartial export: {}\nRecord sequence cutoff: {}\nStored record count: {}\nExported record count: {}",
        snapshot.schema_version,
        match snapshot.kind {
            ExportKind::Chat => "chat",
            ExportKind::Workspace => "workspace",
        },
        time_text(snapshot.captured_at)?,
        if snapshot.partial { "yes" } else { "no" },
        snapshot.record_sequence_cutoff,
        session.record_count,
        snapshot.records.len()
    )?;
    if snapshot.partial {
        writeln!(
            output,
            "[PARTIAL: retained content or turn output is incomplete; see the explicit markers and flags below.]"
        )?;
    }
    writeln!(
        output,
        "\nSession\nSchema version: {}\nChat ID: {}\nProject ID: {}\nHarness: {}\nState: {}\nGeneration: {}\nVersion: {}",
        session.schema_version,
        session.chat_id,
        session.project_id,
        session.harness.as_str(),
        session.state.as_str(),
        session.generation,
        session.version
    )?;
    match session.active_request_id {
        Some(request) => writeln!(output, "Active request ID: {request}")?,
        None => writeln!(output, "Active request ID: none")?,
    }
    writeln!(
        output,
        "Content purged: {}\nHas title: {}\nCreated at: {}\nUpdated at: {}\nTitle:",
        session.content_purged,
        session.has_title,
        time_text(session.created_at)?,
        time_text(session.updated_at)?
    )?;
    match (&snapshot.title, snapshot.title_error) {
        (Some(title), None) => {
            escaped_text(output, title)?;
            writeln!(output)?;
        }
        (None, Some(error)) => writeln!(output, "[TITLE UNAVAILABLE: {}]", content_error(error))?,
        (None, None) => writeln!(output, "[no retained title]")?,
        _ => return Err(TextError::Invalid),
    }
    if session.content_purged {
        writeln!(
            output,
            "[PURGED: private record bodies and title are absent from this copy.]"
        )?;
    }
    writeln!(
        output,
        "\nTurns: {}\nOutcome, correlated terminal, and output completeness are separate recorded facts.",
        snapshot.turns.len()
    )?;
    for turn in &snapshot.turns {
        writeln!(
            output,
            "\nTurn correlation ID: {}\nRequest ID: {}\nTask ID: {}\nRun ID: {}\nGeneration: {}\nPhase: {}\nOutcome: {}\nError code: {}\nCorrelated terminal: {}\nOutput complete: {}\nCreated at: {}\nUpdated at: {}",
            turn.correlation_id,
            turn.request_id,
            turn.task_id,
            turn.run_id,
            turn.generation,
            turn.phase.as_str(),
            outcome(turn.outcome),
            failure(turn.error_code),
            turn.correlated_terminal,
            turn.output_complete,
            time_text(turn.created_at)?,
            time_text(turn.updated_at)?
        )?;
        if !turn.output_complete {
            writeln!(output, "[OUTPUT INCOMPLETE]")?;
        }
    }
    writeln!(
        output,
        "\nRecords in canonical sequence order: {}",
        snapshot.records.len()
    )?;
    for record in &snapshot.records {
        writeln!(
            output,
            "\n--- Record {} ---\nRecord ID: {}\nRole / kind: {}\nTurn correlation ID: {}\nCreated at: {}\nOriginal bytes: {}",
            record.sequence,
            record.record_id,
            record_kind(record.kind),
            record.correlation_id,
            time_text(record.created_at)?,
            record.original_byte_count
        )?;
        match record.retained_byte_count {
            Some(bytes) => writeln!(output, "Retained bytes: {bytes}")?,
            None => writeln!(output, "Retained bytes: unavailable")?,
        }
        writeln!(
            output,
            "Truncated: {}\nContent error: {}",
            record.truncated,
            record.content_error.map_or("none", content_error)
        )?;
        if record.truncated {
            let retained = record.retained_byte_count.ok_or(TextError::Invalid)?;
            writeln!(
                output,
                "[TRUNCATED: retained {retained} of {} original bytes]",
                record.original_byte_count
            )?;
        }
        match (&record.text, record.content_error) {
            (Some(text), None) => {
                writeln!(output, "Content:")?;
                escaped_text(output, text)?;
                writeln!(output)?;
            }
            (None, Some(error)) => {
                writeln!(output, "[CONTENT UNAVAILABLE: {}]", content_error(error))?
            }
            _ => return Err(TextError::Invalid),
        }
    }
    writeln!(
        output,
        "\nEnd of snapshot at record sequence {}. Later records are outside this copy.",
        snapshot.record_sequence_cutoff
    )?;
    Ok(())
}

fn escaped_text(output: &mut ExportBuffer, text: &str) -> io::Result<()> {
    let mut start = 0;
    for (index, character) in text.char_indices() {
        let escaped = (character.is_control() && character != '\n' && character != '\t')
            || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
        if escaped || character == '\\' {
            output.write_all(&text.as_bytes()[start..index])?;
            if character == '\\' {
                output.write_all(b"\\\\")?;
            } else {
                write!(output, "\\u{:04X}", u32::from(character))?;
            }
            start = index + character.len_utf8();
        }
    }
    output.write_all(&text.as_bytes()[start..])
}

fn record_kind(value: ChatRecordKind) -> &'static str {
    match value {
        ChatRecordKind::User => "user",
        ChatRecordKind::Assistant => "assistant",
        ChatRecordKind::Tool => "tool",
        ChatRecordKind::Approval => "approval",
        ChatRecordKind::Notice => "notice",
    }
}
fn outcome(value: Option<ChatTurnOutcome>) -> &'static str {
    match value {
        None => "none",
        Some(ChatTurnOutcome::Completed) => "completed",
        Some(ChatTurnOutcome::Failed) => "failed",
        Some(ChatTurnOutcome::Cancelled) => "cancelled",
        Some(ChatTurnOutcome::Interrupted) => "interrupted",
    }
}
fn failure(value: Option<ChatFailureCode>) -> &'static str {
    match value {
        None => "none",
        Some(ChatFailureCode::VendorFailed) => "vendor_failed",
        Some(ChatFailureCode::ProtocolViolation) => "protocol_violation",
        Some(ChatFailureCode::ProcessLost) => "process_lost",
        Some(ChatFailureCode::DeadlineExceeded) => "deadline_exceeded",
        Some(ChatFailureCode::Cancelled) => "cancelled",
        Some(ChatFailureCode::Interrupted) => "interrupted",
        Some(ChatFailureCode::OwnershipUncertain) => "ownership_uncertain",
        Some(ChatFailureCode::OutputUnavailable) => "output_unavailable",
    }
}
fn content_error(value: ChatContentError) -> &'static str {
    match value {
        ChatContentError::Purged => "chat_content_purged",
        ChatContentError::Unavailable => "chat_content_unavailable",
        ChatContentError::InvalidUtf8 => "chat_content_invalid_utf8",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_export::{ExportRecord, ExportTurn};
    use time::OffsetDateTime;
    use uuid::Uuid;
    use vibemux_types::{
        ChatId, ProjectId, RunId, TaskId,
        chat::{ChatHarness, ChatState, ChatTurnPhase},
    };
    use vibemuxd::chat_session::api::ChatSessionStatus;

    fn snapshot(text: &str) -> ConversationSnapshot {
        let at = OffsetDateTime::UNIX_EPOCH;
        ConversationSnapshot {
            schema_version: 1,
            kind: ExportKind::Chat,
            captured_at: at,
            session: ChatSessionStatus {
                schema_version: 1,
                chat_id: ChatId::new(),
                project_id: ProjectId::new(),
                harness: ChatHarness::Claude,
                state: ChatState::Ready,
                generation: 2,
                version: 3,
                active_request_id: None,
                record_count: 1,
                last_record_sequence: 10,
                content_purged: false,
                has_title: true,
                created_at: at,
                updated_at: at,
            },
            record_sequence_cutoff: 10,
            title: Some("PRIVATE_TITLE 中文".into()),
            title_error: None,
            turns: vec![ExportTurn {
                correlation_id: Uuid::from_u128(100),
                request_id: Uuid::from_u128(200),
                task_id: TaskId::new(),
                run_id: RunId::new(),
                generation: 1,
                phase: ChatTurnPhase::Completed,
                outcome: Some(ChatTurnOutcome::Completed),
                error_code: None,
                correlated_terminal: true,
                output_complete: true,
                created_at: at,
                updated_at: at,
            }],
            records: vec![ExportRecord {
                record_id: Uuid::from_u128(300),
                correlation_id: Uuid::from_u128(100),
                sequence: 10,
                kind: ChatRecordKind::Assistant,
                original_byte_count: text.len() as u64,
                retained_byte_count: Some(text.len() as u64),
                truncated: false,
                content_error: None,
                text: Some(text.into()),
                created_at: at,
            }],
            partial: false,
        }
    }

    #[test]
    fn pretty_json_round_trips_every_field_and_exact_unicode_text() {
        let value = snapshot("Héllo 中文 👩‍💻\n\t<script>literal</script>\u{202e}\u{1b}");
        let encoded = format_export(&value, ExportFormat::Json).expect("JSON export");
        let decoded: serde_json::Value = serde_json::from_slice(&encoded).expect("UTF-8 JSON");
        assert_eq!(
            decoded,
            serde_json::to_value(&value).expect("contract fields")
        );
        assert_eq!(
            decoded["records"][0]["text"],
            value.records[0].text.as_deref().expect("text")
        );
        assert_eq!(decoded["schema_version"], 1);
        assert!(encoded.starts_with(b"{\n  \"schema_version\""));
        assert!(encoded.ends_with(b"\n"));
    }

    #[test]
    fn text_has_metadata_cutoff_flags_and_visible_terminal_or_bidi_controls() {
        let value = snapshot(
            "PRIVATE_BODY\n\tUnicode中文 👩‍💻 <b>literal</b>\r\u{1b}[2J\u{009b}31m\u{202e}rtl\u{2066}isolate\u{2069}\\u001B",
        );
        let text =
            String::from_utf8(format_export(&value, ExportFormat::Text).expect("text export"))
                .expect("UTF-8");
        for expected in [
            "Schema version: 1",
            "Record sequence cutoff: 10",
            "Role / kind: assistant",
            "Task ID:",
            "Run ID:",
            "Correlated terminal: true",
            "Output complete: true",
            "Outcome: completed",
            "PRIVATE_BODY\n\tUnicode中文 👩‍💻 <b>literal</b>",
            "\\u000D",
            "\\u001B[2J",
            "\\u009B31m",
            "\\u202Ertl",
            "\\u2066isolate\\u2069",
            "\\\\u001B",
            "Later records are outside this copy.",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        for forbidden in [
            '\r', '\u{1b}', '\u{009b}', '\u{202e}', '\u{2066}', '\u{2069}',
        ] {
            assert!(!text.contains(forbidden));
        }
    }

    #[test]
    fn normal_global_sequence_gaps_preserve_all_rows_and_do_not_make_export_partial() {
        let mut value = snapshot("first");
        let mut second = value.records[0].clone();
        second.record_id = Uuid::from_u128(301);
        second.sequence = 900;
        value.records.push(second);
        value.session.record_count = 2;
        value.session.last_record_sequence = 900;
        value.record_sequence_cutoff = 900;
        let text =
            String::from_utf8(format_export(&value, ExportFormat::Text).expect("global cursors"))
                .expect("text");
        assert!(text.contains("Partial export: no"));
        assert!(
            text.find("--- Record 10 ---").expect("first")
                < text.find("--- Record 900 ---").expect("second")
        );
    }

    #[test]
    fn unavailable_truncated_and_incomplete_output_remain_explicit_partial_facts() {
        let mut value = snapshot("retained");
        value.records[0].original_byte_count = MAX_CHAT_RECORD_BYTES + 100;
        value.records[0].truncated = true;
        value.turns[0].output_complete = false;
        value.turns[0].correlated_terminal = false;
        value.partial = true;
        let mut missing = value.records[0].clone();
        missing.record_id = Uuid::from_u128(301);
        missing.sequence = 80;
        missing.original_byte_count = 100;
        missing.retained_byte_count = None;
        missing.truncated = false;
        missing.text = None;
        missing.content_error = Some(ChatContentError::Unavailable);
        value.records.push(missing);
        value.session.record_count = 2;
        value.session.last_record_sequence = 80;
        value.record_sequence_cutoff = 80;
        let text = String::from_utf8(format_export(&value, ExportFormat::Text).expect("partial"))
            .expect("UTF-8");
        for marker in [
            "Partial export: yes",
            "[TRUNCATED:",
            "[CONTENT UNAVAILABLE: chat_content_unavailable]",
            "[OUTPUT INCOMPLETE]",
            "Correlated terminal: false",
            "Outcome: completed",
        ] {
            assert!(text.contains(marker));
        }
        value.partial = false;
        assert_eq!(
            format_export(&value, ExportFormat::Json),
            Err(ExportError::InvalidResponse)
        );
    }

    #[test]
    fn purged_and_title_errors_export_only_placeholders_and_reject_copied_private_text() {
        let mut value = snapshot("PRIVATE_BODY");
        value.session.content_purged = true;
        value.session.has_title = false;
        value.title = None;
        value.title_error = Some(ChatContentError::Purged);
        value.records[0].text = None;
        value.records[0].content_error = Some(ChatContentError::Purged);
        value.partial = true;
        let text =
            String::from_utf8(format_export(&value, ExportFormat::Text).expect("purged metadata"))
                .expect("text");
        assert!(text.contains("[PURGED:"));
        assert!(text.contains("[TITLE UNAVAILABLE: chat_content_purged]"));
        assert!(!text.contains("PRIVATE_BODY"));
        assert!(!text.contains("PRIVATE_TITLE"));
        value.records[0].text = Some("PRIVATE_BODY".into());
        assert_eq!(
            format_export(&value, ExportFormat::Json),
            Err(ExportError::InvalidResponse)
        );
    }

    #[test]
    fn malformed_counts_ids_order_correlations_and_byte_metadata_are_rejected() {
        let base = snapshot("中文");
        let mut cases = Vec::new();
        let mut value = base.clone();
        value.schema_version = 2;
        cases.push(value);
        let mut value = base.clone();
        value.record_sequence_cutoff = 11;
        cases.push(value);
        let mut value = base.clone();
        value.session.record_count = 2;
        value.partial = true;
        cases.push(value);
        let mut value = base.clone();
        value.records[0].sequence = 0;
        cases.push(value);
        let mut value = base.clone();
        value.records[0].correlation_id = Uuid::from_u128(999);
        cases.push(value);
        let mut value = base.clone();
        value.records[0].retained_byte_count = Some(2);
        value.records[0].original_byte_count = 2;
        cases.push(value);
        let mut value = base.clone();
        value.records[0].content_error = Some(ChatContentError::Unavailable);
        cases.push(value);
        let mut value = base.clone();
        value.records[0].text = None;
        cases.push(value);
        let mut value = base.clone();
        value.turns.push(value.turns[0].clone());
        cases.push(value);
        let mut value = base.clone();
        value.partial = true;
        cases.push(value);
        let mut value = base.clone();
        value.records.push(value.records[0].clone());
        value.session.record_count = 2;
        cases.push(value);
        let mut descending = base.clone();
        let mut middle = descending.records[0].clone();
        middle.record_id = Uuid::from_u128(301);
        middle.sequence = 5;
        let mut final_record = descending.records[0].clone();
        final_record.record_id = Uuid::from_u128(302);
        final_record.sequence = 20;
        descending.records.extend([middle, final_record]);
        descending.session.record_count = 3;
        descending.record_sequence_cutoff = 20;
        descending.session.last_record_sequence = 20;
        cases.push(descending);
        for value in cases {
            assert_eq!(
                format_export(&value, ExportFormat::Json),
                Err(ExportError::InvalidResponse)
            );
        }
    }

    #[test]
    fn empty_text_empty_history_and_turns_without_records_are_valid_when_coherent() {
        let mut value = snapshot("");
        assert!(format_export(&value, ExportFormat::Json).is_ok());
        value.records.clear();
        value.session.record_count = 0;
        value.session.last_record_sequence = 0;
        value.record_sequence_cutoff = 0;
        value.turns[0].output_complete = false;
        value.partial = true;
        assert!(format_export(&value, ExportFormat::Text).is_ok());
        value.turns.clear();
        value.partial = false;
        value.session.has_title = false;
        value.title = None;
        assert!(format_export(&value, ExportFormat::Json).is_ok());
    }

    #[test]
    fn retention_title_cardinality_and_output_growth_have_hard_limits_without_clipping() {
        let mut value = snapshot("small");
        value.title = Some("x".repeat(MAX_CHAT_TITLE_BYTES as usize + 1));
        assert_eq!(
            format_export(&value, ExportFormat::Json),
            Err(ExportError::Limit)
        );
        let mut value = snapshot("small");
        value.records[0].original_byte_count = MAX_CHAT_BYTES + 1;
        assert_eq!(
            format_export(&value, ExportFormat::Json),
            Err(ExportError::Limit)
        );
        let mut value = snapshot(&"x".repeat(MAX_CHAT_RECORD_BYTES as usize + 1));
        assert_eq!(
            format_export(&value, ExportFormat::Text),
            Err(ExportError::Limit)
        );
        value = snapshot("small");
        value.records = vec![value.records[0].clone(); MAX_EXPORT_RECORDS + 1];
        assert_eq!(
            format_export(&value, ExportFormat::Json),
            Err(ExportError::Limit)
        );
        let mut buffer = ExportBuffer::new(4).expect("small bound");
        buffer.write_all(b"kept").expect("exact bound");
        assert!(buffer.write_all(b"x").is_err());
        assert!(buffer.limit_hit);
        assert_eq!(buffer.bytes, b"kept");
    }

    #[test]
    fn every_retained_record_counts_toward_the_total_even_when_body_is_unavailable() {
        let mut value = snapshot("x");
        value.title = None;
        value.session.has_title = false;
        let count = usize::try_from(MAX_EXPORT_CONTENT_BYTES / MAX_CHAT_RECORD_BYTES)
            .expect("bounded count")
            + 1;
        let base = value.records[0].clone();
        value.records = (0..count)
            .map(|index| {
                let mut record = base.clone();
                record.record_id = Uuid::from_u128(1000 + index as u128);
                record.sequence = 10 + index as u64;
                record.original_byte_count = MAX_CHAT_RECORD_BYTES;
                record.retained_byte_count = Some(MAX_CHAT_RECORD_BYTES);
                record.text = None;
                record.content_error = Some(ChatContentError::Unavailable);
                record
            })
            .collect();
        value.session.record_count = count as u64;
        value.record_sequence_cutoff = 9 + count as u64;
        value.session.last_record_sequence = value.record_sequence_cutoff;
        value.partial = true;
        assert_eq!(
            format_export(&value, ExportFormat::Json),
            Err(ExportError::Limit)
        );
    }

    #[test]
    fn many_empty_turns_use_the_submission_bound_and_debug_stays_redacted() {
        let mut value = snapshot("PRIVATE_BODY");
        let template = value.turns[0].clone();
        for index in 0..MAX_EXPORT_RECORDS {
            let mut turn = template.clone();
            turn.correlation_id = Uuid::from_u128(10_000 + index as u128);
            turn.request_id = Uuid::from_u128(100_000 + index as u128);
            value.turns.push(turn);
        }
        assert!(value.turns.len() <= MAX_EXPORT_TURNS);
        assert!(format_export(&value, ExportFormat::Json).is_ok());
        let mut oversized = value.clone();
        oversized.turns = vec![template; MAX_EXPORT_TURNS + 1];
        assert_eq!(
            format_export(&oversized, ExportFormat::Json),
            Err(ExportError::Limit)
        );
        let mut buffer = ExportBuffer::new(64).expect("bound");
        buffer.write_all(b"PRIVATE_BUFFER").expect("private data");
        for debug in [
            format!("{value:?}"),
            format!("{:?}", value.records[0]),
            format!("{buffer:?}"),
        ] {
            for private in ["PRIVATE_BODY", "PRIVATE_TITLE", "PRIVATE_BUFFER"] {
                assert!(!debug.contains(private));
            }
        }
    }
}
