//! Version-2 owner session. Candidate code runs only in ADR 045's separate copy.

use crate::{
    contract::{Bootstrap, RecoveryRecord, ToolError},
    dev_tests::{self, DevTestReport, DevTestRequest, DevTestRuntime},
    workspace::WorkerTools,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use vibemux_workflow::Sha256Digest;

pub const TEST_SESSION_SCHEMA: u32 = 2;
const MAX_SUITES: usize = 8;
const MAX_TEST_RUNS: usize = 16;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestSuite {
    pub suite_id: String,
    pub test_entries: Vec<String>,
    pub timeout_ms: u64,
    pub output_limit_bytes: usize,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestPolicy {
    pub backend: PathBuf,
    pub node: PathBuf,
    pub suites: Vec<TestSuite>,
    pub max_runs: usize,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestBootstrap {
    pub schema_version: u32,
    pub worker: Bootstrap,
    pub test_policy: TestPolicy,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestCall {
    pub suite_id: String,
    pub expected_revision: u64,
}

#[derive(Serialize)]
pub struct TestReceipt {
    pub operation_id: Uuid,
    pub suite_id: String,
    pub workspace_revision: u64,
    pub input_snapshot: Sha256Digest,
    pub status: String,
    pub error_code: Option<String>,
    pub report: Option<DevTestReport>,
}

/// Only this projection is eligible for a model response. Owner paths and the
/// full recovery receipt are not serialized through the MCP surface.
fn public_result(receipt: &TestReceipt) -> Value {
    let mut value = json!({"operation_id":receipt.operation_id,"suite_id":receipt.suite_id,
        "workspace_revision":receipt.workspace_revision,"input_snapshot":receipt.input_snapshot,
        "status":receipt.status,"error_code":receipt.error_code,"assertions_verified":false});
    if let Some(report) = &receipt.report {
        value["execution"] = json!(report.execution);
        value["reason_code"] = json!(report.reason_code);
        value["backend_exit_code"] = json!(report.backend_exit_code);
        value["process_started"] = json!(report.process_started);
        value["process_exited"] = json!(report.process_exited);
        value["process_tree_empty"] = json!(report.process_tree_empty);
        value["output_complete"] = json!(report.output_complete);
        value["snapshot_removed"] = json!(report.snapshot_removed);
        value["untrusted_stdout"] = json!(report.untrusted_stdout);
        value["untrusted_stderr"] = json!(report.untrusted_stderr);
    }
    value
}

pub struct TestSession {
    pub tools: WorkerTools,
    policy: TestPolicy,
    runtime: DevTestRuntime,
    scratch_parent: PathBuf,
    receipts: Vec<TestReceipt>,
    owner_output_published: bool,
}

impl TestSession {
    pub fn create(
        parent: &Path,
        bootstrap: TestBootstrap,
        trampoline: &Path,
    ) -> Result<Self, ToolError> {
        if bootstrap.schema_version != TEST_SESSION_SCHEMA
            || bootstrap.test_policy.suites.is_empty()
            || bootstrap.test_policy.suites.len() > MAX_SUITES
            || bootstrap.test_policy.max_runs == 0
            || bootstrap.test_policy.max_runs > MAX_TEST_RUNS
            || !bootstrap.test_policy.backend.is_absolute()
            || !bootstrap.test_policy.node.is_absolute()
            || !trampoline.is_absolute()
        {
            return Err(ToolError::InvalidBootstrap);
        }
        let mut suites = BTreeSet::new();
        for suite in &bootstrap.test_policy.suites {
            if !suites.insert(&suite.suite_id) {
                return Err(ToolError::InvalidBootstrap);
            }
            // Check the complete fixed manifest without requiring its execution
            // grant to be enabled. Removing a grant must remove the tool.
            let mut probe = make_request(&bootstrap.worker, suite, Uuid::new_v4());
            probe.grants.read_files = true;
            probe.grants.run_dev_tests = true;
            probe.validate()?;
        }
        let runtime = DevTestRuntime {
            backend: bootstrap.test_policy.backend.clone(),
            node: bootstrap.test_policy.node.clone(),
            trampoline: trampoline.into(),
        };
        Ok(Self {
            tools: WorkerTools::create_test_session(parent, bootstrap.worker)?,
            policy: bootstrap.test_policy,
            runtime,
            scratch_parent: parent.into(),
            receipts: vec![],
            owner_output_published: false,
        })
    }

    pub fn suites(&self) -> Vec<String> {
        self.policy
            .suites
            .iter()
            .map(|suite| suite.suite_id.clone())
            .collect()
    }
    pub fn recovery_hint(&self) -> RecoveryRecord {
        self.tools.owner_recovery_hint()
    }
    pub fn receipts(&self) -> &[TestReceipt] {
        &self.receipts
    }

    /// Called only by terminal owner finalization after the complete candidate
    /// has been durably published. Recovery/test receipts never set this flag.
    pub(crate) fn allow_cleanup_after_publication(&mut self) {
        self.owner_output_published = true;
    }

    fn admit(&mut self, call: &TestCall) -> Result<(DevTestRequest, usize), ToolError> {
        if self.receipts.len() >= self.policy.max_runs {
            return Err(ToolError::ResourceLimit);
        }
        let suite = self
            .policy
            .suites
            .iter()
            .find(|suite| suite.suite_id == call.suite_id)
            .ok_or(ToolError::Denied)?;
        let snapshot = self.tools.test_snapshot(call.expected_revision)?;
        let request = make_request(&snapshot, suite, Uuid::new_v4());
        request.validate()?;
        let identity = dev_tests::input_identity(&request.files)?;
        if self
            .receipts
            .iter()
            .any(|receipt| receipt.suite_id == call.suite_id && receipt.input_snapshot == identity)
        {
            return Err(ToolError::ExecutionAlreadyAdmitted);
        }
        let receipt = TestReceipt {
            operation_id: request.operation_id,
            suite_id: call.suite_id.clone(),
            workspace_revision: call.expected_revision,
            input_snapshot: identity,
            status: "admitted".into(),
            error_code: None,
            report: None,
        };
        self.tools
            .journal_test(receipt.operation_id, "admitted", &receipt)?;
        self.receipts.push(receipt);
        Ok((request, self.receipts.len() - 1))
    }

    /// No detached execution task: the caller polls this owned future while its
    /// protocol loop continues receiving cancellation. Blocking phases are joined.
    /// Dropping any phase retains the editable workspace for owner reconciliation.
    pub async fn run(
        self,
        call: TestCall,
        mut cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(Self, Result<Value, ToolError>), &'static str> {
        let guard = RunGuard(Some(self));
        let (mut guard, admission) = tokio::task::spawn_blocking(move || {
            let mut guard = guard;
            let session = guard.0.as_mut().expect("owned session");
            let admission = session.admit(&call).map(|(request, index)| {
                let prepared = dev_tests::prepare_dev_tests(
                    request,
                    &session.runtime,
                    &session.scratch_parent,
                );
                (prepared, index)
            });
            (guard, admission)
        })
        .await
        .map_err(|_| "test_owner_preparation_failed")?;
        let (prepared, index) = match admission {
            Ok(value) => value,
            Err(error) => return Ok((guard.release(), Err(error))),
        };
        let result = match prepared {
            Ok(prepared) => {
                let executed = prepared.execute(&mut cancellation).await;
                // Move the guard into cleanup too, so a panic/drop cannot erase
                // the only editable workspace while that phase is unobserved.
                let (returned, report) =
                    tokio::task::spawn_blocking(move || (guard, executed.finish()))
                        .await
                        .map_err(|_| "test_owner_cleanup_failed")?;
                guard = returned;
                Ok(report)
            }
            Err(error) => Err(error),
        };
        let (guard, output) = tokio::task::spawn_blocking(move || {
            let mut guard = guard;
            let session = guard.0.as_mut().expect("owned session");
            let receipt = &mut session.receipts[index];
            match result {
                Ok(report) => {
                    receipt.status = "finished".into();
                    if !report.snapshot_removed
                        || (report.process_started
                            && (!report.process_exited || !report.process_tree_empty))
                    {
                        receipt.status = "outcome_unknown".into();
                        session.tools.retain_for_recovery();
                    }
                    receipt.report = Some(report);
                }
                Err(error) => {
                    receipt.status = "refused".into();
                    receipt.error_code = Some(error.code().into());
                }
            }
            let persisted = session
                .tools
                .journal_test(receipt.operation_id, "completed", receipt);
            let output = persisted.map(|()| public_result(receipt));
            (guard, output)
        })
        .await
        .map_err(|_| "test_owner_receipt_failed")?;
        Ok((guard.release(), output))
    }
}

fn make_request(snapshot: &Bootstrap, suite: &TestSuite, operation_id: Uuid) -> DevTestRequest {
    let mut read_scopes = snapshot.read_scopes.clone();
    for pattern in &snapshot.owned_paths {
        if !read_scopes.contains(pattern) {
            read_scopes.push(pattern.clone());
        }
    }
    DevTestRequest {
        schema_version: dev_tests::DEV_TEST_SCHEMA,
        attempt_id: snapshot.attempt_id,
        operation_id,
        suite_id: suite.suite_id.clone(),
        grants: snapshot.grants,
        read_scopes,
        forbidden_paths: snapshot.forbidden_paths.clone(),
        files: snapshot.files.clone(),
        test_entries: suite.test_entries.clone(),
        timeout_ms: suite.timeout_ms,
        output_limit_bytes: suite.output_limit_bytes,
    }
}

struct RunGuard(Option<TestSession>);
impl RunGuard {
    fn release(mut self) -> TestSession {
        self.0.take().expect("owned session")
    }
}
impl Drop for RunGuard {
    fn drop(&mut self) {
        if let Some(session) = &mut self.0 {
            session.tools.retain_for_recovery();
        }
    }
}

impl Drop for TestSession {
    fn drop(&mut self) {
        if !self.owner_output_published {
            // Disarm TempDir cleanup before its field destructor can run.
            // This also covers an async future dropped before its first poll.
            self.tools.retain_for_recovery();
        }
    }
}
