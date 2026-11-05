//! Backend-neutral tool grants shared by workflows and owned conversations.
use serde::{Deserialize, Serialize};

/// The adapter maps these bounded grants to qualified vendor capabilities.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    ReadFiles,
    SearchFiles,
    EditFiles,
    WriteFiles,
    RunDevTests,
}

impl ToolName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadFiles => "read_files",
            Self::SearchFiles => "search_files",
            Self::EditFiles => "edit_files",
            Self::WriteFiles => "write_files",
            Self::RunDevTests => "run_dev_tests",
        }
    }
}
