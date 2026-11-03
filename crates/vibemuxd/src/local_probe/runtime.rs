//! Windows contained probe capture.
use super::{LocalProbeCompletion, ProcessReceipt, image_pin::Bundle};
use crate::harness_dispatch::native_process::{
    LaunchPlan, NativeProcess, ReleaseFailureCause, SessionProcessExit,
};
use std::{collections::BTreeSet, ffi::OsString, future::Future, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::watch,
    time::{Instant, sleep, timeout},
};
use vibemux_probe::{AgentKind, PROBE_ENVIRONMENT_ALLOWLIST, ProbeReport, ProbeState};

const RUN_DEADLINE: Duration = Duration::from_secs(20);
const JOIN_DEADLINE: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_STDOUT_BYTES: usize = 256 * 1024;
const MAX_STDERR_BYTES: u64 = 64 * 1024;
const CHUNK_BYTES: usize = 8 * 1024;
const CANCELLED: &str = "local_probe_cancelled";
const TIMED_OUT: &str = "local_probe_timed_out";
const OUTPUT_LIMIT: &str = "local_probe_output_limit";
const READ_FAILED: &str = "local_probe_read_failed";
const INVALID_REPORT: &str = "local_probe_report_invalid";

pub(super) struct RunLimits {
    pub deadline: Duration,
    pub stdout_bytes: usize,
    pub stderr_bytes: u64,
}

pub(super) async fn run(cancel: watch::Receiver<bool>) -> LocalProbeCompletion {
    if stopped(&cancel) {
        return LocalProbeCompletion::rejected(CANCELLED);
    }
    let bundle = match tokio::task::spawn_blocking(Bundle::acquire).await {
        Ok(Ok(bundle)) => Arc::new(bundle),
        _ => return LocalProbeCompletion::rejected("local_probe_artifact_unavailable"),
    };
    let verified_bundle = Arc::clone(&bundle);
    execute_plan(
        bundle.plan(),
        cancel,
        RunLimits {
            deadline: RUN_DEADLINE,
            stdout_bytes: MAX_STDOUT_BYTES,
            stderr_bytes: MAX_STDERR_BYTES,
        },
        move || async move {
            tokio::task::spawn_blocking(move || verified_bundle.verify())
                .await
                .map_err(|_| "local_probe_artifact_unavailable")?
        },
    )
    .await
}

pub(super) async fn execute_plan<F, A>(
    mut plan: LaunchPlan,
    cancel: watch::Receiver<bool>,
    limits: RunLimits,
    verify: F,
) -> LocalProbeCompletion
where
    F: FnOnce() -> A,
    A: Future<Output = Result<(), &'static str>>,
{
    if stopped(&cancel) {
        return LocalProbeCompletion::rejected(CANCELLED);
    }
    let environment = environment();
    plan.spec.environment_names = environment.iter().map(|(name, _)| name.clone()).collect();
    let held = match NativeProcess::spawn_held(&plan, environment).await {
        Ok(held) => held,
        // A partially failed spawn has no joined receipt. Do not manufacture one.
        Err(_) => {
            return LocalProbeCompletion {
                report: None,
                error_code: Some("local_probe_spawn_unconfirmed"),
                receipt: None,
                not_spawned: false,
            };
        }
    };
    let released = held
        .release_after(|_| async {
            if stopped(&cancel) {
                return Err(CANCELLED);
            }
            verify().await?;
            if stopped(&cancel) {
                return Err(CANCELLED);
            }
            Ok(())
        })
        .await;
    let mut process = match released {
        Ok(process) => process,
        Err(failure) => {
            let (cause, joined) = failure.finish().await;
            let code = match cause {
                ReleaseFailureCause::Authorization(code) => code,
                ReleaseFailureCause::Transport(_) => "local_probe_release_failed",
            };
            return LocalProbeCompletion {
                report: None,
                error_code: Some(code),
                receipt: Some(receipt(joined, joined.stdout_drained.unwrap_or(false))),
                not_spawned: false,
            };
        }
    };
    process.close_stdin();
    let deadline = Instant::now() + limits.deadline;
    let mut bytes = Vec::new();
    let mut stdout_drained = false;
    let mut buffer = [0; CHUNK_BYTES];
    let mut problem = loop {
        if stopped(&cancel) {
            break Some(CANCELLED);
        }
        if Instant::now() >= deadline {
            break Some(TIMED_OUT);
        }
        if process.stderr_diagnostic().bytes > limits.stderr_bytes {
            break Some(OUTPUT_LIMIT);
        }
        if stdout_drained {
            sleep(POLL_INTERVAL).await;
        } else {
            match timeout(POLL_INTERVAL, process.stdout.read(&mut buffer)).await {
                Ok(Ok(0)) => stdout_drained = true,
                Ok(Ok(count)) => {
                    if !append(&mut bytes, &buffer[..count], limits.stdout_bytes) {
                        break Some(OUTPUT_LIMIT);
                    }
                }
                Ok(Err(_)) => break Some(READ_FAILED),
                Err(_) => {}
            }
        }
        // Do not wait for EOF from a descendant after the root already ended.
        match process.root_exited() {
            Ok(true) => break None,
            Ok(false) => {}
            Err(_) => break Some(READ_FAILED),
        }
    };
    let joined = process
        .finish_session(problem.is_some(), Instant::now() + JOIN_DEADLINE)
        .await;
    if !stdout_drained {
        let capture = problem.is_none();
        let tail = drain_tail(
            &mut process.stdout,
            &mut bytes,
            limits.stdout_bytes,
            capture,
        )
        .await;
        stdout_drained = tail.0;
        if problem.is_none() {
            problem = tail.1;
        }
    }
    if problem.is_none() && joined.stderr_bytes > limits.stderr_bytes {
        problem = Some(OUTPUT_LIMIT);
    }
    let receipt = receipt(joined, stdout_drained);
    let mut completion = LocalProbeCompletion {
        report: None,
        error_code: problem,
        receipt: Some(receipt),
        not_spawned: false,
    };
    if !completion.is_quiescent() {
        completion.error_code = Some("local_probe_cleanup_unconfirmed");
    } else if completion.error_code.is_none() {
        if receipt.exit.exit_code != Some(0) {
            completion.error_code = Some("local_probe_exit_failed");
        } else {
            match parse_report(&bytes) {
                Ok(report) => completion.report = Some(report),
                Err(code) => completion.error_code = Some(code),
            }
        }
    }
    completion
}

