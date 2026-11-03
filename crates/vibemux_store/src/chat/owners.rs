//! Lifetime canonical owners and one-use, durable workspace creation intents.
use super::*;
use rusqlite::TransactionBehavior;
use std::collections::BTreeSet;
use vibemux_harness::dispatch::Sha256Digest;
use vibemux_types::{
    Run, RunId, RunSpec, RunStatus, Task, TaskSpec, TaskStatus, ToolName, a2a::RunWorkspace,
    chat_owner::*,
};

pub(crate) const MIGRATION_V11: &str = r#"
CREATE TABLE chat_execution_owners(
 chat_id TEXT PRIMARY KEY REFERENCES chat_sessions(chat_id),
 project_id TEXT NOT NULL, task_id TEXT NOT NULL UNIQUE, run_id TEXT NOT NULL UNIQUE,
 phase TEXT NOT NULL CHECK(phase IN ('provisioning','open','recovery_required','finalized')),
 version INTEGER NOT NULL CHECK(version>0), resource_key TEXT,
 binding_json TEXT NOT NULL, record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE UNIQUE INDEX chat_owner_workspace_reservation ON chat_execution_owners(resource_key)
 WHERE phase!='finalized';
INSERT INTO schema_migrations(version,applied_at) VALUES(11,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
"#;

#[derive(Clone, Debug)]
pub struct ChatExecutionOwnerOpen {
    pub operation_id: Uuid,
    pub harness: ChatHarness,
    pub profile_sha256: String,
    pub base_commit: String,
    pub admitted_tools: BTreeSet<ToolName>,
    pub title_ref: Option<ContentRef>,
    pub timestamp: OffsetDateTime,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnerOpenBinding {
    harness: ChatHarness,
    profile_sha256: String,
    base_commit: String,
    admitted_tools: BTreeSet<ToolName>,
    title_ref: Option<ContentRef>,
}

struct OwnerRow {
    project_id: String,
    task_id: String,
    run_id: String,
    phase: String,
    version: u64,
    resource_key: Option<String>,
    binding: Option<String>,
    encoded: Option<String>,
    sequence: u64,
}

#[derive(Clone, Debug)]
pub struct ChatOwnerMutation {
    pub operation_id: Uuid,
    pub chat_id: ChatId,
    pub expected_version: u64,
    pub timestamp: OffsetDateTime,
}

#[derive(Clone, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ChatOwnerChange {
    Plan {
        workspace: RunWorkspace,
        resource_key: String,
    },
    ClaimCreation,
    VerifyWorkspace,
    WorkspaceFailed {
        code: ChatOwnerFailure,
        cleanup_verified: bool,
    },
    Reconcile,
    Stop,
}
impl fmt::Debug for ChatOwnerChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.kind())
    }
}
impl ChatOwnerChange {
    fn kind(&self) -> &'static str {
        match self {
            Self::Plan { .. } => "chat_owner_plan",
            Self::ClaimCreation => "chat_owner_creation_claimed",
            Self::VerifyWorkspace => "chat_owner_workspace_verified",
            Self::WorkspaceFailed { .. } => "chat_owner_workspace_failed",
            Self::Reconcile => "chat_owner_reconcile_requested",
            Self::Stop => "chat_owner_stopped",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatExecutionOwnerCommit {
    pub owner: ChatExecutionOwner,
    pub session: ChatSession,
    pub task: Task,
    pub run: Run,
    pub event: Option<EventEnvelope>,
    pub sequence: u64,
    pub duplicate: bool,
    /// Durable process authority, exposed only through an allowlisted read view.
    pub process_claimed: bool,
}

pub(super) fn is_execution_owned(connection: &Connection, id: ChatId) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_execution_owners WHERE chat_id=?)",
        [id.to_string()],
        |row| row.get(0),
    )?)
}

pub(super) fn require_read_only(connection: &Connection, id: ChatId) -> Result<(), StoreError> {
    require(
        !is_execution_owned(connection, id)?,
        ChatError::OwnerControlRequired,
    )
}

