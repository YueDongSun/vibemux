//! Durable removal previews and atomic reservations. No filesystem or process I/O.
use super::{
    cleanup_authority::CleanupExecutionClaim,
    cleanup_rows::{encode, increment, load, path_is_reserved, replay, required, write},
    *,
};
use time::Duration;
use vibemux_types::{WorkspaceCleanupId, chat_owner::*, workspace_cleanup::*};

const PLAN_LIFETIME_SECONDS: i64 = 300;

pub(crate) const MIGRATION_V14: &str = r#"
CREATE TABLE workspace_cleanups(
 cleanup_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_execution_owners(chat_id),
 run_id TEXT NOT NULL,resource_key TEXT NOT NULL,worktree_path_key TEXT NOT NULL,branch_key TEXT NOT NULL,
 phase TEXT NOT NULL CHECK(phase IN('planned','claimed','armed','quiescent','removed','refused','unknown')),
 version INTEGER NOT NULL CHECK(version>0),record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE UNIQUE INDEX workspace_cleanup_resource ON workspace_cleanups(resource_key)
 WHERE phase IN('claimed','armed','quiescent','unknown');
CREATE UNIQUE INDEX workspace_cleanup_path ON workspace_cleanups(worktree_path_key)
 WHERE phase IN('claimed','armed','quiescent','unknown');
CREATE UNIQUE INDEX workspace_cleanup_branch ON workspace_cleanups(branch_key)
 WHERE phase IN('claimed','armed','quiescent','unknown');
CREATE INDEX workspace_cleanup_run ON workspace_cleanups(run_id);
CREATE TABLE workspace_cleanup_commands(
 operation_id TEXT PRIMARY KEY REFERENCES chat_operations(operation_id),
 cleanup_id TEXT NOT NULL REFERENCES workspace_cleanups(cleanup_id)
);
INSERT INTO schema_migrations(version,applied_at) VALUES(14,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
"#;

#[derive(Clone, Debug)]
pub struct WorkspaceCleanupPlan {
    pub operation_id: Uuid,
    pub daemon_incarnation: Uuid,
    /// Service-verified handle, Git and terminal observations, never renderer data.
    pub binding: WorkspaceCleanupBinding,
    pub timestamp: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkspaceCleanupConfirmation {
    pub operation_id: Uuid,
    pub cleanup_id: WorkspaceCleanupId,
    pub expected_version: u64,
    pub daemon_incarnation: Uuid,
    #[serde(skip)]
    pub timestamp: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkspaceCleanupMutation {
    pub operation_id: Uuid,
    pub cleanup_id: WorkspaceCleanupId,
    pub expected_version: u64,
    #[serde(skip)]
    pub timestamp: OffsetDateTime,
}

#[derive(Debug)]
pub struct WorkspaceCleanupCommit {
    pub cleanup: WorkspaceCleanup,
    pub event: Option<EventEnvelope>,
    pub sequence: u64,
    pub duplicate: bool,
    /// Present only after a fresh committed confirmation, never on a read/replay.
    pub execution_claim: Option<CleanupExecutionClaim>,
}

pub(super) fn cleanup_error(code: &'static str) -> StoreError {
    StoreError::WorkspaceCleanup(code)
}

pub(super) fn ensure(condition: bool, code: &'static str) -> Result<(), StoreError> {
    if condition {
        Ok(())
    } else {
        Err(cleanup_error(code))
    }
}

pub(super) fn canonical_owner(
    connection: &Connection,
    binding: &WorkspaceCleanupBinding,
) -> Result<ChatExecutionOwnerCommit, StoreError> {
    let owner = owners::required_owner(connection, binding.chat_id)?;
    let row = rows::existing_session(connection, binding.chat_id)?;
    ensure(
        owner.owner.phase == ChatOwnerPhase::Finalized
            && owner.owner.workspace_effect == Some(ChatWorkspaceEffect::Settled)
            && owner.owner.workspace_verified_at.is_some()
            && owner.owner.error_code.is_none()
            && owner.session.state == ChatState::Closed
            && row.fence.is_none()
            && owner.session.active_request_id.is_none()
            && !owner.process_claimed,
        "workspace_cleanup_owner_unsettled",
    )?;
    ensure(
        owner.owner.project_id == binding.project_id
            && owner.owner.task_id == binding.task_id
            && owner.owner.run_id == binding.run_id
            && owner.owner.version == binding.owner_version
            && owner.session.generation == binding.session_generation
            && owner.session.version == binding.session_version
            && owner.owner.workspace.as_ref() == Some(&binding.workspace)
            && owner.owner.resource_key.as_ref() == Some(&binding.resource_key),
        "workspace_cleanup_binding_conflict",
    )?;
    Ok(owner)
}

fn require_unreserved(
    connection: &Connection,
    binding: &WorkspaceCleanupBinding,
) -> Result<(), StoreError> {
    ensure(
        !is_reserved(connection, &binding.resource_key)?
            && !dispatch_reserved(connection, &binding.resource_key)?
            && !path_is_reserved(
                connection,
                &binding.workspace.path_key(),
                &binding.workspace.branch.to_ascii_lowercase(),
            )?,
        "workspace_cleanup_busy",
    )?;
    let reserved: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE resource_key=?1 AND owner_fence IS NOT NULL UNION ALL SELECT 1 FROM workflow_leases WHERE worktree_key=?1 AND state IN('active','quarantined') UNION ALL SELECT 1 FROM a2a_runs WHERE worktree_path_key=?2 OR branch_key=?3 UNION ALL SELECT 1 FROM workspace_cleanups WHERE run_id=?4 AND phase='removed')",
        params![binding.resource_key,binding.workspace.path_key(),binding.workspace.branch.to_ascii_lowercase(),binding.run_id.to_string()], |row|row.get(0))?;
    ensure(!reserved, "workspace_cleanup_busy")
}

impl SqliteStore {
    /// Latest historical or pending cleanup for a canonical Run. The operation
    /// sequence, not preview creation time, keeps a live claim ahead of older
    /// unconsumed previews. This returns no executable authority.
    pub fn workspace_cleanup_for_run(
        &self,
        run_id: vibemux_types::RunId,
    ) -> Result<Option<WorkspaceCleanupCommit>, StoreError> {
        let id:Option<Option<String>>=self.connection.query_row("SELECT CASE WHEN length(CAST(cleanup_id AS BLOB))<=36 THEN cleanup_id END FROM workspace_cleanups WHERE run_id=? ORDER BY updated_sequence DESC,cleanup_id DESC LIMIT 1",
            [run_id.to_string()],|row|row.get(0)).optional()?;
        let Some(id) = id else { return Ok(None) };
        let id = id
            .ok_or_else(|| cleanup_error("workspace_cleanup_projection_mismatch"))?
            .parse()
            .map_err(|_| cleanup_error("workspace_cleanup_projection_mismatch"))?;
        let stored = required(&self.connection, id)?;
        ensure(
            stored.cleanup.binding.run_id == run_id,
            "workspace_cleanup_projection_mismatch",
        )?;
        Ok(Some(stored.receipt(false)))
    }
    /// Read-only recovery of a lost Plan reply. A different global operation
    /// kind conflicts; no receipt lookup recreates execution authority.
    pub fn workspace_cleanup_for_plan_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<WorkspaceCleanupCommit>, StoreError> {
        ensure(!operation_id.is_nil(), "workspace_cleanup_invalid_input")?;
        let row:Option<(Option<String>,Option<String>)>=self.connection.query_row(
            "SELECT CASE WHEN length(CAST(o.kind AS BLOB))<=64 THEN o.kind END,CASE WHEN length(CAST(c.cleanup_id AS BLOB))<=36 THEN c.cleanup_id END FROM chat_operations o LEFT JOIN workspace_cleanup_commands c ON c.operation_id=o.operation_id WHERE o.operation_id=?",
            [operation_id.to_string()],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        let Some((kind, id)) = row else {
            let other:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM chat_turns WHERE request_id=?1 UNION ALL SELECT 1 FROM harness_dispatches WHERE request_id=?1 UNION ALL SELECT 1 FROM chat_execution_attempts WHERE request_id=?1)",
                [operation_id.to_string()],|row|row.get(0))?;
            ensure(!other, "workspace_cleanup_conflict")?;
            return Ok(None);
        };
        ensure(
            kind.as_deref() == Some("workspace_cleanup_planned"),
            "workspace_cleanup_conflict",
        )?;
        let id = id
            .ok_or_else(|| cleanup_error("workspace_cleanup_projection_mismatch"))?
            .parse()
            .map_err(|_| cleanup_error("workspace_cleanup_projection_mismatch"))?;
        let stored = required(&self.connection, id)?;
        ensure(
            stored.cleanup.plan_operation_id == operation_id,
            "workspace_cleanup_projection_mismatch",
        )?;
        Ok(Some(stored.receipt(true)))
    }

    /// Stable-ID bounded startup reconciliation page. Reads never release a
    /// reservation or return a claim, even for records created by this process.
    pub fn workspace_cleanup_pending(
        &self,
        after: Option<WorkspaceCleanupId>,
        limit: usize,
    ) -> Result<Vec<WorkspaceCleanupCommit>, StoreError> {
        ensure(
            (1..=128).contains(&limit),
            "workspace_cleanup_invalid_input",
        )?;
        let mut statement=self.connection.prepare("SELECT CASE WHEN length(CAST(cleanup_id AS BLOB))<=36 THEN cleanup_id END FROM workspace_cleanups WHERE phase IN('claimed','armed','quiescent','unknown') AND (?1 IS NULL OR cleanup_id>?1) ORDER BY cleanup_id LIMIT ?2")?;
        let ids: Vec<Option<String>> = statement
            .query_map(
                params![
                    after.map(|id| id.to_string()),
                    u32::try_from(limit)
                        .map_err(|_| cleanup_error("workspace_cleanup_invalid_input"))?
                ],
                |row| row.get(0),
            )?
            .collect::<Result<_, _>>()?;
        ids.into_iter()
            .map(|id| {
                let id = id
                    .ok_or_else(|| cleanup_error("workspace_cleanup_projection_mismatch"))?
                    .parse()
                    .map_err(|_| cleanup_error("workspace_cleanup_projection_mismatch"))?;
                Ok(required(&self.connection, id)?.receipt(false))
            })
            .collect()
    }

    /// Active cleanup and Removed tombstones block presentation of this historic
    /// owner Run. Terminal outcomes do not reserve unrelated future bindings.
    pub fn workspace_cleanup_blocks_directory(
        &self,
        run_id: vibemux_types::RunId,
    ) -> Result<bool, StoreError> {
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM workspace_cleanups WHERE run_id=? AND phase IN('claimed','armed','quiescent','unknown','removed'))",
            [run_id.to_string()],|row|row.get(0))?)
    }

    pub fn workspace_cleanup(
        &self,
        id: WorkspaceCleanupId,
    ) -> Result<Option<WorkspaceCleanupCommit>, StoreError> {
        load(&self.connection, id).map(|row| row.map(|row| row.receipt(false)))
    }

    /// A preview records observations but takes no reservation or authority.
    pub fn plan_workspace_cleanup(
        &mut self,
        input: &WorkspaceCleanupPlan,
    ) -> Result<WorkspaceCleanupCommit, StoreError> {
        input
            .binding
            .validate()
            .map_err(|_| cleanup_error("workspace_cleanup_invalid_input"))?;
        ensure(
            !input.daemon_incarnation.is_nil(),
            "workspace_cleanup_invalid_input",
        )?;
        let binding = encode(&(input.daemon_incarnation, &input.binding))?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(commit) = replay(
            &transaction,
            input.operation_id,
            "workspace_cleanup_planned",
            &binding,
        )? {
            return Ok(commit);
        }
        let owner = canonical_owner(&transaction, &input.binding)?;
        require_unreserved(&transaction, &input.binding)?;
        ensure(
            input.timestamp >= owner.owner.updated_at.max(owner.session.updated_at),
            "workspace_cleanup_invalid_input",
        )?;
        let record = WorkspaceCleanup {
            schema_version: WORKSPACE_CLEANUP_SCHEMA_VERSION,
            id: WorkspaceCleanupId::new(),
            plan_operation_id: input.operation_id,
            daemon_incarnation: input.daemon_incarnation,
            version: 1,
            phase: WorkspaceCleanupPhase::Planned,
            binding: input.binding.clone(),
            created_at: input.timestamp,
            expires_at: input.timestamp + Duration::seconds(PLAN_LIFETIME_SECONDS),
            updated_at: input.timestamp,
            confirmation_operation_id: None,
            claim_id: None,
            executor: None,
            arm_operation_id: None,
            process_result: None,
            quiescence: None,
            outcome: None,
            error_code: None,
        };
        let commit = write(
            &transaction,
            record,
            None,
            input.operation_id,
            "workspace_cleanup_planned",
            &binding,
            &owner.session,
        )?;
        transaction.commit()?;
        Ok(commit)
    }

    pub fn confirm_workspace_cleanup(
        &mut self,
        input: &WorkspaceCleanupConfirmation,
    ) -> Result<WorkspaceCleanupCommit, StoreError> {
        let binding = encode(input)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(commit) = replay(
            &transaction,
            input.operation_id,
            "workspace_cleanup_claimed",
            &binding,
        )? {
            return Ok(commit);
        }
        let mut stored = required(&transaction, input.cleanup_id)?;
        ensure(
            stored.cleanup.phase == WorkspaceCleanupPhase::Planned
                && stored.cleanup.version == input.expected_version,
            "workspace_cleanup_conflict",
        )?;
        ensure(
            stored.cleanup.daemon_incarnation == input.daemon_incarnation
                && input.timestamp >= stored.cleanup.created_at
                && input.timestamp < stored.cleanup.expires_at,
            "workspace_cleanup_stale_plan",
        )?;
        let owner = canonical_owner(&transaction, &stored.cleanup.binding)?;
        require_unreserved(&transaction, &stored.cleanup.binding)?;
        let previous = stored.cleanup.version;
        stored.cleanup.version = increment(previous)?;
        stored.cleanup.phase = WorkspaceCleanupPhase::Claimed;
        stored.cleanup.confirmation_operation_id = Some(input.operation_id);
        let claim_id = Uuid::new_v4();
        stored.cleanup.claim_id = Some(claim_id);
        stored.cleanup.updated_at = input.timestamp;
        let mut commit = write(
            &transaction,
            stored.cleanup,
            Some(previous),
            input.operation_id,
            "workspace_cleanup_claimed",
            &binding,
            &owner.session,
        )?;
        transaction.commit()?;
        commit.execution_claim = Some(CleanupExecutionClaim::new(
            commit.cleanup.id,
            claim_id,
            input.daemon_incarnation,
        ));
        Ok(commit)
    }
}
