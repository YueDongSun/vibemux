//! Fixed local-diagnostic process boundary; never a generic executable bridge.

use serde_json::Value;
use tokio::sync::watch;
use vibemux_harness::dispatch::ProcessExit;

#[cfg(windows)]
mod image_pin;
#[cfg(windows)]
mod runtime;
#[cfg(all(test, windows, feature = "test_helpers"))]
mod tests;

#[derive(Clone, Copy)]
struct ProcessReceipt {
    exit: ProcessExit,
    termination_succeeded: bool,
    root_reaped: bool,
    tree_empty: bool,
    stdout_drained: bool,
    stderr_drained: bool,
}

/// Metadata is available only after successful execution and joined cleanup.
/// This receipt supplies no Task/Run transition or authority over other resources.
pub struct LocalProbeCompletion {
    report: Option<Value>,
    error_code: Option<&'static str>,
    receipt: Option<ProcessReceipt>,
    not_spawned: bool,
}

impl LocalProbeCompletion {
    pub fn report(&self) -> Option<&Value> {
        self.report.as_ref()
    }
    pub fn error_code(&self) -> Option<&'static str> {
        self.error_code
    }
    pub fn is_quiescent(&self) -> bool {
        self.not_spawned
            || self.receipt.is_some_and(|receipt| {
                receipt.termination_succeeded
                    && receipt.root_reaped
                    && receipt.tree_empty
                    && receipt.stdout_drained
                    && receipt.stderr_drained
            })
    }
    fn rejected(code: &'static str) -> Self {
        Self {
            report: None,
            error_code: Some(code),
            receipt: None,
            not_spawned: true,
        }
    }
}
impl std::fmt::Debug for LocalProbeCompletion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalProbeCompletion")
            .field("error_code", &self.error_code)
            .field("quiescent", &self.is_quiescent())
            .finish_non_exhaustive()
    }
}

/// Run only the bundled first-party diagnostic CLI, without cache flags.
/// The host must own this future through completion and use `cancel` to stop it.
/// Dropping an observer does not replace that ownership or prove cleanup.
pub async fn run(cancel: watch::Receiver<bool>) -> LocalProbeCompletion {
    #[cfg(windows)]
    {
        runtime::run(cancel).await
    }
    #[cfg(not(windows))]
    {
        let _ = cancel;
        LocalProbeCompletion::rejected("local_probe_platform_unsupported")
    }
}
