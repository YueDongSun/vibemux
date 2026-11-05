//! Complete private tool readback for the fixed Surfaces fixture. No storage writes.
use super::{
    ARCHIVED_PROMPT, ARCHIVED_TITLE, QaError, SAVED_PROMPTS, SAVED_TITLE, SeedIds,
    WORKSPACE_PROMPT, WORKSPACE_TITLE,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use vibemux_client::chat_export::{
    ConversationSnapshot, EXPORT_SCHEMA_VERSION, ExportFormat, ExportKind, ExportRequest,
    collect_conversation, verify_snapshot,
};
use vibemux_types::{
    RunId, TaskId,
    chat::{ChatRecordKind, ChatTurnOutcome, ChatTurnPhase, MAX_CHAT_RECORD_BYTES},
    chat_file_tool::{FileChangeKind, FileChangeToolRecord, FileChangeToolStatus, FileToolChange},
    chat_tool::{CommandToolStatus, ToolRecord},
};
use vibemuxd::{
    chat_session::{api::ChatSessionStatus, execution_api::ChatExecutionStatus},
    control::ControlClient,
};

const COMMAND: &str = "synthetic reported command";
const COMMAND_OUTPUT: &str = "PRIVATE_MIXED_OUTPUT";
const FIND_TARGET: &str = "DEMO_SURFACE_FIND_TARGET";
const DIFF_LINES: usize = 300;
const FIND_LINE: usize = 270;

struct Expected<'a> {
    session: &'a ChatSessionStatus,
    kind: ExportKind,
    title: &'a str,
    prompts: &'a [&'a str],
    cwd: &'a Path,
    owner: Option<(TaskId, RunId)>,
    tools: bool,
}

#[derive(Default)]
struct Group<'a> {
    prompt: &'a str,
    user: usize,
    assistant: usize,
    command: usize,
    file: usize,
}

fn require(condition: bool) -> Result<(), QaError> {
    if condition {
        Ok(())
    } else {
        Err(QaError::CheckFailed)
    }
}

pub(super) async fn check(
    client: &ControlClient,
    ids: &SeedIds,
    saved: &ChatSessionStatus,
    archived: &ChatSessionStatus,
    workspace: &ChatExecutionStatus,
    project_root: &Path,
) -> Result<(), QaError> {
    let worktree = project_root
        .join(".vibemux")
        .join("worktrees")
        .join(ids.workspace_run.to_string());
    for expected in [
        Expected {
            session: saved,
            kind: ExportKind::Chat,
            title: SAVED_TITLE,
            prompts: &SAVED_PROMPTS,
            cwd: project_root,
            owner: None,
            tools: true,
        },
        Expected {
            session: archived,
            kind: ExportKind::Chat,
            title: ARCHIVED_TITLE,
            prompts: &[ARCHIVED_PROMPT],
            cwd: project_root,
            owner: None,
            tools: false,
        },
        Expected {
            session: &workspace.session,
            kind: ExportKind::Workspace,
            title: WORKSPACE_TITLE,
            prompts: &[WORKSPACE_PROMPT],
            cwd: &worktree,
            owner: Some((workspace.owner.task_id, ids.workspace_run)),
            tools: true,
        },
    ] {
        let count = expected.prompts.len() as u64 * if expected.tools { 4 } else { 2 };
        require(
            expected.session.record_count == count
                && expected.session.last_record_sequence == count,
        )?;
        // This existing GUI-neutral collector uses ChatRequest::Content and
        // ChatExecutionRequest::Content through HistoryClient. It enforces the
        // 8192-byte chunks, advancing cursors, identical record metadata, bounded
        // totals and a final unchanged status; it creates no destination/file.
        let snapshot = collect_conversation(
            client,
            &ExportRequest {
                kind: expected.kind,
                format: ExportFormat::Json,
                chat_id: expected.session.chat_id,
                generation: expected.session.generation,
                expected_version: expected.session.version,
            },
        )
        .await
        .map_err(|_| QaError::CheckFailed)?;
        validate_snapshot(&snapshot, &expected)?;
        verify_snapshot(client, &snapshot)
            .await
            .map_err(|_| QaError::CheckFailed)?;
    }
    require(
        super::saved_status(client, ids.saved_chat)
            .await
            .map_err(|_| QaError::CheckFailed)?
            == *saved,
    )?;
    require(
        super::saved_status(client, ids.archived_chat)
            .await
            .map_err(|_| QaError::CheckFailed)?
            == *archived,
    )?;
    require(
        super::workspace_status(client, ids.workspace_chat)
            .await
            .map_err(|_| QaError::CheckFailed)?
            == *workspace,
    )
}

