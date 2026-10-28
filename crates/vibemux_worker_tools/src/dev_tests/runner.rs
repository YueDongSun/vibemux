use super::{
    DEV_TEST_SCHEMA, DevTestReport, DevTestRequest, DevTestRuntime, ExecutedDevTest,
    ExecutionOutcome, PreparedDevTest, snapshot::Snapshot,
};
use crate::contract::ToolError;
use std::{ffi::OsString, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::watch,
    time::{Instant, sleep, sleep_until, timeout_at},
};
use vibemux_platform::{
    LAUNCH_GO_BYTE, ProcessTree,
    sandbox_mxc::{MxcOfflinePolicy, offline_command},
};

const FINALIZE_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const READ_CHUNK: usize = 4096;
const PROCESS_FLAGS: u32 = 0x0800_0000 | 0x0000_0200;
const PROFILE: &str = "node_test_offline_v1";
const NODE_ARGUMENTS: [&str; 6] = [
    "--preserve-symlinks",
    "--preserve-symlinks-main",
    "--test",
    "--test-reporter=tap",
    "--test-concurrency=1",
    "--",
];

pub(super) fn prepare(
    request: DevTestRequest,
    runtime: &DevTestRuntime,
    parent: &Path,
) -> Result<PreparedDevTest, ToolError> {
    let canonical_file = |path: &Path| -> Result<std::path::PathBuf, ToolError> {
        if !path.is_absolute() || !path.is_file() {
            return Err(ToolError::InvalidPath);
        }
        path.canonicalize().map_err(|_| ToolError::Io)
    };
    let backend = canonical_file(&runtime.backend)?;
    let node = canonical_file(&runtime.node)?;
    let trampoline = canonical_file(&runtime.trampoline)?;
    if node
        .file_name()
        .is_none_or(|name| !name.eq_ignore_ascii_case("node.exe"))
    {
        return Err(ToolError::InvalidPath);
    }
    let snapshot = Snapshot::create(parent, &request)?;
    let mut arguments: Vec<OsString> = NODE_ARGUMENTS.iter().map(OsString::from).collect();
    arguments.extend(
        request
            .test_entries
            .iter()
            .map(|path| OsString::from(format!("./{path}"))),
    );
    let command = offline_command(
        &backend,
        &node,
        &arguments,
        &MxcOfflinePolicy {
            workspace: snapshot.workspace.clone(),
            scratch: snapshot.scratch.clone(),
            read_only_paths: vec![],
            denied_paths: vec![],
            profile_id: format!("rd_mxc_{}", uuid::Uuid::new_v4().simple()),
        },
    );
    match command {
        Ok(command) => Ok(PreparedDevTest {
            snapshot,
            request,
            command,
            trampoline,
        }),
        Err(_) => {
            let _ = snapshot.remove();
            Err(ToolError::ExecutionUnsupported)
        }
    }
}

fn cancelled(cancellation: &watch::Receiver<bool>) -> bool {
    *cancellation.borrow() || cancellation.has_changed().is_err()
}

struct Capture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_eof: bool,
    stderr_eof: bool,
    failed: bool,
    overflow: bool,
    limit: usize,
}

impl Capture {
    fn new(limit: usize) -> Self {
        Self {
            stdout: vec![],
            stderr: vec![],
            stdout_eof: false,
            stderr_eof: false,
            failed: false,
            overflow: false,
            limit,
        }
    }
    fn accept(&mut self, standard_output: bool, result: std::io::Result<usize>, bytes: &[u8]) {
        match result {
            Ok(0) => {
                if standard_output {
                    self.stdout_eof = true;
                } else {
                    self.stderr_eof = true;
                }
            }
            Ok(count) => {
                let remaining = self
                    .limit
                    .saturating_sub(self.stdout.len() + self.stderr.len());
                let retained = count.min(remaining);
                let output = if standard_output {
                    &mut self.stdout
                } else {
                    &mut self.stderr
                };
                output.extend_from_slice(&bytes[..retained]);
                self.overflow |= count > remaining;
            }
            Err(_) => {
                self.failed = true;
                if standard_output {
                    self.stdout_eof = true;
                } else {
                    self.stderr_eof = true;
                }
            }
        }
    }
    fn complete(&self) -> bool {
        self.stdout_eof && self.stderr_eof && !self.failed && !self.overflow
    }
}

