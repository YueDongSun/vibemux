//! Closed owner/workspace API. Private paths and receipt tokens stay daemon-side.
use super::{ChatService, ChatServiceError};
use serde::{Deserialize, Serialize};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{ChatExecutionOwnerCommit, ChatOwnerMutation};
use vibemux_types::{
    ChatId, ProjectId, RunId, RunStatus, TaskId, ToolName,
    chat::{ChatError, ChatHarness, MAX_CHAT_TITLE_BYTES},
    chat_owner::{ChatOwnerFailure, ChatOwnerPhase, ChatWorkspaceEffect},
};

pub const MAX_OWNER_API_PAGE: u32 = 32;
const OWNER_API_SCHEMA_VERSION: u32 = 1;
const fn default_limit() -> u32 {
    16
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatOwnerRequest {
    List {
        #[serde(default)]
        after: Option<ChatId>,
        #[serde(default = "default_limit")]
        limit: u32,
        #[serde(default)]
        include_finalized: bool,
    },
    Open {
        operation_id: Uuid,
        harness: ChatHarness,
        #[serde(default)]
        title: Option<String>,
        consent_private_storage: bool,
        consent_workspace: bool,
    },
    LookupOpen {
        operation_id: Uuid,
    },
    Status {
        chat_id: ChatId,
    },
    Title {
        chat_id: ChatId,
    },
    Reconcile {
        operation_id: Uuid,
        chat_id: ChatId,
        expected_version: u64,
    },
    Stop {
        operation_id: Uuid,
        chat_id: ChatId,
        expected_version: u64,
    },
}

impl ChatOwnerRequest {
    pub fn validate(&self) -> Result<(), ChatError> {
        let valid = match self {
            Self::List { limit, .. } => *limit > 0 && *limit <= MAX_OWNER_API_PAGE,
            Self::Open {
                operation_id,
                title,
                consent_private_storage,
                consent_workspace,
                ..
            } => {
                !operation_id.is_nil()
                    && *consent_private_storage
                    && *consent_workspace
                    && title.as_ref().is_none_or(|text| {
                        !text.trim().is_empty() && text.len() as u64 <= MAX_CHAT_TITLE_BYTES
                    })
            }
            Self::LookupOpen { operation_id } => !operation_id.is_nil(),
            Self::Reconcile {
                operation_id,
                expected_version,
                ..
            }
            | Self::Stop {
                operation_id,
                expected_version,
                ..
            } => {
                !operation_id.is_nil()
                    && *expected_version > 0
                    && *expected_version <= i64::MAX as u64
            }
            Self::Status { .. } | Self::Title { .. } => true,
        };
        if valid {
            Ok(())
        } else {
            Err(ChatError::InvalidInput)
        }
    }
}

impl fmt::Debug for ChatOwnerRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::List { .. } => "ChatOwnerRequest::List",
            Self::Open { .. } => "ChatOwnerRequest::Open(<private>)",
            Self::LookupOpen { .. } => "ChatOwnerRequest::LookupOpen",
            Self::Status { .. } => "ChatOwnerRequest::Status",
            Self::Title { .. } => "ChatOwnerRequest::Title",
            Self::Reconcile { .. } => "ChatOwnerRequest::Reconcile",
            Self::Stop { .. } => "ChatOwnerRequest::Stop",
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatOwnerStatus {
    pub schema_version: u32,
    pub chat_id: ChatId,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub harness: ChatHarness,
    pub phase: ChatOwnerPhase,
    pub version: u64,
    pub run_status: RunStatus,
    pub admitted_tools: Vec<ToolName>,
    pub has_title: bool,
    pub workspace_planned: bool,
    pub workspace_branch: Option<String>,
    pub workspace_effect: Option<ChatWorkspaceEffect>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub workspace_verified_at: Option<OffsetDateTime>,
    pub provisioning_active: bool,
    pub error_code: Option<ChatOwnerFailure>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl ChatOwnerStatus {
    pub(super) fn from_commit(
        service: &ChatService,
        value: &ChatExecutionOwnerCommit,
    ) -> Result<Self, ChatServiceError> {
        let owner = &value.owner;
        Ok(Self {
            schema_version: OWNER_API_SCHEMA_VERSION,
            chat_id: owner.chat_id,
            project_id: owner.project_id,
            task_id: owner.task_id,
            run_id: owner.run_id,
            harness: owner.harness,
            phase: owner.phase,
            version: owner.version,
            run_status: value.run.status(),
            admitted_tools: owner.admitted_tools.iter().copied().collect(),
            has_title: value.session.title_ref.is_some(),
            workspace_planned: owner.workspace.is_some(),
            workspace_branch: owner
                .workspace
                .as_ref()
                .map(|workspace| workspace.branch.clone()),
            workspace_effect: owner.workspace_effect,
            workspace_verified_at: owner.workspace_verified_at,
            provisioning_active: service.workspace_provisioning_active(owner.chat_id)?,
            error_code: owner.error_code,
            created_at: owner.created_at,
            updated_at: owner.updated_at,
        })
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatOwnerResponse {
    List {
        owners: Vec<ChatOwnerStatus>,
        next_after: Option<ChatId>,
        more: bool,
    },
    Owner {
        owner: ChatOwnerStatus,
        sequence: u64,
        duplicate: bool,
    },
    Status {
        owner: Option<ChatOwnerStatus>,
    },
    Title {
        chat_id: ChatId,
        title: Option<String>,
    },
}
impl fmt::Debug for ChatOwnerResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::List { .. } => "ChatOwnerResponse::List",
            Self::Owner { .. } => "ChatOwnerResponse::Owner",
            Self::Status { .. } => "ChatOwnerResponse::Status",
            Self::Title { .. } => "ChatOwnerResponse::Title(<private>)",
        })
    }
}

