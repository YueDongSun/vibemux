//! Explicit synthetic acceptance; these tests never launch an installed CLI.
use super::*;
use std::{path::Path, sync::atomic::AtomicUsize};
use vibemux_platform::TRAMPOLINE_EXIT_NOT_RELEASED;

const OBSERVATION_DEADLINE: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

fn plan(root: &Path, mode: &str, ready: Option<&Path>) -> (LaunchPlan, Vec<(String, OsString)>) {
    let image = std::env::current_exe().expect("test image");
    let profile = image.parent().expect("deps").parent().expect("profile");
    let executable = profile.join("vibemux_native_fixture.exe");
    let trampoline = profile.join("vibemux_launch_trampoline.exe");
    assert!(
        executable.is_file() && trampoline.is_file(),
        "build fixed native fixture and trampoline first"
    );
    let mut arguments = vec!["--fixture_mode".into(), mode.into()];
    if let Some(ready) = ready {
        arguments.extend([
            "--fixture_ready_path".into(),
            ready.to_str().expect("fixture path").into(),
        ]);
    }
    let names = SYSTEM_ENVIRONMENT_NAMES
        .iter()
        .map(|name| (*name).into())
        .collect::<Vec<_>>();
    let environment = resolve_environment(&names).expect("system allowlist");
    (
        LaunchPlan {
            trampoline,
            executable: executable.clone(),
            working_directory: root.to_owned(),
            spec: LaunchSpec {
                executable,
                arguments,
                environment_names: names,
            },
        },
        environment,
    )
}

fn process_stamp(pid: u32) -> Option<(u64, PathBuf)> {
    let mut system = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
    );
    let process = system.process(pid)?;
    if process.status() == sysinfo::ProcessStatus::Zombie {
        return None;
    }
    let executable = process.exe()?.canonicalize().ok()?;
    Some((process.start_time(), executable))
}

