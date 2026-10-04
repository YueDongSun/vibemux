//! Driver for one DSH SDK session: the effect half of the substrate.
//!
//! `vibemux_harness::dsh` owns the pure contracts — wire vocabulary, session
//! state machine, bounded record buffer. This crate owns everything that
//! touches the operating system: resolving the operator configuration,
//! spawning the Node launcher pair through `vibemux_platform::ProcessTree`,
//! framing stdout, enforcing deadlines, and terminating the contained tree
//! (ADR 030).
//!
//! Two consumers are planned. `vibemux_dsh_tui` drives a session directly for
//! an operator, and a later stage wires the same driver behind
//! `vibemuxd`'s control operations. Keeping the effects here rather than in
//! the terminal UI means the daemon does not have to reimplement framing,
//! deadlines, or containment.

#![forbid(unsafe_code)]

pub mod config;
pub mod driver;

use thiserror::Error;

pub use config::{
    DSH_SESSION_CONFIG_FILE_NAME, DshSessionConfig, DshSessionConfigFile, DshSessionConfigSummary,
    load_session_config,
};
pub use driver::{DshDriver, DshLaunch, DshPoll, DshPollRecord};

/// Stable, machine-readable driver failures. Every variant maps to one
/// `dsh_driver_*` code and carries no prompt text, path, or credential.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DshDriverError {
    #[error("the DSH session configuration is absent or unreadable")]
    Unconfigured,
    #[error("the DSH session configuration is invalid")]
    ConfigInvalid,
    #[error("the DSH launcher image is not a trusted native image")]
    ExecutableUntrusted,
    #[error("the DSH entry script is not owned by the DSH package")]
    EntryScriptUntrusted,
    #[error("the DSH runtime could not be spawned")]
    SpawnFailed,
    #[error("the DSH runtime stdin is unavailable")]
    StdinUnavailable,
    #[error("writing to the DSH runtime failed")]
    WriteFailed,
    #[error("the DSH runtime stdout closed before the session finished")]
    TransportClosed,
    #[error("the DSH runtime produced a frame above the configured ceiling")]
    FrameTooLarge,
    #[error("the DSH runtime produced a line that is not valid UTF-8")]
    InvalidUtf8,
    #[error("the DSH initialize handshake exceeded its deadline")]
    InitializeTimeout,
    #[error("the DSH turn exceeded its deadline")]
    TurnTimeout,
    #[error("the DSH runtime did not exit within its shutdown grace period")]
    ShutdownTimeout,
    #[error("the DSH session is not in a state that accepts this operation")]
    InvalidState,
}

impl DshDriverError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unconfigured => "dsh_driver_unconfigured",
            Self::ConfigInvalid => "dsh_driver_config_invalid",
            Self::ExecutableUntrusted => "dsh_driver_executable_untrusted",
            Self::EntryScriptUntrusted => "dsh_driver_entry_script_untrusted",
            Self::SpawnFailed => "dsh_driver_spawn_failed",
            Self::StdinUnavailable => "dsh_driver_stdin_unavailable",
            Self::WriteFailed => "dsh_driver_write_failed",
            Self::TransportClosed => "dsh_driver_transport_closed",
            Self::FrameTooLarge => "dsh_driver_frame_too_large",
            Self::InvalidUtf8 => "dsh_driver_invalid_utf8",
            Self::InitializeTimeout => "dsh_driver_initialize_timeout",
            Self::TurnTimeout => "dsh_driver_turn_timeout",
            Self::ShutdownTimeout => "dsh_driver_shutdown_timeout",
            Self::InvalidState => "dsh_driver_invalid_state",
        }
    }

    /// Whether the session's state machine already recorded this failure.
    #[must_use]
    pub const fn is_session_failure(&self) -> bool {
        matches!(
            self,
            Self::TransportClosed
                | Self::FrameTooLarge
                | Self::InvalidUtf8
                | Self::InitializeTimeout
                | Self::TurnTimeout
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn driver_codes_are_unique_and_prefixed() {
        let all = [
            DshDriverError::Unconfigured,
            DshDriverError::ConfigInvalid,
            DshDriverError::ExecutableUntrusted,
            DshDriverError::EntryScriptUntrusted,
            DshDriverError::SpawnFailed,
            DshDriverError::StdinUnavailable,
            DshDriverError::WriteFailed,
            DshDriverError::TransportClosed,
            DshDriverError::FrameTooLarge,
            DshDriverError::InvalidUtf8,
            DshDriverError::InitializeTimeout,
            DshDriverError::TurnTimeout,
            DshDriverError::ShutdownTimeout,
            DshDriverError::InvalidState,
        ];
        let mut codes = BTreeSet::new();
        for error in all {
            let code = error.code();
            assert!(code.starts_with("dsh_driver_"), "{code}");
            assert!(codes.insert(code), "duplicate code {code}");
        }
    }
}