fn validate_snapshot(
    snapshot: &ConversationSnapshot,
    expected: &Expected<'_>,
) -> Result<(), QaError> {
    let count = expected.prompts.len() * if expected.tools { 4 } else { 2 };
    require(
        snapshot.schema_version == EXPORT_SCHEMA_VERSION
            && snapshot.kind == expected.kind
            && snapshot.session == *expected.session
            && snapshot.title.as_deref() == Some(expected.title)
            && snapshot.title_error.is_none()
            && !snapshot.partial
            && snapshot.record_sequence_cutoff == expected.session.last_record_sequence
            && snapshot.session.record_count == count as u64
            && snapshot.record_sequence_cutoff == count as u64
            && snapshot.turns.len() == expected.prompts.len()
            && snapshot.records.len() == count,
    )?;
    let mut groups = BTreeMap::new();
    let mut requests = BTreeSet::new();
    let mut tasks = BTreeSet::new();
    let mut runs = BTreeSet::new();
    for (turn, prompt) in snapshot.turns.iter().zip(expected.prompts) {
        require(
            !turn.correlation_id.is_nil()
                && !turn.request_id.is_nil()
                && requests.insert(turn.request_id)
                && !turn.task_id.as_uuid().is_nil()
                && !turn.run_id.as_uuid().is_nil()
                && turn.generation > 0
                && turn.generation <= expected.session.generation
                && turn.phase == ChatTurnPhase::Completed
                && turn.outcome == Some(ChatTurnOutcome::Completed)
                && turn.error_code.is_none()
                && turn.correlated_terminal
                && turn.output_complete
                && turn.created_at <= turn.updated_at
                && turn.created_at >= expected.session.created_at
                && turn.updated_at <= expected.session.updated_at,
        )?;
        if let Some((task_id, run_id)) = expected.owner {
            require(turn.task_id == task_id && turn.run_id == run_id)?;
        } else {
            require(tasks.insert(turn.task_id) && runs.insert(turn.run_id))?;
        }
        require(
            groups
                .insert(
                    turn.correlation_id,
                    Group {
                        prompt,
                        ..Default::default()
                    },
                )
                .is_none(),
        )?;
    }
    let mut record_ids = BTreeSet::new();
    for (index, record) in snapshot.records.iter().enumerate() {
        let text = record.text.as_deref().ok_or(QaError::CheckFailed)?;
        require(
            !record.record_id.is_nil()
                && record_ids.insert(record.record_id)
                && record.sequence == index as u64 + 1
                && record.content_error.is_none()
                && !record.truncated
                && !text.is_empty()
                && text.len() as u64 <= MAX_CHAT_RECORD_BYTES
                && record.retained_byte_count == Some(text.len() as u64)
                && record.original_byte_count == text.len() as u64
                && record.created_at >= expected.session.created_at
                && record.created_at <= expected.session.updated_at,
        )?;
        // correlation_id is canonical turn_id for Saved or attempt_id for
        // Workspace. Vendor call IDs may repeat across those groups/processes.
        let group = groups
            .get_mut(&record.correlation_id)
            .ok_or(QaError::CheckFailed)?;
        match record.kind {
            ChatRecordKind::User => {
                require(text == group.prompt)?;
                group.user += 1;
            }
            ChatRecordKind::Assistant => group.assistant += 1,
            ChatRecordKind::Tool if expected.tools => {
                let tool = ToolRecord::from_private_text(text).map_err(|_| QaError::CheckFailed)?;
                require(!tool.is_truncated())?;
                match tool {
                    ToolRecord::Command(command) => {
                        let cwd = expected.cwd.to_str().ok_or(QaError::CheckFailed)?;
                        require(
                            command.status == CommandToolStatus::Failed
                                && command.exit_code == Some(7)
                                && command.command == COMMAND
                                && command.output.as_deref() == Some(COMMAND_OUTPUT)
                                && command.duration_ms == Some(1)
                                && normalized_path(&command.working_directory)
                                    == normalized_path(cwd),
                        )?;
                        group.command += 1;
                    }
                    ToolRecord::FileChange(file) => {
                        require(file == expected_file(&file.call_id)?)?;
                        group.file += 1;
                    }
                }
            }
            _ => return Err(QaError::CheckFailed),
        }
    }
    for group in groups.values() {
        require(
            group.user == 1
                && group.assistant == 1
                && group.command == usize::from(expected.tools)
                && group.file == usize::from(expected.tools),
        )?;
    }
    Ok(())
}

