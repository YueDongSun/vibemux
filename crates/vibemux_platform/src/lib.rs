#![deny(unsafe_code)]
//! Reviewed platform boundaries that remain independent from domain and storage types.
//!
//! `unsafe` is denied at this crate root and allowed in exactly one narrow
//! module: [`windows_acl_native`], the read-only Win32 ACL verification
//! surface reviewed and recorded by ADR 025 (amending ADR 020's read path).
//! ACL writes stay on the fixed, reviewed PowerShell companion in
//! [`windows_security`]. Every other workspace crate keeps
//! `#![forbid(unsafe_code)]`.

#[cfg(windows)]
mod windows_acl_native;
#[cfg(windows)]
mod windows_security;

#[cfg(windows)]
pub use windows_security::{
    WindowsAclSummary, secure_user_directory, verify_restricted_path_acl,
    verify_restricted_path_acls,
};

/// Shared ACL-loosening recipe for fail-closed tests across the workspace
/// (`vibemuxd` process/control tests, `vibemux_cli` classify tests). Test
/// builds only; enabled by consumers via the `test_helpers` feature.
#[cfg(all(windows, any(test, feature = "test_helpers")))]
pub mod test_helpers {
    pub use crate::windows_security::tests::{loosen_with_icacls, restore_with_icacls};
}

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
