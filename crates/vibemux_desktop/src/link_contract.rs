//! Closed presentation requests; private paths and payloads stay native-owned.
use crate::command_error::CommandError;
use serde::{Deserialize, Serialize};
use std::fmt;
use url::Url;
use uuid::Uuid;
use vibemux_types::{ProjectId, RunId, TaskId};

pub const MAX_LINK_URL_BYTES: usize = 2048;
const MAX_PROJECT_KEY_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeAction {
    Open,
    CopyPath,
    CopyBranch,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OpenLinkRequest {
    Https {
        url: String,
    },
    Worktree {
        project_key: String,
        task_id: TaskId,
        run_id: RunId,
        action: WorktreeAction,
    },
}

impl fmt::Debug for OpenLinkRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Https { .. } => formatter.debug_struct("Https").finish_non_exhaustive(),
            Self::Worktree {
                task_id,
                run_id,
                action,
                ..
            } => formatter
                .debug_struct("Worktree")
                .field("task_id", task_id)
                .field("run_id", run_id)
                .field("action", action)
                .finish_non_exhaustive(),
        }
    }
}

impl OpenLinkRequest {
    pub fn validate(&self) -> Result<(), CommandError> {
        match self {
            Self::Https { url } => validate_https(url).map(|_| ()),
            Self::Worktree {
                project_key,
                task_id,
                run_id,
                ..
            } => {
                if project_key.is_empty()
                    || project_key.len() > MAX_PROJECT_KEY_BYTES
                    || project_key.chars().any(char::is_control)
                    || Uuid::parse_str(&task_id.to_string()).map_or(true, |id| id.is_nil())
                    || Uuid::parse_str(&run_id.to_string()).map_or(true, |id| id.is_nil())
                {
                    return Err(CommandError::new(
                        "invalid_arguments",
                        "Check the project, Task and Run identifiers.",
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkOutcome {
    OpenRequested,
    PathCopied,
    BranchCopied,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenLinkReply {
    Https {
        outcome: LinkOutcome,
    },
    Worktree {
        project_id: ProjectId,
        task_id: TaskId,
        run_id: RunId,
        action: WorktreeAction,
        outcome: LinkOutcome,
    },
}

pub fn validate_https(value: &str) -> Result<Url, CommandError> {
    let invalid = || {
        CommandError::new(
            "invalid_link_url",
            "Open a complete HTTPS link without credentials or whitespace.",
        )
    };
    if value.is_empty()
        || value.len() > MAX_LINK_URL_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(invalid());
    }
    let url = Url::parse(value).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid());
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_validation_normalizes_only_complete_bounded_web_links() {
        assert_eq!(
            validate_https("HTTPS://Example.org/path?q=one#section")
                .expect("https")
                .as_str(),
            "https://example.org/path?q=one#section"
        );
        for value in [
            "",
            "https:",
            "https:///",
            "http://example.org",
            "file:///C:/x",
            "javascript:alert(1)",
            "//example.org",
            "https://user:secret@example.org",
            "https://example.org/\n",
            " https://example.org",
            "https://example.org/a b",
        ] {
            assert_eq!(
                validate_https(value).expect_err("refused").code,
                "invalid_link_url"
            );
        }
        assert!(
            validate_https(&format!(
                "https://example.org/{}",
                "x".repeat(MAX_LINK_URL_BYTES)
            ))
            .is_err()
        );
        assert!(
            !format!(
                "{:?}",
                OpenLinkRequest::Https {
                    url: "https://example.org/?private=value".into()
                }
            )
            .contains("private")
        );
    }
}
