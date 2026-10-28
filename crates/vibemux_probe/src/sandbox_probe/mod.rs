//! Explicit offline OS diagnostic; never a workflow capability credential.

pub mod evidence;
mod fixture;
#[cfg(windows)]
mod runner;

use evidence::{CheckObservation, IndependentEvidence, SCHEMA_VERSION, SUITE};
use serde::Serialize;
use std::path::Path;

#[doc(hidden)]
pub use fixture::fixture_entry;

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    Passed,
    Failed,
    Unavailable,
}

/// A diagnostic observation, deliberately not accepted as workflow authority.
#[derive(Debug, Serialize)]
pub struct SandboxProbeReport {
    pub schema_version: u16,
    pub suite: &'static str,
    pub run_id: String,
    pub outcome: ProbeOutcome,
    pub reason_code: Option<&'static str>,
    pub backend_cli_version: Option<String>,
    pub backend_exit_code: Option<i32>,
    pub requested_backend: &'static str,
    pub requested_network_mode: &'static str,
    pub managed_requirements_requested: bool,
    pub reusable_qualification: bool,
    pub retained_fixture_name: Option<String>,
    pub checks: Vec<CheckObservation>,
    pub independent: IndependentEvidence,
    pub stderr_bytes: usize,
}

impl SandboxProbeReport {
    fn new() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            suite: SUITE,
            run_id: uuid::Uuid::new_v4().to_string(),
            outcome: ProbeOutcome::Unavailable,
            reason_code: Some("sandbox_probe_unavailable"),
            backend_cli_version: None,
            backend_exit_code: None,
            requested_backend: "mxc",
            requested_network_mode: "disabled",
            managed_requirements_requested: true,
            reusable_qualification: false,
            retained_fixture_name: None,
            checks: Vec::new(),
            independent: IndependentEvidence::default(),
            stderr_bytes: 0,
        }
    }
}

/// Run the fixed offline diagnostic beneath a caller-owned scratch parent.
/// Never invokes a model, changes saved policy, or updates daemon state.
pub async fn run_offline_probe(backend: &Path, scratch_parent: &Path) -> SandboxProbeReport {
    #[cfg(windows)]
    {
        runner::run(backend, scratch_parent).await
    }
    #[cfg(not(windows))]
    {
        let _ = (backend, scratch_parent);
        let mut report = SandboxProbeReport::new();
        report.reason_code = Some("sandbox_platform_unsupported");
        report
    }
}
