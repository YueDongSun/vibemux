//! Explicit deny-only diagnostics, separate from production approval authority.
use serde::Serialize;
use std::time::Duration;
use uuid::Uuid;

use super::RuntimeCleanup;
pub use super::approval_probe::{ProbeError, ProbeInitChecks, ProbeReport};

mod driver;
mod launch;

pub use launch::{
    DiagnosticCandidate, DiagnosticCandidateSummary, DiagnosticExecution,
    DiagnosticPreparationError,
};

const DIAGNOSTIC_REPORT_VERSION: u32 = 3;
const DIAGNOSTIC_INITIALIZE_LIMIT: Duration = Duration::from_secs(15);
const DIAGNOSTIC_TURN_LIMIT: Duration = Duration::from_secs(60);
const DIAGNOSTIC_SHUTDOWN_LIMIT: Duration = Duration::from_secs(3);
const DIAGNOSTIC_OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;

/// Finite process limits; they do not bound the vendor's internal model steps.
#[derive(Clone, Copy, Debug)]
pub struct DiagnosticLimits {
    pub initialize: Duration,
    pub turn: Duration,
    pub shutdown: Duration,
    pub output_bytes: u64,
}

impl Default for DiagnosticLimits {
    fn default() -> Self {
        Self {
            initialize: DIAGNOSTIC_INITIALIZE_LIMIT,
            turn: DIAGNOSTIC_TURN_LIMIT,
            shutdown: DIAGNOSTIC_SHUTDOWN_LIMIT,
            output_bytes: DIAGNOSTIC_OUTPUT_LIMIT,
        }
    }
}

impl DiagnosticLimits {
    pub(super) fn valid(self) -> bool {
        !self.initialize.is_zero()
            && self.initialize <= DIAGNOSTIC_INITIALIZE_LIMIT
            && !self.turn.is_zero()
            && self.turn <= DIAGNOSTIC_TURN_LIMIT
            && !self.shutdown.is_zero()
            && self.shutdown <= DIAGNOSTIC_SHUTDOWN_LIMIT
            && self.output_bytes > 0
            && self.output_bytes <= DIAGNOSTIC_OUTPUT_LIMIT
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStop {
    ObservedTerminal,
    Cancelled,
    InitializeDeadline,
    TurnDeadline,
    OutputLimit,
    InputUnknown,
    ProtocolRejected,
    PreInputWork,
    EofWithoutTerminal,
    RootExited,
    ReadFailed,
    InvalidConfiguration,
}

/// Content-free observations, never a native-profile qualification certificate.
#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticReport {
    pub report_version: u32,
    pub observations: ProbeReport,
    pub initialize_written: bool,
    /// Exact control handshake accepted, independently of the flush receipt.
    pub initialize_accepted: bool,
    /// Queue acceptance may precede delivery; false Written never permits retry.
    pub input_queued: bool,
    pub input_written: bool,
    pub denial_queued: bool,
    pub denial_written: bool,
    pub stdout_bytes: u64,
    pub stop: DiagnosticStop,
    pub cleanup: DiagnosticCleanup,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct DiagnosticCleanup {
    pub root_reaped: bool,
    pub tree_empty: bool,
    pub cleanup_verified: bool,
    pub forced_termination: bool,
    pub exit_code: Option<i32>,
    pub stderr_bytes: u64,
}

impl From<RuntimeCleanup> for DiagnosticCleanup {
    fn from(cleanup: RuntimeCleanup) -> Self {
        Self {
            root_reaped: cleanup.root_reaped,
            tree_empty: cleanup.tree_empty,
            cleanup_verified: cleanup.cleanup_verified,
            forced_termination: cleanup.exit.forced_termination,
            exit_code: cleanup.exit.exit_code,
            stderr_bytes: cleanup.stderr_bytes,
        }
    }
}

/// Private input: it must never appear in Debug, a default log or the report.
pub(super) struct ProbeContext {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub working_directory: String,
    pub prompt: String,
    pub limits: DiagnosticLimits,
}
