#![forbid(unsafe_code)]
//! Reviewed platform boundaries that remain independent from domain and storage types.

#[cfg(windows)]
mod windows_security;

#[cfg(windows)]
pub use windows_security::{
    WindowsAclSummary, secure_user_directory, verify_restricted_path_acl,
    verify_restricted_path_acls,
};

use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum PlatformError {
    #[error("system platform helper is unavailable")]
    HelperUnavailable,
    #[error("system platform helper exceeded its deadline")]
    HelperTimeout,
    #[error("system platform helper rejected the operation")]
    HelperRejected,
    #[error("platform access-control verification failed at stage {stage}")]
    AccessControlInvalid { stage: u32 },
    /// A batched verification failed at the 1-based `index` of the
    /// requested path list. Carries no path text (ADR 020); the stage
    /// meanings match [`PlatformError::AccessControlInvalid`].
    #[error("platform access-control verification failed at stage {stage} for path index {index}")]
    AccessControlInvalidAt { stage: u32, index: u32 },
}

impl PlatformError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::HelperUnavailable => "platform_helper_unavailable",
            Self::HelperTimeout => "platform_helper_timeout",
            Self::HelperRejected => "platform_helper_rejected",
            Self::AccessControlInvalid { .. } | Self::AccessControlInvalidAt { .. } => {
                "platform_access_control_invalid"
            }
        }
    }
}
