use thiserror::Error;
use vibemuxd::process::DaemonPathError;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DaemonCliError {
    #[error("daemon bootstrap configuration is invalid")]
    InvalidConfiguration,
    #[error("daemon project root is invalid")]
    InvalidProjectRoot,
    #[error("daemon executable is unavailable")]
    ExecutableUnavailable,
    #[error("daemon is not running")]
    NotRunning,
    #[error("daemon runtime state is stale: {reason_code}")]
    StaleRuntime { reason_code: String },
    #[error("daemon process could not be spawned")]
    SpawnFailed,
    #[error("daemon process exited before becoming healthy")]
    StartFailed,
    #[error("daemon startup exceeded its deadline")]
    StartupTimeout,
    #[error("daemon shutdown exceeded its deadline")]
    ShutdownTimeout,
    #[error("daemon control operation failed: {code}")]
    Control { code: String },
    #[error("daemon recovery confirmation does not match current artifacts")]
    RecoveryConfirmationMismatch,
    #[error("daemon recovery is blocked: {reason_code}")]
    RecoveryBlocked { reason_code: String },
    #[error("daemon recovery artifact operation failed: {code}")]
    RecoveryArtifact { code: String },
    #[error("CLI arguments are invalid")]
    InvalidArguments,
}

impl DaemonCliError {
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidConfiguration => "daemon_invalid_configuration",
            Self::InvalidProjectRoot => "daemon_invalid_project_root",
            Self::ExecutableUnavailable => "daemon_executable_unavailable",
            Self::NotRunning => "daemon_not_running",
            Self::StaleRuntime { .. } => "daemon_stale_runtime",
            Self::SpawnFailed => "daemon_spawn_failed",
            Self::StartFailed => "daemon_start_failed",
            Self::StartupTimeout => "daemon_startup_timeout",
            Self::ShutdownTimeout => "daemon_shutdown_timeout",
            Self::Control { code } => code,
            Self::RecoveryConfirmationMismatch => "daemon_recovery_confirmation_mismatch",
            Self::RecoveryBlocked { .. } => "daemon_recovery_blocked",
            Self::RecoveryArtifact { code } => code,
            Self::InvalidArguments => "cli_invalid_arguments",
        }
    }
}

impl From<DaemonPathError> for DaemonCliError {
    fn from(error: DaemonPathError) -> Self {
        let code = error.code();
        match error {
            DaemonPathError::InvalidProjectRoot => Self::InvalidProjectRoot,
            DaemonPathError::RuntimeDirectoryUnavailable
            | DaemonPathError::ControlRuntimeUnavailable
            | DaemonPathError::UnsafeRuntimeDirectory
            | DaemonPathError::UnsafeControlRuntime
            | DaemonPathError::ControlRuntimeSecurityInvalid
            | DaemonPathError::UnsafeDatabasePath
            | DaemonPathError::DatabaseCollision
            | DaemonPathError::LegacyRuntimeConflict => Self::Control {
                code: code.to_string(),
            },
        }
    }
}