fn normalized_path(value: &str) -> String {
    let value = value
        .strip_prefix("\\\\?\\")
        .unwrap_or(value)
        .replace('\\', "/");
    if cfg!(windows) {
        value.trim_end_matches('/').to_ascii_lowercase()
    } else {
        value.trim_end_matches('/').to_owned()
    }
}

fn expected_diff() -> String {
    (0..DIFF_LINES)
        .map(|index| {
            if index == FIND_LINE {
                format!("+{FIND_TARGET}")
            } else {
                format!("+DEMO surface line {index}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn expected_file(call_id: &str) -> Result<FileChangeToolRecord, QaError> {
    let make = |path: &str, kind, move_path: Option<&str>, diff: String| {
        FileToolChange::from_observation(path.to_owned(), kind, move_path.map(str::to_owned), diff)
            .map_err(|_| QaError::CheckFailed)
    };
    FileChangeToolRecord::from_observation(
        call_id.to_owned(),
        FileChangeToolStatus::Completed,
        vec![
            make(
                "DEMO_surface.txt",
                FileChangeKind::Update,
                Some("DEMO_moved_surface.txt"),
                expected_diff(),
            )?,
            make(
                "DEMO_added.txt",
                FileChangeKind::Add,
                None,
                "+DEMO added line\n".to_owned(),
            )?,
            make(
                "DEMO_deleted.txt",
                FileChangeKind::Delete,
                None,
                "-DEMO deleted line\n".to_owned(),
            )?,
        ],
    )
    .map_err(|_| QaError::CheckFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use uuid::Uuid;
    use vibemux_client::chat_export::{ExportRecord, ExportTurn};
    use vibemux_types::{ChatId, ProjectId, chat_tool::CommandToolRecord};
    use vibemuxd::chat_session::api::ChatContentError;

    #[derive(Clone)]
    struct Fixture {
        snapshot: ConversationSnapshot,
        session: ChatSessionStatus,
        title: &'static str,
        kind: ExportKind,
        prompts: &'static [&'static str],
        cwd: PathBuf,
        owner: Option<(TaskId, RunId)>,
        tools: bool,
    }

    impl Fixture {
        fn validate(&self) -> Result<(), QaError> {
            validate_snapshot(
                &self.snapshot,
                &Expected {
                    session: &self.session,
                    kind: self.kind,
                    title: self.title,
                    prompts: self.prompts,
                    cwd: &self.cwd,
                    owner: self.owner,
                    tools: self.tools,
                },
            )
        }

        fn set_text(&mut self, index: usize, text: String) {
            let record = &mut self.snapshot.records[index];
            record.original_byte_count = text.len() as u64;
            record.retained_byte_count = Some(text.len() as u64);
            record.text = Some(text);
        }

        // Keep the body byte metadata correct so semantic faults cannot pass
        // merely because a metadata-size assertion found the mutation first.
        fn rewrite(&mut self, index: usize, change: impl FnOnce(&mut Value)) {
            let mut body: Value =
                serde_json::from_str(self.snapshot.records[index].text.as_ref().expect("body"))
                    .expect("tool JSON");
            change(&mut body);
            self.set_text(index, serde_json::to_string(&body).expect("tool text"));
        }
    }

    fn fixture(kind: ExportKind, tools: bool) -> Fixture {
        let (prompts, title): (&'static [&'static str], _) = match (kind, tools) {
            (ExportKind::Chat, true) => (&SAVED_PROMPTS, SAVED_TITLE),
            (ExportKind::Chat, false) => (&[ARCHIVED_PROMPT], ARCHIVED_TITLE),
            (ExportKind::Workspace, _) => (&[WORKSPACE_PROMPT], WORKSPACE_TITLE),
        };
        let count = prompts.len() * if tools { 4 } else { 2 };
        let session: ChatSessionStatus = serde_json::from_value(json!({
            "schema_version":1,"chat_id":ChatId::new(),"project_id":ProjectId::new(),
            "harness":if tools {"codex"} else {"claude"},"state":if tools {"closed"} else {"archived"},
            "generation":1,"version":5,"active_request_id":null,"record_count":count,"last_record_sequence":count,
            "content_purged":false,"has_title":true,"created_at":"2026-11-02T00:00:00Z","updated_at":"2026-11-02T00:00:01Z"
        })).expect("session");
        let owner = (kind == ExportKind::Workspace).then(|| (TaskId::new(), RunId::new()));
        let cwd = PathBuf::from("C:/DEMO/project");
        let mut turns = Vec::new();
        let mut records = Vec::new();
        for prompt in prompts {
            let correlation_id = Uuid::new_v4();
            let (task_id, run_id) = owner.unwrap_or_else(|| (TaskId::new(), RunId::new()));
            turns.push(ExportTurn {
                correlation_id,
                request_id: Uuid::new_v4(),
                task_id,
                run_id,
                generation: 1,
                phase: ChatTurnPhase::Completed,
                outcome: Some(ChatTurnOutcome::Completed),
                error_code: None,
                correlated_terminal: true,
                output_complete: true,
                created_at: session.created_at,
                updated_at: session.updated_at,
            });
            let mut bodies = vec![(ChatRecordKind::User, (*prompt).to_owned())];
            if tools {
                // Deliberately reuse vendor IDs across canonical Saved turns.
                // They identify a reported call, not the conversation group.
                let command = CommandToolRecord::from_observation(
                    "same_command_id".to_owned(),
                    CommandToolStatus::Failed,
                    COMMAND.to_owned(),
                    cwd.to_string_lossy().into_owned(),
                    Some(COMMAND_OUTPUT.to_owned()),
                    Some(7),
                    Some(1),
                )
                .expect("command");
                bodies.push((
                    ChatRecordKind::Tool,
                    command.to_private_text().expect("command text"),
                ));
                bodies.push((
                    ChatRecordKind::Tool,
                    expected_file("same_file_id")
                        .expect("file")
                        .to_private_text()
                        .expect("file text"),
                ));
            }
            bodies.push((ChatRecordKind::Assistant, "synthetic answer".to_owned()));
            for (kind, text) in bodies {
                records.push(ExportRecord {
                    record_id: Uuid::new_v4(),
                    correlation_id,
                    sequence: records.len() as u64 + 1,
                    kind,
                    original_byte_count: text.len() as u64,
                    retained_byte_count: Some(text.len() as u64),
                    truncated: false,
                    content_error: None,
                    text: Some(text),
                    created_at: session.created_at,
                });
            }
        }
        Fixture {
            snapshot: ConversationSnapshot {
                schema_version: EXPORT_SCHEMA_VERSION,
                kind,
                captured_at: session.updated_at,
                session: session.clone(),
                record_sequence_cutoff: count as u64,
                title: Some(title.to_owned()),
                title_error: None,
                turns,
                records,
                partial: false,
            },
            session,
            title,
            kind,
            prompts,
            cwd,
            owner,
            tools,
        }
    }

    #[test]
    fn complete_saved_turns_and_workspace_attempt_accept_repeated_vendor_ids_without_cross_group_dedupe()
     {
        for (kind, tools) in [
            (ExportKind::Chat, true),
            (ExportKind::Chat, false),
            (ExportKind::Workspace, true),
        ] {
            let fixture = fixture(kind, tools);
            assert_eq!(fixture.validate(), Ok(()));
        }
    }

    #[test]
    fn fixed_file_fixture_retains_three_ordered_changes_and_the_long_find_target() {
        let file = expected_file("fixture_id").expect("file");
        assert_eq!(file.original_change_count, 3);
        assert!(!file.changes_truncated);
        assert_eq!(
            file.changes
                .iter()
                .map(|change| change.kind)
                .collect::<Vec<_>>(),
            vec![
                FileChangeKind::Update,
                FileChangeKind::Add,
                FileChangeKind::Delete
            ]
        );
        assert_eq!(
            file.changes[0].move_path.as_deref(),
            Some("DEMO_moved_surface.txt")
        );
        let diff = &file.changes[0].diff;
        assert_eq!(diff.lines().count(), DIFF_LINES);
        assert_eq!(
            diff.lines().nth(FIND_LINE),
            Some("+DEMO_SURFACE_FIND_TARGET")
        );
        assert_eq!(diff.matches(FIND_TARGET).count(), 1);
        assert!(
            file.changes
                .iter()
                .all(|change| !change.diff_truncated && change.diff.len() < 16 * 1024)
        );
        assert!(
            file.changes
                .iter()
                .map(|change| change.diff.len())
                .sum::<usize>()
                < 32 * 1024
        );
        assert!(
            !ToolRecord::from_private_text(&file.to_private_text().expect("body"))
                .expect("decode")
                .is_truncated()
        );
    }

    #[test]
    fn summary_identity_title_counts_and_private_completeness_are_fail_closed() {
        for fault in [
            "schema",
            "kind",
            "session",
            "title",
            "title_error",
            "partial",
            "cutoff",
            "count",
            "missing_record",
            "missing_turn",
        ] {
            let mut fixture = fixture(ExportKind::Chat, true);
            match fault {
                "schema" => fixture.snapshot.schema_version = 2,
                "kind" => fixture.snapshot.kind = ExportKind::Workspace,
                "session" => fixture.snapshot.session.chat_id = ChatId::new(),
                "title" => fixture.snapshot.title = Some("different title".to_owned()),
                "title_error" => fixture.snapshot.title_error = Some(ChatContentError::Unavailable),
                "partial" => fixture.snapshot.partial = true,
                "cutoff" => fixture.snapshot.record_sequence_cutoff += 1,
                "count" => fixture.snapshot.session.record_count = 4,
                "missing_record" => {
                    fixture.snapshot.records.pop();
                }
                _ => {
                    fixture.snapshot.turns.pop();
                }
            }
            assert_eq!(fixture.validate(), Err(QaError::CheckFailed), "{fault}");
        }
    }

    #[test]
    fn missing_malformed_unknown_or_unavailable_tool_content_is_not_a_count_only_pass() {
        for fault in [
            "missing",
            "error",
            "legacy",
            "invalid_json",
            "unknown_format",
            "unknown_version",
            "outer_truncation",
            "byte_count",
            "unknown_kind",
        ] {
            let mut fixture = fixture(ExportKind::Chat, true);
            match fault {
                "missing" => fixture.snapshot.records[1].text = None,
                "error" => {
                    fixture.snapshot.records[1].content_error = Some(ChatContentError::Unavailable)
                }
                "legacy" => fixture.set_text(1, "opaque tool event".to_owned()),
                "invalid_json" => fixture.set_text(1, "{".to_owned()),
                "unknown_format" => fixture.rewrite(1, |body| body["format"] = json!("other_tool")),
                "unknown_version" => fixture.rewrite(1, |body| body["schema_version"] = json!(2)),
                "outer_truncation" => fixture.snapshot.records[1].truncated = true,
                "byte_count" => fixture.snapshot.records[1].original_byte_count += 1,
                _ => fixture.snapshot.records[1].kind = ChatRecordKind::Notice,
            }
            assert_eq!(fixture.validate(), Err(QaError::CheckFailed), "{fault}");
        }
    }

    #[test]
    fn command_and_file_semantics_are_verified_after_valid_body_metadata() {
        for fault in [
            "command_status",
            "exit",
            "cwd",
            "command",
            "output",
            "duration",
            "file_status",
            "file_order",
            "move",
            "marker",
            "list_truncated",
        ] {
            let mut fixture = fixture(ExportKind::Chat, true);
            match fault {
                "command_status" => fixture.rewrite(1, |body| body["status"] = json!("completed")),
                "exit" => fixture.rewrite(1, |body| body["exit_code"] = json!(0)),
                "cwd" => {
                    fixture.rewrite(1, |body| body["working_directory"] = json!("C:/DEMO/other"))
                }
                "command" => fixture.rewrite(1, |body| {
                    body["command"] = json!("other command");
                    body["command_original_bytes"] = json!(13);
                }),
                "output" => fixture.rewrite(1, |body| {
                    body["output"] = json!("other output");
                    body["output_original_bytes"] = json!(12);
                }),
                "duration" => fixture.rewrite(1, |body| body["duration_ms"] = json!(2)),
                "file_status" => fixture.rewrite(2, |body| body["status"] = json!("failed")),
                "file_order" => fixture.rewrite(2, |body| {
                    body["changes"].as_array_mut().expect("changes").swap(1, 2)
                }),
                "move" => fixture.rewrite(2, |body| {
                    body["changes"][0]["move_path"] = json!("DEMO_other.txt")
                }),
                "marker" => fixture.rewrite(2, |body| {
                    let changed = body["changes"][0]["diff"]
                        .as_str()
                        .expect("diff")
                        .replace(FIND_TARGET, "DEMO_SURFACE_OTHER_TEXT");
                    body["changes"][0]["diff_original_bytes"] = json!(changed.len());
                    body["changes"][0]["diff"] = json!(changed);
                }),
                _ => fixture.rewrite(2, |body| {
                    body["original_change_count"] = json!(4);
                    body["changes_truncated"] = json!(true);
                }),
            }
            assert_eq!(fixture.validate(), Err(QaError::CheckFailed), "{fault}");
        }
        let mut fixture = fixture(ExportKind::Chat, true);
        let truncated = CommandToolRecord::from_observation(
            "id".to_owned(),
            CommandToolStatus::Failed,
            COMMAND.to_owned(),
            fixture.cwd.to_string_lossy().into_owned(),
            Some("x".repeat(16385)),
            Some(7),
            Some(1),
        )
        .expect("bounded truncated command");
        assert!(truncated.output_truncated);
        fixture.set_text(1, truncated.to_private_text().expect("bounded body"));
        assert_eq!(fixture.validate(), Err(QaError::CheckFailed));
    }

    #[test]
    fn canonical_group_mismatches_and_duplicate_replays_cannot_balance_the_total_count() {
        for fault in [
            "foreign_group",
            "cross_group",
            "duplicate_command",
            "record_id",
            "sequence",
            "turn_id",
            "request_id",
            "task_id",
            "prompt",
        ] {
            let mut fixture = fixture(ExportKind::Chat, true);
            match fault {
                "foreign_group" => fixture.snapshot.records[1].correlation_id = Uuid::new_v4(),
                "cross_group" => {
                    fixture.snapshot.records[1].correlation_id =
                        fixture.snapshot.turns[1].correlation_id
                }
                "duplicate_command" => fixture.set_text(
                    2,
                    fixture.snapshot.records[1].text.clone().expect("command"),
                ),
                "record_id" => {
                    fixture.snapshot.records[1].record_id = fixture.snapshot.records[0].record_id
                }
                "sequence" => fixture.snapshot.records[1].sequence = 1,
                "turn_id" => {
                    fixture.snapshot.turns[1].correlation_id =
                        fixture.snapshot.turns[0].correlation_id
                }
                "request_id" => {
                    fixture.snapshot.turns[1].request_id = fixture.snapshot.turns[0].request_id
                }
                "task_id" => fixture.snapshot.turns[1].task_id = fixture.snapshot.turns[0].task_id,
                _ => fixture.set_text(0, SAVED_PROMPTS[1].to_owned()),
            }
            assert_eq!(fixture.validate(), Err(QaError::CheckFailed), "{fault}");
        }
    }

    #[test]
    fn completed_correlation_and_workspace_owner_are_required_without_inference_from_tool_status() {
        for fault in [
            "phase",
            "outcome",
            "terminal",
            "complete",
            "generation",
            "owner_task",
            "owner_run",
        ] {
            let mut fixture = fixture(ExportKind::Workspace, true);
            let turn = &mut fixture.snapshot.turns[0];
            match fault {
                "phase" => turn.phase = ChatTurnPhase::Running,
                "outcome" => turn.outcome = None,
                "terminal" => turn.correlated_terminal = false,
                "complete" => turn.output_complete = false,
                "generation" => turn.generation += 1,
                "owner_task" => turn.task_id = TaskId::new(),
                _ => turn.run_id = RunId::new(),
            }
            assert_eq!(fixture.validate(), Err(QaError::CheckFailed), "{fault}");
        }
    }
}
