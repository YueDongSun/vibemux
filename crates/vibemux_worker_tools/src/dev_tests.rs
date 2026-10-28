//! Owner-admitted development tests over a separate disposable text snapshot.
//! Execution evidence is never an assertion oracle or workflow acceptance.

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_workflow::PathPattern;

use crate::contract::{Bootstrap, SCHEMA_VERSION, TextFile, ToolError};

#[cfg(windows)]
mod runner;
#[cfg(windows)]
mod snapshot;

pub const DEV_TEST_SCHEMA: u32 = 1;
pub const MAX_TEST_ENTRIES: usize = 16;
pub const MAX_TIMEOUT_MS: u64 = 60_000;
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

pub(crate) fn input_identity(
    files: &[TextFile],
) -> Result<vibemux_workflow::Sha256Digest, ToolError> {
    let mut ordered: Vec<_> = files.iter().collect();
    ordered.sort_by(|a, b| a.path.cmp(&b.path));
    let encoded = serde_json::to_vec(&ordered).map_err(|_| ToolError::InvalidBootstrap)?;
    Ok(vibemux_workflow::Sha256Digest::of_fields(
        "vibemux.dev_tests.input.v1",
        &[&encoded],
    ))
}

/// Trusted owner input. There is no command, environment, preload or reporter
/// field. All files must be disclosable because even exit status can leak data.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DevTestRequest {
    pub schema_version: u32,
    pub attempt_id: Uuid,
    pub operation_id: Uuid,
    pub suite_id: String,
    pub grants: WritableGrant,
    pub read_scopes: Vec<PathPattern>,
    pub forbidden_paths: Vec<PathPattern>,
    pub files: Vec<TextFile>,
    pub test_entries: Vec<String>,
    pub timeout_ms: u64,
    pub output_limit_bytes: usize,
}

impl DevTestRequest {
    pub fn validate(&self) -> Result<(), ToolError> {
        if !self.grants.read_files || !self.grants.run_dev_tests {
            return Err(ToolError::Denied);
        }
        if self.schema_version != DEV_TEST_SCHEMA
            || self.attempt_id.is_nil()
            || self.operation_id.is_nil()
            || self.suite_id.is_empty()
            || self.suite_id.len() > 64
            || !self
                .suite_id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            || self.timeout_ms == 0
            || self.timeout_ms > MAX_TIMEOUT_MS
            || self.output_limit_bytes == 0
            || self.output_limit_bytes > MAX_OUTPUT_BYTES
            || self.test_entries.is_empty()
            || self.test_entries.len() > MAX_TEST_ENTRIES
        {
            return Err(ToolError::InvalidBootstrap);
        }
        // Reuse the text/path limits, case-alias rejection and protected paths.
        // No mutation scope is supplied: EVERY input needs a read scope.
        crate::workspace::validate_bootstrap(&Bootstrap {
            schema_version: SCHEMA_VERSION,
            attempt_id: self.attempt_id,
            grants: WritableGrant::READ_ONLY,
            owned_paths: vec![],
            read_scopes: self.read_scopes.clone(),
            forbidden_paths: self.forbidden_paths.clone(),
            files: self.files.clone(),
        })?;
        let mut entries = std::collections::BTreeSet::new();
        for path in &self.test_entries {
            crate::workspace::validate_path(path)?;
            if !entries.insert(path)
                || !self.files.iter().any(|file| &file.path == path)
                || ![".cjs", ".mjs", ".js"]
                    .iter()
                    .any(|extension| path.ends_with(extension))
            {
                return Err(ToolError::InvalidPath);
            }
        }
        if self.files.iter().any(|file| {
            file.path
                .split('/')
                .next()
                .is_some_and(|first| first.eq_ignore_ascii_case(".vibemux_test_scratch"))
        }) {
            return Err(ToolError::ProtectedPath);
        }
        Ok(())
    }
}