fn stopped(cancel: &watch::Receiver<bool>) -> bool {
    *cancel.borrow() || cancel.has_changed().is_err()
}
fn environment() -> Vec<(String, OsString)> {
    PROBE_ENVIRONMENT_ALLOWLIST
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| ((*name).into(), value)))
        .collect()
}
fn append(bytes: &mut Vec<u8>, chunk: &[u8], maximum: usize) -> bool {
    if bytes
        .len()
        .checked_add(chunk.len())
        .is_none_or(|length| length > maximum)
    {
        return false;
    }
    bytes.extend_from_slice(chunk);
    true
}
fn receipt(joined: SessionProcessExit, stdout_drained: bool) -> ProcessReceipt {
    ProcessReceipt {
        exit: joined.exit,
        termination_succeeded: joined.termination_succeeded,
        root_reaped: joined.root_reaped,
        tree_empty: joined.tree_empty,
        stdout_drained,
        stderr_drained: joined.stderr_drained,
    }
}
async fn drain_tail(
    stdout: &mut (impl AsyncRead + Unpin),
    bytes: &mut Vec<u8>,
    maximum: usize,
    capture: bool,
) -> (bool, Option<&'static str>) {
    let result = timeout(JOIN_DEADLINE, async {
        let mut buffer = [0; CHUNK_BYTES];
        let mut problem = None;
        loop {
            match stdout.read(&mut buffer).await {
                Ok(0) => return (true, problem),
                Ok(count) => {
                    if capture && problem.is_none() && !append(bytes, &buffer[..count], maximum) {
                        problem = Some(OUTPUT_LIMIT);
                    }
                }
                Err(_) => return (false, Some(READ_FAILED)),
            }
        }
    })
    .await;
    result.unwrap_or((false, Some(READ_FAILED)))
}
fn parse_report(bytes: &[u8]) -> Result<serde_json::Value, &'static str> {
    let report: ProbeReport = serde_json::from_slice(bytes).map_err(|_| INVALID_REPORT)?;
    let known = AgentKind::all().into_iter().collect::<BTreeSet<_>>();
    let present = report
        .agents
        .iter()
        .map(|agent| agent.agent)
        .collect::<BTreeSet<_>>();
    if present != known
        || report.agents.len() != known.len()
        || report.agents.iter().any(|agent| {
            agent.authentication_state != ProbeState::NotRun
                || agent.inference_state != ProbeState::NotRun
        })
    {
        return Err(INVALID_REPORT);
    }
    vibemux_probe::snapshot::project_report(report).map_err(|_| INVALID_REPORT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::ReadBuf;

    struct FailedRead;
    impl AsyncRead for FailedRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("PRIVATE_PIPE_FAILURE")))
        }
    }

    #[tokio::test]
    async fn a_pipe_read_error_cannot_become_eof_or_publish_private_error_text() {
        let mut bytes = Vec::new();
        let result = drain_tail(&mut FailedRead, &mut bytes, 1024, true).await;
        assert_eq!(result, (false, Some(READ_FAILED)));
        assert!(bytes.is_empty());
        assert!(!format!("{result:?}").contains("PRIVATE"));
    }
}