fn provisional_resource(run_id: vibemux_types::RunId) -> String {
    Sha256Digest::of(format!("chat_owner_provisioning:{run_id}").as_bytes()).to_hex()
}

fn owner_event(
    transaction: &Transaction<'_>,
    session: &ChatSession,
    owner: &ChatExecutionOwner,
    kind: &str,
) -> Result<(EventEnvelope, i64), StoreError> {
    let event_id = EventId::new();
    SqliteStore::insert_event(
        transaction,
        EventDraft {
            event_id,
            event_type: EventType::new(kind)?,
            project_id: owner.project_id,
            task_id: Some(owner.task_id),
            run_id: Some(owner.run_id),
            causation_id: None,
            actor: ActorName::new("daemon_chat_owner")?,
            timestamp: owner.updated_at,
            idempotency_key: Some(format!("chat_event:{event_id}")),
            payload: EventPayload::new(
                json!({"chat_id":owner.chat_id,"owner_phase":owner.phase,"owner_version":owner.version,"generation":session.generation,"workspace_planned":owner.workspace.is_some(),"workspace_verified":owner.workspace_verified_at.is_some(),"workspace_effect":owner.workspace_effect,"admitted_tools":owner.admitted_tools,"error_code":owner.error_code}),
            )?,
        },
    )
}

fn load_owner(
    connection: &Connection,
    id: ChatId,
) -> Result<Option<ChatExecutionOwnerCommit>, StoreError> {
    let row = connection.query_row(
        "SELECT project_id,task_id,run_id,phase,version,resource_key,CASE WHEN length(CAST(binding_json AS BLOB))<=16384 THEN binding_json END,CASE WHEN length(CAST(record_json AS BLOB))<=16384 THEN record_json END,updated_sequence FROM chat_execution_owners WHERE chat_id=?",
        [id.to_string()], |row| Ok(OwnerRow {
            project_id: row.get(0)?, task_id: row.get(1)?, run_id: row.get(2)?,
            phase: row.get(3)?, version: row.get(4)?, resource_key: row.get(5)?,
            binding: row.get(6)?, encoded: row.get(7)?, sequence: row.get(8)?,
        })).optional()?;
    let Some(OwnerRow {
        project_id,
        task_id,
        run_id,
        phase,
        version,
        resource_key,
        binding,
        encoded,
        sequence,
    }) = row
    else {
        return Ok(None);
    };
    let owner: ChatExecutionOwner = rows::decode(encoded)?;
    owner
        .validate()
        .map_err(|_| fail(ChatError::ProjectionMismatch))?;
    let original: OwnerOpenBinding = rows::decode(binding)?;
    let stored = rows::existing_session(connection, id)?;
    let session = stored.session;
    let task: Task = rows::load_projection(connection, "task", &task_id)?;
    let run: Run = rows::load_projection(connection, "run", &run_id)?;
    require(
        owner.chat_id == id
            && owner.project_id.to_string() == project_id
            && owner.task_id.to_string() == task_id
            && owner.run_id.to_string() == run_id
            && owner.phase.as_str() == phase
            && owner.version == version
            && owner.resource_key == resource_key
            && sequence > 0
            && original.harness == owner.harness
            && original.profile_sha256 == owner.profile_sha256
            && original.base_commit == owner.base_commit
            && original.admitted_tools == owner.admitted_tools
            && owner.project_id == session.project_id
            && owner.harness == session.harness
            && owner.base_commit == session.base_commit
            && owner.profile_sha256 == session.profile_sha256
            && session.resource_key
                == owner
                    .resource_key
                    .clone()
                    .unwrap_or_else(|| provisional_resource(owner.run_id))
            && task.task_id() == owner.task_id
            && task.project_id() == owner.project_id
            && run.run_id() == owner.run_id
            && run.task_id() == owner.task_id
            && run.project_id() == owner.project_id
            && run.base_commit() == owner.base_commit
            && run.role() == "chat_execution_owner"
            && run.protocol() == "chat_owner_v1"
            && run.harness() == owner.harness.as_str(),
        ChatError::ProjectionMismatch,
    )?;
    let states_match = match owner.phase {
        ChatOwnerPhase::Provisioning | ChatOwnerPhase::Open => {
            let expected_task = if session.state == ChatState::RecoveryRequired {
                TaskStatus::Blocked
            } else {
                TaskStatus::InProgress
            };
            task.status() == expected_task
                && matches!(run.status(), RunStatus::Preparing | RunStatus::Running)
        }
        ChatOwnerPhase::RecoveryRequired => {
            task.status() == TaskStatus::Blocked
                && matches!(run.status(), RunStatus::Preparing | RunStatus::Running)
        }
        ChatOwnerPhase::Finalized => {
            task.status() == TaskStatus::Cancelled
                && matches!(run.status(), RunStatus::Failed | RunStatus::Stopped)
                && stored.fence.is_none()
                && session.active_request_id.is_none()
        }
    };
    require(states_match, ChatError::ProjectionMismatch)?;
    Ok(Some(ChatExecutionOwnerCommit {
        owner,
        session,
        task,
        run,
        event: None,
        sequence,
        duplicate: false,
        process_claimed: stored.fence.is_some(),
    }))
}