async fn assert_original_process_ended(pid: u32, stamp: &(u64, PathBuf)) {
    timeout(OBSERVATION_DEADLINE, async {
        while process_stamp(pid).as_ref() == Some(stamp) {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await
    .expect("exact owned process ended");
}

async fn descendant_pid(ready: &Path) -> u32 {
    timeout(OBSERVATION_DEADLINE, async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(ready)
                .unwrap_or_default()
                .trim()
                .parse::<u32>()
            {
                break pid;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await
    .expect("synthetic descendant ready")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit held-process acceptance: build native fixture and trampoline"]
async fn authorization_denial_joins_an_unreleased_trampoline_without_starting_the_program() {
    let root = tempfile::tempdir().expect("fixture root");
    let ready = root.path().join("never_started");
    let (plan, environment) = plan(root.path(), "early_descendant", Some(&ready));
    let held = NativeProcess::spawn_held(&plan, environment)
        .await
        .expect("contained held owner");
    let failure = held
        .release_after(|pid| async move {
            assert_ne!(pid, 0);
            Err::<(), _>("PRIVATE_ADMISSION_ERROR")
        })
        .await
        .err()
        .expect("authorization refused");
    assert!(!failure.release_attempted());
    assert!(!format!("{failure:?}").contains("PRIVATE"));
    let (cause, receipt) = failure.finish().await;
    assert!(matches!(
        cause,
        ReleaseFailureCause::Authorization("PRIVATE_ADMISSION_ERROR")
    ));
    assert_eq!(receipt.exit.exit_code, Some(TRAMPOLINE_EXIT_NOT_RELEASED));
    assert!(receipt.root_reaped && receipt.tree_empty && receipt.cleanup_verified);
    assert!(
        !ready.exists(),
        "the held program and descendant never started"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit held-process acceptance: build native fixture and trampoline"]
async fn explicit_release_is_one_shot_and_legacy_spawn_preserves_the_same_pipe_bytes() {
    let root = tempfile::tempdir().expect("fixture root");
    let payload = b"fixed bytes\n\x06 literal $(never_run); \"quoted\"\r\n";
    for legacy in [false, true] {
        let (plan, environment) = plan(root.path(), "echo_stdin", None);
        let approvals = Arc::new(AtomicUsize::new(0));
        let mut process = if legacy {
            NativeProcess::spawn(&plan, environment)
                .await
                .expect("legacy immediate release")
        } else {
            let approvals = approvals.clone();
            NativeProcess::spawn_held(&plan, environment)
                .await
                .expect("held")
                .release_after(move |_| async move {
                    approvals.fetch_add(1, Ordering::AcqRel);
                    Ok::<(), DispatchError>(())
                })
                .await
                .expect("explicit release")
        };
        process
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(payload)
            .await
            .expect("fixed data");
        process.close_stdin();
        let mut output = Vec::new();
        timeout(
            OBSERVATION_DEADLINE,
            (&mut process.stdout).take(1024).read_to_end(&mut output),
        )
        .await
        .expect("bounded echo")
        .expect("pipe");
        let receipt = process
            .finish_session(false, Instant::now() + OBSERVATION_DEADLINE)
            .await;
        assert_eq!(output, payload, "an extra go byte would change the echo");
        assert_eq!(receipt.exit.exit_code, Some(0));
        assert!(receipt.cleanup_verified);
        assert_eq!(approvals.load(Ordering::Acquire), usize::from(!legacy));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit held-process acceptance: build native fixture and trampoline"]
async fn released_descendants_are_contained_and_all_owned_pipes_end() {
    let root = tempfile::tempdir().expect("fixture root");
    let ready = root.path().join("descendant_ready");
    let (plan, environment) = plan(root.path(), "early_descendant", Some(&ready));
    let ready_check = ready.clone();
    let mut process = NativeProcess::spawn_held(&plan, environment)
        .await
        .expect("held")
        .release_after(move |_| async move {
            assert!(!ready_check.exists());
            Ok::<(), DispatchError>(())
        })
        .await
        .expect("release after authorization");
    let pid = descendant_pid(&ready).await;
    let stamp = process_stamp(pid).expect("owned descendant identity");
    assert!(process.tree.active_process_count().expect("owned job") >= 2);
    let receipt = process.finish_session(true, Instant::now()).await;
    let mut output = Vec::new();
    timeout(
        OBSERVATION_DEADLINE,
        (&mut process.stdout).take(1024).read_to_end(&mut output),
    )
    .await
    .expect("stdout EOF")
    .expect("pipe");
    assert!(output.is_empty());
    assert!(receipt.root_reaped && receipt.tree_empty && receipt.cleanup_verified);
    assert_original_process_ended(pid, &stamp).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit held-process acceptance: build native fixture and trampoline"]
async fn dropped_held_owner_starts_no_program_and_supplies_no_joined_receipt() {
    let root = tempfile::tempdir().expect("fixture root");
    let ready = root.path().join("never_started");
    let (plan, environment) = plan(root.path(), "early_descendant", Some(&ready));
    let held = NativeProcess::spawn_held(&plan, environment)
        .await
        .expect("held");
    let pid = held.process_id;
    let stamp = process_stamp(pid).expect("exact held root identity");
    drop(held);
    assert_original_process_ended(pid, &stamp).await;
    assert!(!ready.exists());
    // Drop containment is intentionally not promoted to a durable cleanup receipt.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit held-process acceptance: build native fixture and trampoline"]
async fn transport_failure_stays_a_release_attempt_until_its_owned_cleanup_finishes() {
    let root = tempfile::tempdir().expect("fixture root");
    let ready = root.path().join("never_started");
    let (plan, environment) = plan(root.path(), "early_descendant", Some(&ready));
    let mut held = NativeProcess::spawn_held(&plan, environment)
        .await
        .expect("held");
    // Inject a lost release pipe without exposing such an operation in the API.
    held.process.close_stdin();
    let failure = held
        .release_after(|_| async { Ok::<(), DispatchError>(()) })
        .await
        .err()
        .expect("transport failure");
    assert!(
        failure.release_attempted(),
        "pipe failure cannot establish no start"
    );
    let (cause, receipt) = failure.finish().await;
    assert!(matches!(
        cause,
        ReleaseFailureCause::Transport(DispatchError::PipeFailed)
    ));
    assert!(receipt.root_reaped && receipt.tree_empty && receipt.cleanup_verified);
    assert!(!ready.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit held-process acceptance: build native fixture and trampoline"]
async fn aborting_pending_authorization_ends_its_owner_without_creating_a_receipt() {
    let root = tempfile::tempdir().expect("fixture root");
    let ready = root.path().join("never_started");
    let (plan, environment) = plan(root.path(), "early_descendant", Some(&ready));
    let held = NativeProcess::spawn_held(&plan, environment)
        .await
        .expect("held");
    let pid = held.process_id;
    let stamp = process_stamp(pid).expect("exact held root");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let request = tokio::spawn(held.release_after(move |pid| async move {
        let _ = entered_tx.send(pid);
        let _ = release_rx.await;
        Ok::<(), DispatchError>(())
    }));
    assert_eq!(
        timeout(OBSERVATION_DEADLINE, entered_rx)
            .await
            .expect("callback entered")
            .expect("PID"),
        pid
    );
    request.abort();
    match request.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("authorization remained pending"),
    }
    drop(release_tx);
    assert_original_process_ended(pid, &stamp).await;
    assert!(!ready.exists());
    // Aborting an authorization future has no joined/durable cleanup receipt.
}
