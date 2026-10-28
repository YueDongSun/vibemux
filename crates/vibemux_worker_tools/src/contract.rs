use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_workflow::{PathPattern, Sha256Digest};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_FILES: usize = 256;
pub const MAX_FILE_BYTES: usize = 64 * 1024;
pub const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_OPERATIONS: usize = 256;
pub const MAX_FRAME_BYTES: usize = 512 * 1024;
pub const MAX_BOOTSTRAP_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextFile {
    pub path: String,
    pub content: String,
}

/// Supplied by the trusted owner; never accepted as an MCP operation.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    pub schema_version: u32,
    pub attempt_id: Uuid,
    pub grants: WritableGrant,
    pub owned_paths: Vec<PathPattern>,
    pub read_scopes: Vec<PathPattern>,
    pub forbidden_paths: Vec<PathPattern>,
    pub files: Vec<TextFile>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Read {
        path: String,
    },
    Search {
        prefix: String,
    },
    Create {
        path: String,
        content: String,
    },
    Edit {
        path: String,
        expected_revision: u64,
        content: String,
    },
}

impl Operation {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Read { .. } => "read_file",
            Self::Search { .. } => "search_paths",
            Self::Create { .. } => "create_file",
            Self::Edit { .. } => "edit_file",
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileVersion {
    pub path: String,
    pub revision: u64,
    pub byte_count: usize,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationResult {
    Read { file: FileVersion, content: String },
    Search { files: Vec<FileVersion> },
    Changed { file: FileVersion },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationReceipt {
    pub sequence: u64,
    pub operation: String,
    pub status: String,
    pub code: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub schema_version: u32,
    pub attempt_id: Uuid,
    pub base_snapshot: Sha256Digest,
    pub operations: Vec<OperationReceipt>,
    pub changed_files: Vec<TextFile>,
}

/// Private owner recovery evidence; never part of the MCP tool surface.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRecord {
    pub schema_version: u32,
    pub attempt_id: Uuid,
    pub base_snapshot: Sha256Digest,
    pub retained_directory: std::path::PathBuf,
    pub operations: Vec<OperationReceipt>,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ToolError {
    #[error("worker tools bootstrap is invalid")]
    InvalidBootstrap,
    #[error("worker tools path is invalid")]
    InvalidPath,
    #[error("worker tools operation is not granted")]
    Denied,
    #[error("worker tools path is outside its scope")]
    OutsideScope,
    #[error("worker tools path is protected")]
    ProtectedPath,
    #[error("worker tools text exceeds its limits")]
    ContentLimit,
    #[error("worker tools target already exists")]
    AlreadyExists,
    #[error("worker tools target does not exist")]
    Missing,
    #[error("worker tools target revision changed")]
    StaleRevision,
    #[error("worker tools resource limit reached")]
    ResourceLimit,
    #[error("worker tools file I/O failed before mutation")]
    Io,
    #[error("worker tools mutation outcome is unknown")]
    MutationUnknown,
    #[error("worker tools workspace identity changed")]
    WorkspaceChanged,
    #[error("worker tools workspace is quarantined")]
    Quarantined,
    #[error("worker tools command execution is not qualified")]
    ExecutionUnsupported,
    #[error("worker tools test execution requires owner reconciliation")]
    ExecutionAlreadyAdmitted,
}

impl ToolError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidBootstrap => "worker_tools_invalid_bootstrap",
            Self::InvalidPath => "worker_tools_invalid_path",
            Self::Denied => "worker_tools_denied",
            Self::OutsideScope => "worker_tools_outside_scope",
            Self::ProtectedPath => "worker_tools_protected_path",
            Self::ContentLimit => "worker_tools_content_limit",
            Self::AlreadyExists => "worker_tools_already_exists",
            Self::Missing => "worker_tools_missing",
            Self::StaleRevision => "worker_tools_stale_revision",
            Self::ResourceLimit => "worker_tools_resource_limit",
            Self::Io => "worker_tools_io_failed",
            Self::MutationUnknown => "worker_tools_mutation_unknown",
            Self::WorkspaceChanged => "worker_tools_workspace_changed",
            Self::Quarantined => "worker_tools_quarantined",
            Self::ExecutionUnsupported => "worker_tools_execution_unsupported",
            Self::ExecutionAlreadyAdmitted => "worker_tools_execution_already_admitted",
        }
    }
}