pub(super) fn required_owner(
    connection: &Connection,
    id: ChatId,
) -> Result<ChatExecutionOwnerCommit, StoreError> {
    load_owner(connection, id)?.ok_or_else(|| fail(ChatError::NotFound))
}

impl SqliteStore {
    pub fn chat_execution_owner_for_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<ChatExecutionOwnerCommit>, StoreError> {
        require(!operation_id.is_nil(), ChatError::InvalidInput)?;
        let id: Option<String> = self.connection.query_row(
            "SELECT chat_id FROM chat_operations WHERE operation_id=? AND kind='chat_owner_opened'",
            [operation_id.to_string()], |row| row.get(0)).optional()?;
        id.map(|id| {
            let id = id
                .parse()
                .map_err(|_| fail(ChatError::ProjectionMismatch))?;
            required_owner(&self.connection, id)
        })
        .transpose()
    }
    /// Indexed durable Run binding, validated through the complete owner loader.
    pub fn chat_execution_owner_for_run(
        &self,
        run_id: RunId,
    ) -> Result<Option<ChatExecutionOwnerCommit>, StoreError> {
        let id: Option<String> = self
            .connection
            .query_row(
                "SELECT chat_id FROM chat_execution_owners WHERE run_id=?",
                [run_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        id.map(|id| {
            let id: ChatId = id
                .parse()
                .map_err(|_| fail(ChatError::ProjectionMismatch))?;
            let owner = required_owner(&self.connection, id)?;
            require(owner.owner.run_id == run_id, ChatError::ProjectionMismatch)?;
            Ok(owner)
        })
        .transpose()
    }

    pub fn chat_execution_owner(
        &self,
        id: ChatId,
    ) -> Result<Option<ChatExecutionOwner>, StoreError> {
        Ok(load_owner(&self.connection, id)?.map(|stored| stored.owner))
    }

    pub fn chat_execution_owner_state(
        &self,
        id: ChatId,
    ) -> Result<Option<ChatExecutionOwnerCommit>, StoreError> {
        load_owner(&self.connection, id)
    }

    pub fn list_chat_execution_owners(
        &self,
        after: Option<ChatId>,
        limit: u32,
        finalized: bool,
    ) -> Result<Vec<ChatExecutionOwnerCommit>, StoreError> {
        require(limit > 0 && limit <= MAX_CHAT_PAGE, ChatError::InvalidInput)?;
        let mut statement = self.connection.prepare("SELECT chat_id FROM chat_execution_owners WHERE (?1 IS NULL OR chat_id>?1) AND (?2 OR phase!='finalized') ORDER BY chat_id LIMIT ?3")?;
        let ids = statement
            .query_map(
                params![after.map(|id| id.to_string()), finalized, limit],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                let id = id
                    .parse()
                    .map_err(|_| fail(ChatError::ProjectionMismatch))?;
                required_owner(&self.connection, id)
            })
            .collect()
    }

    pub fn open_chat_execution_owner(
        &mut self,
        input: &ChatExecutionOwnerOpen,
    ) -> Result<ChatExecutionOwnerCommit, StoreError> {
        require(
            !input.operation_id.is_nil()
                && valid_digest(&input.profile_sha256)
                && matches!(input.base_commit.len(), 40 | 64)
                && input
                    .base_commit
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            ChatError::InvalidInput,
        )?;
        if let Some(reference) = &input.title_ref {
            reference.validate(MAX_CHAT_TITLE_BYTES).map_err(fail)?;
        }
        let binding = serde_json::to_string(&OwnerOpenBinding {
            harness: input.harness,
            profile_sha256: input.profile_sha256.clone(),
            base_commit: input.base_commit.clone(),
            admitted_tools: input.admitted_tools.clone(),
            title_ref: input.title_ref.clone(),
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(row) = operation(
            &transaction,
            input.operation_id,
            "chat_owner_opened",
            &binding,
        )? {
            let mut stored = required_owner(&transaction, row.chat_id)?;
            stored.sequence = row.sequence;
            stored.duplicate = true;
            return Ok(stored);
        }
        let count: u64 =
            transaction.query_row("SELECT COUNT(*) FROM chat_sessions", [], |row| row.get(0))?;
        require(count < MAX_CHAT_SESSIONS, ChatError::Capacity)?;
        let project_id = crate::resolve_or_mint_project_id(&transaction)?;
        let task = Task::new(TaskSpec {
            project_id,
            title: format!("{} workspace conversation", input.harness.as_str()),
            description: String::new(),
        })
        .and_then(|task| task.transitioned(TaskStatus::InProgress))
        .map_err(|_| fail(ChatError::InvalidInput))?;
        let run = Run::new(RunSpec {
            project_id,
            task_id: task.task_id(),
            harness: input.harness.as_str().to_owned(),
            role: "chat_execution_owner".to_owned(),
            protocol: "chat_owner_v1".to_owned(),
            base_commit: input.base_commit.clone(),
        })
        .map_err(|_| fail(ChatError::InvalidInput))?;
        let opening = ChatOpen {
            operation_id: input.operation_id,
            harness: input.harness,
            profile_sha256: input.profile_sha256.clone(),
            resource_key: provisional_resource(run.run_id()),
            base_commit: input.base_commit.clone(),
            title_ref: input.title_ref.clone(),
            timestamp: input.timestamp,
        };
        let mut session = sessions::new_session(project_id, &opening);
        let owner = ChatExecutionOwner {
            schema_version: CHAT_OWNER_SCHEMA_VERSION,
            chat_id: session.chat_id,
            project_id,
            task_id: task.task_id(),
            run_id: run.run_id(),
            harness: input.harness,
            phase: ChatOwnerPhase::Provisioning,
            version: 1,
            base_commit: input.base_commit.clone(),
            profile_sha256: input.profile_sha256.clone(),
            admitted_tools: input.admitted_tools.clone(),
            workspace: None,
            resource_key: None,
            workspace_effect: None,
            workspace_verified_at: None,
            error_code: None,
            created_at: input.timestamp,
            updated_at: input.timestamp,
        };
        let (event, sequence) = owner_event(&transaction, &session, &owner, "chat_owner_opened")?;
        rows::save_task(&transaction, &task, sequence)?;
        rows::save_run(&transaction, &run, sequence)?;
        rows::insert_session(&transaction, &session, sequence)?;
        if let Some(reference) = &input.title_ref {
            claim_content(&transaction, &mut session, reference, MAX_CHAT_TITLE_BYTES)?;
            session.title_ref = Some(reference.clone());
            rows::save_session(&transaction, &session, None, 1, sequence)?;
        }
        transaction.execute("INSERT INTO chat_execution_owners(chat_id,project_id,task_id,run_id,phase,version,resource_key,binding_json,record_json,updated_sequence) VALUES(?,?,?,?,?,1,NULL,?,?,?)",
            params![owner.chat_id.to_string(),project_id.to_string(),owner.task_id.to_string(),owner.run_id.to_string(),owner.phase.as_str(),binding,serde_json::to_string(&owner)?,sequence])?;
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            "chat_owner_opened",
            &binding,
            &[],
            sequence,
        )?;
        transaction.commit()?;
        Ok(ChatExecutionOwnerCommit {
            owner,
            session,
            task,
            run,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
            process_claimed: false,
        })
    }

    pub fn change_chat_execution_owner(
        &mut self,
        input: &ChatOwnerMutation,
        change: &ChatOwnerChange,
    ) -> Result<ChatExecutionOwnerCommit, StoreError> {
        require(
            !input.operation_id.is_nil()
                && input.expected_version > 0
                && input.expected_version <= i64::MAX as u64,
            ChatError::InvalidInput,
        )?;
        let binding = serde_json::to_string(&(input.chat_id, input.expected_version, change))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(row) = operation(&transaction, input.operation_id, change.kind(), &binding)? {
            require(row.chat_id == input.chat_id, ChatError::Conflict)?;
            let mut stored = required_owner(&transaction, row.chat_id)?;
            stored.sequence = row.sequence;
            stored.duplicate = true;
            return Ok(stored);
        }
        let stored = required_owner(&transaction, input.chat_id)?;
        require(
            stored.owner.error_code != Some(ChatOwnerFailure::WorkspaceCleanupUnverified)
                || matches!(
                    change,
                    ChatOwnerChange::Reconcile
                        | ChatOwnerChange::WorkspaceFailed {
                            code: ChatOwnerFailure::WorkspaceCleanupUnverified,
                            ..
                        }
                ),
            ChatError::Busy,
        )?;
        require(
            stored.owner.version == input.expected_version,
            ChatError::Conflict,
        )?;
        require(
            stored.owner.phase != ChatOwnerPhase::Finalized,
            ChatError::InvalidTransition,
        )?;
        let session_row = rows::existing_session(&transaction, input.chat_id)?;
        require(
            session_row.fence.is_none() && stored.session.active_request_id.is_none(),
            ChatError::Busy,
        )?;
        let mut owner = stored.owner;
        let mut session = stored.session;
        let mut task = stored.task;
        let mut run = stored.run;
        let old_owner_version = owner.version;
        let old_session_version = session.version;
        let old_resource = session.resource_key.clone();
        owner.updated_at = input
            .timestamp
            .max(owner.updated_at)
            .max(session.updated_at);
        match change {
            ChatOwnerChange::Plan {
                workspace,
                resource_key,
            } => {
                require(
                    owner.workspace.is_none() && valid_digest(resource_key),
                    ChatError::Conflict,
                )?;
                require(
                    !is_reserved(&transaction, resource_key)?
                        && !dispatch_reserved(&transaction, resource_key)?,
                    ChatError::Busy,
                )?;
                owner.workspace = Some(workspace.clone());
                owner.resource_key = Some(resource_key.clone());
                owner.workspace_effect = Some(ChatWorkspaceEffect::Planned);
                owner.phase = ChatOwnerPhase::Provisioning;
                owner.error_code = None;
                session.resource_key = resource_key.clone();
            }
            ChatOwnerChange::ClaimCreation => {
                require(
                    owner.workspace.is_some()
                        && matches!(
                            owner.workspace_effect,
                            Some(ChatWorkspaceEffect::Planned | ChatWorkspaceEffect::Settled)
                        )
                        && matches!(
                            owner.phase,
                            ChatOwnerPhase::Provisioning | ChatOwnerPhase::RecoveryRequired
                        ),
                    ChatError::InvalidTransition,
                )?;
                owner.workspace_effect = Some(ChatWorkspaceEffect::Claimed);
                owner.workspace_verified_at = None;
                owner.phase = ChatOwnerPhase::Provisioning;
                owner.error_code = None;
            }
            ChatOwnerChange::VerifyWorkspace => {
                require(
                    owner.workspace.is_some()
                        && matches!(
                            owner.workspace_effect,
                            Some(ChatWorkspaceEffect::Claimed | ChatWorkspaceEffect::Settled)
                        ),
                    ChatError::InvalidTransition,
                )?;
                owner.workspace_effect = Some(ChatWorkspaceEffect::Settled);
                owner.workspace_verified_at = Some(owner.updated_at);
                owner.phase = ChatOwnerPhase::Open;
                owner.error_code = None;
            }
            ChatOwnerChange::WorkspaceFailed {
                code,
                cleanup_verified,
            } => {
                require(
                    *code != ChatOwnerFailure::WorkspaceCleanupUnverified || !cleanup_verified,
                    ChatError::InvalidInput,
                )?;
                if *cleanup_verified && owner.workspace_effect == Some(ChatWorkspaceEffect::Claimed)
                {
                    owner.workspace_effect = Some(ChatWorkspaceEffect::Settled);
                }
                owner.phase = ChatOwnerPhase::RecoveryRequired;
                owner.error_code = Some(*code);
            }
            ChatOwnerChange::Reconcile => {}
            ChatOwnerChange::Stop => {
                require(
                    owner.workspace_effect != Some(ChatWorkspaceEffect::Claimed),
                    ChatError::Busy,
                )?;
                let target = match run.status() {
                    RunStatus::Preparing => RunStatus::Failed,
                    RunStatus::Running => RunStatus::Stopped,
                    _ => return Err(fail(ChatError::InvalidTransition)),
                };
                run = run
                    .transitioned(target, None)
                    .map_err(|_| fail(ChatError::InvalidTransition))?;
                owner.phase = ChatOwnerPhase::Finalized;
                session.state = ChatState::Closed;
            }
        }
        let task_state = match owner.phase {
            ChatOwnerPhase::Provisioning | ChatOwnerPhase::Open => TaskStatus::InProgress,
            ChatOwnerPhase::RecoveryRequired => TaskStatus::Blocked,
            ChatOwnerPhase::Finalized => TaskStatus::Cancelled,
        };
        if task.status() != task_state {
            task = task
                .transitioned(task_state)
                .map_err(|_| fail(ChatError::InvalidTransition))?;
        }
        owner.version = next_version(owner.version)?;
        owner.validate().map_err(fail)?;
        session.version = next_version(session.version)?;
        session.updated_at = owner.updated_at;
        let (event, sequence) = owner_event(&transaction, &session, &owner, change.kind())?;
        if old_resource != session.resource_key {
            let changed = transaction.execute("UPDATE chat_sessions SET resource_key=? WHERE chat_id=? AND version=? AND resource_key=?",
                params![session.resource_key, session.chat_id.to_string(), old_session_version, old_resource])?;
            require(changed == 1, ChatError::Conflict)?;
        }
        rows::save_session(&transaction, &session, None, old_session_version, sequence)?;
        rows::save_task(&transaction, &task, sequence)?;
        rows::save_run(&transaction, &run, sequence)?;
        let changed = transaction.execute("UPDATE chat_execution_owners SET phase=?,version=?,resource_key=?,record_json=?,updated_sequence=? WHERE chat_id=? AND version=?",
            params![owner.phase.as_str(),owner.version,owner.resource_key,serde_json::to_string(&owner)?,sequence,owner.chat_id.to_string(),old_owner_version])?;
        require(changed == 1, ChatError::Conflict)?;
        record_operation(
            &transaction,
            input.operation_id,
            &session,
            change.kind(),
            &binding,
            &[],
            sequence,
        )?;
        transaction.commit()?;
        Ok(ChatExecutionOwnerCommit {
            owner,
            session,
            task,
            run,
            event: Some(event),
            sequence: sequence as u64,
            duplicate: false,
            process_claimed: false,
        })
    }
}
