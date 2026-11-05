use super::{
    LocalProbeCompletion,
    runtime::{RunLimits, execute_plan},
};
use crate::harness_dispatch::native_process::LaunchPlan;
use std::{path::Path, time::Duration};
use tokio::sync::watch;
use vibemux_harness::dispatch::launch_spec::LaunchSpec;

fn plan(root: &Path, mode: &str, ready: Option<&Path>) -> LaunchPlan {
    let image = std::env::current_exe().expect("test image");
    let profile = image.parent().expect("deps").parent().expect("profile");
    let executable = profile.join("vibemux_native_fixture.exe");
    let trampoline = profile.join("vibemux_launch_trampoline.exe");
    assert!(
        executable.is_file() && trampoline.is_file(),
        "build fixed fixture; preserve the existing qualified trampoline"
    );
    let mut arguments = vec!["--fixture_mode".into(), mode.into()];
    if let Some(ready) = ready {
        arguments.extend([
            "--fixture_ready_path".into(),
            ready.to_str().expect("fixture path").into(),
        ]);
    }
    LaunchPlan {
        trampoline,
        executable: executable.clone(),
        working_directory: root.into(),
        spec: LaunchSpec {
            executable,
            arguments,
            environment_names: Vec::new(),
        },
    }
}

fn limits() -> RunLimits {
    RunLimits {
        deadline: Duration::from_secs(10),
        stdout_bytes: 256 * 1024,
        stderr_bytes: 64 * 1024,
    }
}
fn joined(result: &LocalProbeCompletion) {
    assert!(result.is_quiescent());
    let receipt = result.receipt.expect("actual receipt");
    assert!(receipt.termination_succeeded && receipt.root_reaped && receipt.tree_empty);
    assert!(receipt.stdout_drained && receipt.stderr_drained);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit fixed native diagnostic fixture; no installed launcher, model or GUI"]
async fn valid_private_report_is_published_only_after_joined_cleanup() {
    let root = tempfile::tempdir().expect("owned root");
    let (_sender, cancel) = watch::channel(false);
    let result = execute_plan(
        plan(root.path(), "probe_report", None),
        cancel,
        RunLimits {
            deadline: Duration::from_secs(10),
            stdout_bytes: 256 * 1024,
            stderr_bytes: 64 * 1024,
        },
        || async { Ok(()) },
    )
    .await;
    assert_eq!(result.error_code(), None);
    joined(&result);
    let report = result.report().expect("safe metadata");
    assert!(!report.to_string().contains("PRIVATE_"));
    assert_eq!(report["source"], "local_probe");
    let receipt = result.receipt.expect("actual joined receipt");
    assert!(receipt.termination_succeeded && receipt.root_reaped && receipt.tree_empty);
    assert!(receipt.stdout_drained && receipt.stderr_drained);
    assert!(!root.path().join(".vibemux/probe_cache.json").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit fixed native diagnostic fixture; no installed launcher, model or GUI"]
async fn malformed_nonzero_empty_or_inference_claiming_reports_never_publish_data() {
    let root = tempfile::tempdir().expect("owned root");
    for (mode, expected) in [
        ("probe_invalid_json", "local_probe_report_invalid"),
        ("probe_claimed_inference", "local_probe_report_invalid"),
        ("exit_code", "local_probe_exit_failed"),
        ("echo_stdin", "local_probe_report_invalid"),
    ] {
        let (_sender, cancel) = watch::channel(false);
        let result = execute_plan(plan(root.path(), mode, None), cancel, limits(), || async {
            Ok(())
        })
        .await;
        joined(&result);
        assert_eq!(result.error_code(), Some(expected));
        assert!(result.report().is_none());
        assert!(!format!("{result:?}").contains("PRIVATE"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit fixed native diagnostic fixture; no installed launcher, model or GUI"]
async fn stdout_and_stderr_overflow_stop_the_owned_tree_without_exposing_bytes() {
    let root = tempfile::tempdir().expect("owned root");
    for mode in ["probe_overflow", "probe_stderr_overflow"] {
        let (_sender, cancel) = watch::channel(false);
        let result = execute_plan(plan(root.path(), mode, None), cancel, limits(), || async {
            Ok(())
        })
        .await;
        joined(&result);
        assert_eq!(result.error_code(), Some("local_probe_output_limit"));
        assert!(result.report().is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit fixed native diagnostic fixture; no installed launcher, model or GUI"]
async fn cancellation_after_start_and_deadline_join_the_actual_process() {
    let root = tempfile::tempdir().expect("owned root");
    let ready = root.path().join("cancel_ready");
    let (sender, cancel) = watch::channel(false);
    let launch = plan(root.path(), "silent_input", Some(&ready));
    let work =
        tokio::spawn(
            async move { execute_plan(launch, cancel, limits(), || async { Ok(()) }).await },
        );
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture actually started");
    sender.send(true).expect("cancel accepted");
    let result = work.await.expect("runner joined");
    joined(&result);
    assert_eq!(result.error_code(), Some("local_probe_cancelled"));

    let (_sender, cancel) = watch::channel(false);
    let mut deadline = limits();
    deadline.deadline = Duration::from_millis(500);
    let result = execute_plan(
        plan(root.path(), "silent_input", None),
        cancel,
        deadline,
        || async { Ok(()) },
    )
    .await;
    joined(&result);
    assert_eq!(result.error_code(), Some("local_probe_timed_out"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit fixed native diagnostic fixture; no installed launcher, model or GUI"]
async fn root_exit_with_a_descendant_holding_stdout_joins_before_global_timeout() {
    let root = tempfile::tempdir().expect("owned root");
    let ready = root.path().join("descendant_ready");
    let (_sender, cancel) = watch::channel(false);
    let result = execute_plan(
        plan(root.path(), "early_descendant", Some(&ready)),
        cancel,
        limits(),
        || async { Ok(()) },
    )
    .await;
    joined(&result);
    assert!(ready.is_file(), "the descendant actually existed");
    assert_eq!(result.error_code(), Some("local_probe_report_invalid"));
    assert_eq!(result.receipt.expect("receipt").exit.exit_code, Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit fixed native diagnostic fixture; no installed launcher, model or GUI"]
async fn failed_final_artifact_check_does_not_release_the_fixture() {
    let root = tempfile::tempdir().expect("owned root");
    let ready = root.path().join("must_not_start");
    let (_sender, cancel) = watch::channel(false);
    let result = execute_plan(
        plan(root.path(), "early_descendant", Some(&ready)),
        cancel,
        limits(),
        || async { Err("local_probe_artifact_changed") },
    )
    .await;
    joined(&result);
    assert_eq!(result.error_code(), Some("local_probe_artifact_changed"));
    assert!(!ready.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit fixed native diagnostic fixture; no installed launcher, model or GUI"]
async fn cancellation_during_final_verification_prevents_the_go_byte() {
    let root = tempfile::tempdir().expect("owned root");
    let ready = root.path().join("must_not_run");
    let launch = plan(root.path(), "early_descendant", Some(&ready));
    let (sender, cancel) = watch::channel(false);
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let work = tokio::spawn(async move {
        execute_plan(launch, cancel, limits(), move || async move {
            tokio::task::spawn_blocking(move || {
                let _ = entered.send(());
                held.recv().map_err(|_| "local_probe_test_gate_failed")?;
                Ok(())
            })
            .await
            .map_err(|_| "local_probe_test_gate_failed")?
        })
        .await
    });
    observed.await.expect("final verification actually entered");
    let cancelled = sender.send(true);
    release
        .send(())
        .expect("release verification before assertions");
    let result = work.await.expect("owned runner joined");
    cancelled.expect("cancellation signal accepted");
    joined(&result);
    assert_eq!(result.error_code(), Some("local_probe_cancelled"));
    assert_eq!(
        result.receipt.expect("receipt").exit.exit_code,
        Some(vibemux_platform::TRAMPOLINE_EXIT_NOT_RELEASED)
    );
    assert!(!ready.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit fixed native diagnostic fixture; no installed launcher, model or GUI"]
async fn public_runner_resolves_and_executes_its_fixed_sibling_bundle() {
    use crate::harness_dispatch::native_process::NativeProcess;
    use tokio::{
        io::AsyncReadExt,
        time::{Instant, timeout},
    };
    let root = tempfile::tempdir().expect("owned bundle");
    let source = plan(root.path(), "probe_report", None);
    let host = root.path().join("probe_host.exe");
    for target in [&host, &root.path().join("vibemux_probe.exe")] {
        std::fs::copy(&source.executable, target).expect("copy exact fixed fixture");
        assert!(
            std::fs::read(&source.executable).expect("source bytes")
                == std::fs::read(target).expect("copy bytes"),
            "exact fixture copy"
        );
    }
    let trampoline = root.path().join("vibemux_launch_trampoline.exe");
    std::fs::copy(&source.trampoline, &trampoline).expect("copy qualified trampoline");
    assert!(
        std::fs::read(&source.trampoline).expect("source bytes")
            == std::fs::read(&trampoline).expect("copy bytes"),
        "exact trampoline copy"
    );
    std::fs::write(
        root.path().join("fixture_mode.json"),
        br#"{"mode":"probe_report"}"#,
    )
    .expect("fixed sibling mode");
    let launch = LaunchPlan {
        trampoline,
        executable: host.clone(),
        working_directory: root.path().into(),
        spec: LaunchSpec {
            executable: host,
            arguments: vec!["--fixture_mode".into(), "probe_host".into()],
            environment_names: Vec::new(),
        },
    };
    let environment = vibemux_probe::PROBE_ENVIRONMENT_ALLOWLIST
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| ((*name).into(), value)))
        .collect();
    let mut owner = NativeProcess::spawn(&launch, environment)
        .await
        .expect("contained facade host");
    owner.close_stdin();
    let mut bytes = Vec::new();
    let read = timeout(
        Duration::from_secs(30),
        (&mut owner.stdout)
            .take(32 * 1024 + 1)
            .read_to_end(&mut bytes),
    )
    .await;
    let clean_read = matches!(&read, Ok(Ok(_))) && bytes.len() <= 32 * 1024;
    let joined = owner
        .finish_session(!clean_read, Instant::now() + Duration::from_secs(2))
        .await;
    assert!(
        joined.cleanup_verified && joined.root_reaped && joined.tree_empty && joined.stderr_drained
    );
    assert!(clean_read, "bounded EOF from facade host");
    assert_eq!(joined.exit.exit_code, Some(0));
    let view: serde_json::Value = serde_json::from_slice(&bytes).expect("facade result");
    assert_eq!(view["error_code"], serde_json::Value::Null);
    assert_eq!(view["quiescent"], true);
    assert_eq!(view["report"]["source"], "local_probe");
    assert_eq!(
        view["report"]["agents"]
            .as_array()
            .expect("all agents")
            .len(),
        vibemux_probe::AgentKind::all().len()
    );
    assert!(!view.to_string().contains("PRIVATE_"));
    assert!(!root.path().join(".vibemux/probe_cache.json").exists());
}
