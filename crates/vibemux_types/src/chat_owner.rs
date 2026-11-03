//! Lifetime Run ownership and durable workspace provisioning for native chats.
use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{
    ChatId, ProjectId, RunId, TaskId, ToolName,
    a2a::RunWorkspace,
    chat::{ChatError, ChatHarness, valid_digest},
};

pub const CHAT_OWNER_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatOwnerPhase {
    Provisioning,
    Open,
    RecoveryRequired,
    Finalized,
}

impl ChatOwnerPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Open => "open",
            Self::RecoveryRequired => "recovery_required",
            Self::Finalized => "finalized",
        }
    }

    pub const fn holds_workspace(self) -> bool {
        !matches!(self, Self::Finalized)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatOwnerFailure {
    WorkspaceCreateFailed,
    WorkspaceUnverified,
    WorkspaceCleanupUnverified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatWorkspaceEffect {
    Planned,
    Claimed,
    Settled,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatExecutionOwner {
    pub schema_version: u32,
    pub chat_id: ChatId,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub harness: ChatHarness,
    pub phase: ChatOwnerPhase,
    pub version: u64,
    pub base_commit: String,
    pub profile_sha256: String,
    pub admitted_tools: BTreeSet<ToolName>,
    /// Persisted before Git creation. The ownership token is private metadata.
    pub workspace: Option<RunWorkspace>,
    pub resource_key: Option<String>,
    pub workspace_effect: Option<ChatWorkspaceEffect>,
    /// Last verified provisioning receipt, independent of current process state.
    #[serde(with = "time::serde::rfc3339::option")]
    pub workspace_verified_at: Option<OffsetDateTime>,
    pub error_code: Option<ChatOwnerFailure>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl fmt::Debug for ChatExecutionOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatExecutionOwner")
            .field("chat_id", &self.chat_id)
            .field("task_id", &self.task_id)
            .field("run_id", &self.run_id)
            .field("phase", &self.phase)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl ChatExecutionOwner {
    pub fn validate(&self) -> Result<(), ChatError> {
        if self.schema_version != CHAT_OWNER_SCHEMA_VERSION
            || self.version == 0
            || self.version > i64::MAX as u64
            || !valid_digest(&self.profile_sha256)
            || !matches!(self.base_commit.len(), 40 | 64)
            || !self
                .base_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.updated_at < self.created_at
            || self.workspace.is_some() != self.resource_key.is_some()
            || self.workspace.is_some() != self.workspace_effect.is_some()
            || self
                .resource_key
                .as_ref()
                .is_some_and(|key| !valid_digest(key))
            || self.workspace_verified_at.is_some_and(|at| {
                self.workspace.is_none() || at < self.created_at || at > self.updated_at
            })
            || (self.phase == ChatOwnerPhase::Open
                && (self.workspace_verified_at.is_none()
                    || self.workspace_effect != Some(ChatWorkspaceEffect::Settled)
                    || self.error_code.is_some()))
            || (self.phase == ChatOwnerPhase::RecoveryRequired && self.error_code.is_none())
            || (self.phase == ChatOwnerPhase::Finalized
                && (self.workspace_effect == Some(ChatWorkspaceEffect::Claimed)
                    || self.error_code == Some(ChatOwnerFailure::WorkspaceCleanupUnverified)))
        {
            return Err(ChatError::InvalidInput);
        }
        if let Some(workspace) = &self.workspace {
            workspace.validate().map_err(|_| ChatError::InvalidInput)?;
            let normalized = workspace.path.replace('\\', "/");
            if workspace.base_commit != self.base_commit
                || workspace.branch != format!("codex/run_{}", self.run_id.as_uuid().simple())
                || normalized.rsplit('/').next() != Some(self.run_id.to_string().as_str())
                || normalized
                    .split('/')
                    .any(|component| matches!(component, "." | ".."))
            {
                return Err(ChatError::InvalidInput);
            }
        }
        Ok(())
    }
}
