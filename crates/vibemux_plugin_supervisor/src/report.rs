use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginStderrReport {
    pub captured_bytes: u64,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginExitReport {
    pub graceful: bool,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stderr: PluginStderrReport,
}
