//! Runs the trusted verifier on one subject directory (ADR 031 §6).
//!
//! Each suite is one contained process behind the launch trampoline, with a
//! cleared environment, the pinned argv template, and a deadline. The
//! verifier writes its result file outside the subject; the daemon reads it
//! with a size bound and maps it to a [`SuiteResult`]. A missing,
//! unparseable, or inconsistent result is `blocked`, never a pass. The
//! recorded command is the argv template, so no path reaches a receipt.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use tokio::{io::AsyncReadExt, time::Instant};
use vibemux_harness::dispatch::launch_spec::{LaunchSpec, SYSTEM_ENVIRONMENT_NAMES};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    gates::{SuiteResult, SuiteStatus},
};

use super::{
    error::WorkflowError,
    settings::{
        CANDIDATE_PLACEHOLDER, LoadedWorkflowConfig, OUT_PLACEHOLDER, VERIFIER_DIR_PLACEHOLDER,
    },
};
use crate::harness_dispatch::{
    child_path_text,
    native_process::{LaunchPlan, NativeProcess, resolve_environment},
};

const MAX_RESULT_BYTES: u64 = 1024 * 1024;
const MAX_FAILED_TEST_NAMES: usize = 32;
const MAX_TEST_NAME_CHARS: usize = 160;
/// Grace after the suite deadline for the tree to be terminated.
const EXIT_GRACE: Duration = Duration::from_secs(5);
const STDOUT_CHUNK_BYTES: usize = 8 * 1024;

/// The trusted verifier's result file (schema 1).
#[derive(Debug, Deserialize)]
struct VerifierOutput {
    schema_version: u32,
    suite: String,
    status: String,
    tests_total: u32,
    tests_passed: u32,
    tests_failed: u32,
    failed_tests: Vec<String>,
    duration_ms: u64,
    #[serde(default)]
    node_version: Option<String>,
    #[serde(default)]
    blocked_reason: Option<String>,
}

pub(crate) struct SuiteRun {
    pub results: Vec<SuiteResult>,
    pub tool_versions: BTreeMap<String, String>,
}

/// Runs `suites` against `subject` and returns one result per suite.
pub(crate) async fn run_suites(
    config: &LoadedWorkflowConfig,
    trampoline: &Path,
    suites: &[SpecIdentifier],
    subject: &Path,
    scratch: &Path,
) -> Result<SuiteRun, WorkflowError> {
    config.check_verifier_unchanged()?;
    let subject_text = child_path_text(subject).ok_or(WorkflowError::Verifier)?;
    let verifier_text =
        child_path_text(&config.verifier_directory).ok_or(WorkflowError::Verifier)?;
    std::fs::create_dir_all(scratch).map_err(|_| WorkflowError::Verifier)?;
    let mut results = Vec::with_capacity(suites.len());
    let mut tool_versions = BTreeMap::new();
    for suite_id in suites {
        let template = config
            .config
            .verifier
            .suites
            .get(suite_id)
            .ok_or(WorkflowError::Verifier)?;
        let out = scratch.join(format!("{suite_id}_{}.json", uuid::Uuid::new_v4().simple()));
        let out_text = child_path_text(&out).ok_or(WorkflowError::Verifier)?;
        let arguments: Vec<String> = template
            .iter()
            .map(|argument| {
                argument
                    .replace(VERIFIER_DIR_PLACEHOLDER, &verifier_text)
                    .replace(CANDIDATE_PLACEHOLDER, &subject_text)
                    .replace(OUT_PLACEHOLDER, &out_text)
            })
            .collect();
        let mut command = vec![
            config
                .config
                .verifier
                .executable
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("verifier")
                .to_ascii_lowercase(),
        ];
        command.extend(template.iter().cloned());
        let started = Instant::now();
        let exit_code = run_one(config, trampoline, arguments, scratch).await;
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let output = read_result(&out);
        let _ = std::fs::remove_file(&out);
        let result = map_result(suite_id, command, exit_code, elapsed, output);
        if let Some(version) = result.1 {
            tool_versions.insert("node".to_string(), version);
        }
        results.push(result.0);
    }
    tool_versions.insert(
        "vibemux_verifier_runner".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );
    Ok(SuiteRun {
        results,
        tool_versions,
    })
}