pub(super) async fn execute(
    prepared: PreparedDevTest,
    cancellation: &mut watch::Receiver<bool>,
) -> ExecutedDevTest {
    let PreparedDevTest {
        snapshot,
        request,
        command,
        trampoline,
    } = prepared;
    let mut report = DevTestReport {
        schema_version: DEV_TEST_SCHEMA,
        attempt_id: request.attempt_id,
        operation_id: request.operation_id,
        suite_id: request.suite_id.clone(),
        runtime_profile: PROFILE.into(),
        input_snapshot: snapshot.identity,
        execution: ExecutionOutcome::InfrastructureFailed,
        reason_code: None,
        backend_exit_code: None,
        process_started: false,
        process_exited: false,
        process_tree_empty: false,
        output_complete: false,
        untrusted_stdout: String::new(),
        untrusted_stderr: String::new(),
        snapshot_removed: false,
        retained_directory: Some(snapshot.root.clone()),
        assertions_verified: false,
        reusable_qualification: false,
    };
    if cancelled(cancellation) {
        report.execution = ExecutionOutcome::Cancelled;
        report.reason_code = Some("dev_test_cancelled_before_spawn".into());
        return ExecutedDevTest { snapshot, report };
    }
    let mut launch = Command::new(trampoline);
    launch
        .arg("__launch_trampoline")
        .arg(command.get_program())
        .args(command.get_args())
        .env_clear()
        .envs(
            command
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .current_dir(&snapshot.workspace)
        .creation_flags(PROCESS_FLAGS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    ProcessTree::prepare_command(launch.as_std_mut());
    let spawned = match vibemux_platform::process_stdio::spawn_with_async_output(&mut launch).await
    {
        Ok(spawned) => spawned,
        Err(_) => {
            report.reason_code = Some("dev_test_spawn_failed".into());
            return ExecutedDevTest { snapshot, report };
        }
    };
    let mut child = spawned.child;
    let child_output = spawned.output;
    report.process_started = true;
    let Some(mut tree) = child.id().and_then(|pid| ProcessTree::contain(pid).ok()) else {
        let _ = child.start_kill();
        let deadline = Instant::now() + FINALIZE_TIMEOUT;
        report.process_exited = matches!(timeout_at(deadline, child.wait()).await, Ok(Ok(_)));
        // The trusted trampoline was never released; it cannot start descendants.
        report.process_tree_empty = report.process_exited;
        report.reason_code = Some("dev_test_containment_failed".into());
        return ExecutedDevTest { snapshot, report };
    };
    let Some(mut input) = child.stdin.take() else {
        report.reason_code = Some("dev_test_pipe_failed".into());
        stop_tree(
            &mut child,
            &mut tree,
            &mut report,
            Instant::now() + FINALIZE_TIMEOUT,
        )
        .await;
        return ExecutedDevTest { snapshot, report };
    };
    let vibemux_platform::process_stdio::ChildOutput {
        stdout: mut output,
        stderr: mut error,
    } = child_output;
    let execution_deadline = Instant::now() + Duration::from_millis(request.timeout_ms);
    let released = if cancelled(cancellation) {
        report.execution = ExecutionOutcome::Cancelled;
        false
    } else {
        matches!(
            timeout_at(execution_deadline, async {
                input.write_all(&[LAUNCH_GO_BYTE]).await?;
                input.flush().await
            })
            .await,
            Ok(Ok(()))
        )
    };
    drop(input);
    let mut capture = Capture::new(request.output_limit_bytes);
    let mut stdout_buffer = [0; READ_CHUNK];
    let mut stderr_buffer = [0; READ_CHUNK];
    if released {
        loop {
            // Biased select plus an explicit fence keeps a continuously ready
            // output stream from starving deadline/cancellation handling.
            if cancelled(cancellation) {
                report.execution = ExecutionOutcome::Cancelled;
                break;
            }
            if Instant::now() >= execution_deadline {
                report.execution = ExecutionOutcome::TimedOut;
                break;
            }
            tokio::select! {
                biased;
                _ = cancellation.changed() => { if cancelled(cancellation) { report.execution = ExecutionOutcome::Cancelled; break; } }
                _ = sleep_until(execution_deadline) => { report.execution = ExecutionOutcome::TimedOut; break; }
                result = child.wait() => {
                    match result {
                        Ok(status) => { report.backend_exit_code = status.code(); report.process_exited = true; report.execution = ExecutionOutcome::Completed; }
                        Err(_) => { report.reason_code = Some("dev_test_wait_failed".into()); }
                    }
                    break;
                }
                result = output.read(&mut stdout_buffer), if !capture.stdout_eof => { capture.accept(true, result, &stdout_buffer); }
                result = error.read(&mut stderr_buffer), if !capture.stderr_eof => { capture.accept(false, result, &stderr_buffer); }
            }
            if capture.overflow {
                report.execution = ExecutionOutcome::OutputLimit;
                break;
            }
            if capture.failed {
                report.reason_code = Some("dev_test_capture_failed".into());
                break;
            }
        }
    } else if report.execution != ExecutionOutcome::Cancelled {
        report.reason_code = Some("dev_test_release_failed".into());
    }
    // Root exit does not authorize deletion. Kill remaining descendants first,
    // reconcile the Job, then finish both pipes within one finalization budget.
    let finalize_deadline = Instant::now() + FINALIZE_TIMEOUT;
    stop_tree(&mut child, &mut tree, &mut report, finalize_deadline).await;
    while !capture.stdout_eof || !capture.stderr_eof {
        if Instant::now() >= finalize_deadline {
            break;
        }
        tokio::select! {
            biased;
            _ = sleep_until(finalize_deadline) => { break; }
            result = output.read(&mut stdout_buffer), if !capture.stdout_eof => { capture.accept(true, result, &stdout_buffer); }
            result = error.read(&mut stderr_buffer), if !capture.stderr_eof => { capture.accept(false, result, &stderr_buffer); }
        }
    }
    // Output already buffered at root exit still counts toward the same cap.
    if capture.overflow && report.execution == ExecutionOutcome::Completed {
        report.execution = ExecutionOutcome::OutputLimit;
    }
    if capture.failed && report.execution == ExecutionOutcome::Completed {
        report.execution = ExecutionOutcome::InfrastructureFailed;
        report.reason_code = Some("dev_test_capture_failed".into());
    }
    report.output_complete = capture.complete();
    report.untrusted_stdout = String::from_utf8_lossy(&capture.stdout).into_owned();
    report.untrusted_stderr = String::from_utf8_lossy(&capture.stderr).into_owned();
    // Both pipes, child and Job handles close before synchronous cleanup can run.
    drop((output, error, child, tree));
    ExecutedDevTest { snapshot, report }
}

async fn stop_tree(
    child: &mut Child,
    tree: &mut ProcessTree,
    report: &mut DevTestReport,
    deadline: Instant,
) {
    let terminated = tree.terminate().is_ok();
    if !report.process_exited
        && let Ok(Ok(_)) = timeout_at(deadline, child.wait()).await
    {
        report.process_exited = true;
    }
    while Instant::now() < deadline {
        match tree.active_process_count() {
            Ok(0) => {
                report.process_tree_empty = terminated;
                break;
            }
            Ok(_) => {
                sleep(POLL_INTERVAL).await;
            }
            Err(_) => {
                break;
            }
        }
    }
}