/// Exact trusted executables, outside the candidate copy. The owner is responsible
/// for their integrity and exclusive path ownership throughout this invocation.
pub struct DevTestRuntime {
    pub backend: std::path::PathBuf,
    pub node: std::path::PathBuf,
    pub trampoline: std::path::PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOutcome {
    Completed,
    TimedOut,
    Cancelled,
    OutputLimit,
    InfrastructureFailed,
}

/// Private owner result. stdout/stderr remain untrusted candidate claims, even
/// when the process exits zero. This type has no test-pass/accepted field.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DevTestReport {
    pub schema_version: u32,
    pub attempt_id: Uuid,
    pub operation_id: Uuid,
    pub suite_id: String,
    pub runtime_profile: String,
    pub input_snapshot: vibemux_workflow::Sha256Digest,
    pub execution: ExecutionOutcome,
    pub reason_code: Option<String>,
    pub backend_exit_code: Option<i32>,
    pub process_started: bool,
    pub process_exited: bool,
    pub process_tree_empty: bool,
    pub output_complete: bool,
    pub untrusted_stdout: String,
    pub untrusted_stderr: String,
    pub snapshot_removed: bool,
    pub retained_directory: Option<std::path::PathBuf>,
    pub assertions_verified: bool,
    pub reusable_qualification: bool,
}

/// Synchronous preparation boundary; run on an owner-controlled blocking thread
/// when embedding in an async daemon. The standalone command calls it before
/// entering its runtime. This component is currently Windows-only.
pub fn prepare_dev_tests(
    request: DevTestRequest,
    runtime: &DevTestRuntime,
    scratch_parent: &std::path::Path,
) -> Result<PreparedDevTest, ToolError> {
    request.validate()?;
    #[cfg(windows)]
    {
        runner::prepare(request, runtime, scratch_parent)
    }
    #[cfg(not(windows))]
    {
        let _ = (runtime, scratch_parent);
        Err(ToolError::ExecutionUnsupported)
    }
}

/// Holds the fixed command and separately materialized snapshot. It never
/// removes the snapshot on Drop; an interrupted owner must reconcile it.
pub struct PreparedDevTest {
    #[cfg(not(windows))]
    unavailable: std::convert::Infallible,
    #[cfg(windows)]
    snapshot: snapshot::Snapshot,
    #[cfg(windows)]
    request: DevTestRequest,
    #[cfg(windows)]
    command: std::process::Command,
    #[cfg(windows)]
    trampoline: std::path::PathBuf,
}

impl PreparedDevTest {
    /// One execution, without retries. Keep this future alive until it returns
    /// and cancel through the channel (true OR sender closure). Dropping it
    /// requests Job termination but cannot certify reaping/cleanup; its owner
    /// record and snapshot remain for reconciliation before any retry.
    pub async fn execute(
        self,
        cancellation: &mut tokio::sync::watch::Receiver<bool>,
    ) -> ExecutedDevTest {
        #[cfg(windows)]
        {
            runner::execute(self, cancellation).await
        }
        #[cfg(not(windows))]
        {
            let _ = cancellation;
            match self.unavailable {}
        }
    }
}

pub struct ExecutedDevTest {
    #[cfg(windows)]
    snapshot: snapshot::Snapshot,
    report: DevTestReport,
}

impl ExecutedDevTest {
    pub fn report(&self) -> &DevTestReport {
        &self.report
    }

    /// Explicit synchronous cleanup boundary, after process supervision ends.
    /// Recursive removal has no hard wall-clock guarantee. It never follows a
    /// candidate-supplied deletion target or repairs permissions to force success.
    pub fn finish(self) -> DevTestReport {
        #[cfg(windows)]
        {
            let Self {
                snapshot,
                mut report,
            } = self;
            if !report.process_started || (report.process_exited && report.process_tree_empty) {
                match snapshot.remove() {
                    Ok(()) => {
                        report.snapshot_removed = true;
                        report.retained_directory = None;
                    }
                    Err(error) => {
                        if report.reason_code.is_none() {
                            report.reason_code = Some(format!(
                                "dev_test_cleanup_failed_{}",
                                error.raw_os_error().unwrap_or(0)
                            ));
                        }
                    }
                }
            }
            report
        }
        #[cfg(not(windows))]
        {
            self.report
        }
    }
}