/// Exit code of the contained verifier, or `None` when it could not start,
/// timed out, or was killed.
async fn run_one(
    config: &LoadedWorkflowConfig,
    trampoline: &Path,
    arguments: Vec<String>,
    working_directory: &Path,
) -> Option<i32> {
    let mut names: Vec<String> = SYSTEM_ENVIRONMENT_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    names.extend(config.config.verifier.environment_names.iter().cloned());
    let environment = resolve_environment(&names).ok()?;
    let plan = LaunchPlan {
        trampoline: trampoline.to_path_buf(),
        executable: PathBuf::from(child_path_text(&config.verifier_executable)?),
        spec: LaunchSpec {
            executable: config.verifier_executable.clone(),
            arguments,
            environment_names: names,
        },
        working_directory: PathBuf::from(child_path_text(working_directory)?),
    };
    let mut process = NativeProcess::spawn(&plan, environment).await.ok()?;
    process.close_stdin();
    let deadline = Instant::now() + Duration::from_millis(config.config.verifier.timeout_ms);
    let drained = tokio::time::timeout_at(deadline, async {
        let mut buffer = vec![0_u8; STDOUT_CHUNK_BYTES];
        loop {
            match process.stdout.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await
    .is_ok();
    let exit_deadline = if drained {
        Instant::now() + EXIT_GRACE
    } else {
        Instant::now()
    };
    let (exit, _) = process.finish(!drained, exit_deadline).await;
    if exit.forced_termination {
        return None;
    }
    exit.exit_code
}

fn read_result(path: &Path) -> Option<(VerifierOutput, Sha256Digest)> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_RESULT_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let output: VerifierOutput = serde_json::from_slice(&bytes).ok()?;
    Some((output, Sha256Digest::of(&bytes)))
}

/// A trusted test name without anything after a detail separator, so a
/// runner diagnostic (which may hold a path) never reaches a receipt.
fn test_name(name: &str) -> String {
    let head = name.split(" (").next().unwrap_or_default();
    head.chars().take(MAX_TEST_NAME_CHARS).collect()
}

fn blocked(
    suite_id: &SpecIdentifier,
    command: Vec<String>,
    exit_code: Option<i32>,
    duration_ms: u64,
    reason: &str,
) -> SuiteResult {
    SuiteResult {
        suite_id: suite_id.clone(),
        status: SuiteStatus::Blocked,
        tests_total: 0,
        tests_passed: 0,
        tests_failed: 0,
        failed_tests: Vec::new(),
        command,
        exit_code,
        duration_ms,
        output_sha256: Sha256Digest::of(b""),
        blocked_reason: Some(reason.to_string()),
    }
}

fn map_result(
    suite_id: &SpecIdentifier,
    command: Vec<String>,
    exit_code: Option<i32>,
    elapsed_ms: u64,
    output: Option<(VerifierOutput, Sha256Digest)>,
) -> (SuiteResult, Option<String>) {
    let Some((output, output_sha256)) = output else {
        let reason = if exit_code.is_none() {
            "verifier_timeout_or_spawn_failure"
        } else {
            "verifier_output_missing"
        };
        return (
            blocked(suite_id, command, exit_code, elapsed_ms, reason),
            None,
        );
    };
    let status = match (output.status.as_str(), exit_code) {
        ("passed", Some(0)) => SuiteStatus::Passed,
        ("failed", Some(1)) => SuiteStatus::Failed,
        ("blocked", Some(2)) => SuiteStatus::Blocked,
        _ => {
            return (
                blocked(
                    suite_id,
                    command,
                    exit_code,
                    elapsed_ms,
                    "verifier_inconsistent",
                ),
                output.node_version,
            );
        }
    };
    if output.schema_version != 1 || output.suite != suite_id.as_str() {
        return (
            blocked(
                suite_id,
                command,
                exit_code,
                elapsed_ms,
                "verifier_output_mismatch",
            ),
            output.node_version,
        );
    }
    let blocked_reason = (status == SuiteStatus::Blocked).then(|| {
        output
            .blocked_reason
            .as_deref()
            .map_or("verifier_blocked".to_string(), test_name)
    });
    let result = SuiteResult {
        suite_id: suite_id.clone(),
        status,
        tests_total: output.tests_total,
        tests_passed: output.tests_passed,
        tests_failed: output.tests_failed,
        failed_tests: output
            .failed_tests
            .iter()
            .take(MAX_FAILED_TEST_NAMES)
            .map(|name| test_name(name))
            .collect(),
        command,
        exit_code,
        duration_ms: output.duration_ms,
        output_sha256,
        blocked_reason,
    };
    (result, output.node_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suite() -> SpecIdentifier {
        SpecIdentifier::new("api").expect("suite")
    }

    fn output(status: &str) -> VerifierOutput {
        VerifierOutput {
            schema_version: 1,
            suite: "api".into(),
            status: status.into(),
            tests_total: 3,
            tests_passed: 2,
            tests_failed: 1,
            failed_tests: vec!["rejects blank titles (C:\\private\\path)".into()],
            duration_ms: 10,
            node_version: Some("v22.0.0".into()),
            blocked_reason: None,
        }
    }

    #[test]
    fn exit_code_and_status_must_agree_and_details_are_dropped() {
        let digest = Sha256Digest::of(b"out");
        let (failed, version) = map_result(
            &suite(),
            vec![],
            Some(1),
            5,
            Some((output("failed"), digest)),
        );
        assert_eq!(failed.status, SuiteStatus::Failed);
        assert_eq!(failed.failed_tests, vec!["rejects blank titles"]);
        assert_eq!(version.as_deref(), Some("v22.0.0"));
        let (lying, _) = map_result(
            &suite(),
            vec![],
            Some(1),
            5,
            Some((output("passed"), digest)),
        );
        assert_eq!(lying.status, SuiteStatus::Blocked);
        let (missing, _) = map_result(&suite(), vec![], Some(0), 5, None);
        assert_eq!(missing.status, SuiteStatus::Blocked);
        let (timeout, _) = map_result(&suite(), vec![], None, 5, Some((output("passed"), digest)));
        assert_eq!(timeout.status, SuiteStatus::Blocked);
    }
}