pub async fn execute(
    service: &ChatService,
    request: ChatOwnerRequest,
) -> Result<ChatOwnerResponse, ChatServiceError> {
    request.validate()?;
    match request {
        ChatOwnerRequest::List {
            after,
            limit,
            include_finalized,
        } => {
            let mut rows = service
                .execution_owners(after, limit + 1, include_finalized)
                .await?;
            let more = rows.len() > limit as usize;
            rows.truncate(limit as usize);
            let next_after = rows.last().map(|row| row.owner.chat_id);
            let owners = rows
                .iter()
                .map(|row| ChatOwnerStatus::from_commit(service, row))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ChatOwnerResponse::List {
                owners,
                next_after,
                more,
            })
        }
        ChatOwnerRequest::Open {
            operation_id,
            harness,
            title,
            ..
        } => owner_response(
            service,
            service
                .open_execution_owner(operation_id, harness, title)
                .await?,
        ),
        ChatOwnerRequest::LookupOpen { operation_id } => status_response(
            service,
            service.execution_owner_for_operation(operation_id).await?,
        ),
        ChatOwnerRequest::Status { chat_id } => {
            status_response(service, service.execution_owner(chat_id).await?)
        }
        ChatOwnerRequest::Title { chat_id } => Ok(ChatOwnerResponse::Title {
            chat_id,
            title: service.execution_owner_title(chat_id).await?,
        }),
        ChatOwnerRequest::Reconcile {
            operation_id,
            chat_id,
            expected_version,
        } => owner_response(
            service,
            service
                .reconcile_execution_owner(ChatOwnerMutation {
                    operation_id,
                    chat_id,
                    expected_version,
                    timestamp: OffsetDateTime::now_utc(),
                })
                .await?,
        ),
        ChatOwnerRequest::Stop {
            operation_id,
            chat_id,
            expected_version,
        } => owner_response(
            service,
            service
                .stop_execution_owner(ChatOwnerMutation {
                    operation_id,
                    chat_id,
                    expected_version,
                    timestamp: OffsetDateTime::now_utc(),
                })
                .await?,
        ),
    }
}

fn owner_response(
    service: &ChatService,
    value: ChatExecutionOwnerCommit,
) -> Result<ChatOwnerResponse, ChatServiceError> {
    Ok(ChatOwnerResponse::Owner {
        owner: ChatOwnerStatus::from_commit(service, &value)?,
        sequence: value.sequence,
        duplicate: value.duplicate,
    })
}
fn status_response(
    service: &ChatService,
    value: Option<ChatExecutionOwnerCommit>,
) -> Result<ChatOwnerResponse, ChatServiceError> {
    Ok(ChatOwnerResponse::Status {
        owner: value
            .as_ref()
            .map(|value| ChatOwnerStatus::from_commit(service, value))
            .transpose()?,
    })
}
