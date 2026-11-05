//! Explicit installed synthetic-process checks, separate from model inference.
use super::{
    contracts::*,
    spawn_runtime,
    tests::{Capture, binding},
};
use crate::harness_dispatch::native_process::LaunchPlan;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;
use vibemux_harness::{
    chat_session::{SessionConfig, SessionLimits, SessionTarget, TurnOutcome, TurnRequest},
    dispatch::launch_spec::{LaunchSpec, SYSTEM_ENVIRONMENT_NAMES, interpreter_script_path},
};

const WAIT: Duration = Duration::from_secs(10);

fn launch(
    project: &Path,
    vendor: &Path,
    trampoline: &Path,
    target: SessionTarget,
    sink: Arc<Capture>,
) -> RuntimeOwner {
    let project = std::fs::canonicalize(project).expect("canonical project");
    let working_directory = interpreter_script_path(project.to_str().expect("UTF-8 fixture path"));
    spawn_runtime(
        RuntimePlan {
            binding: binding(),
            launch: LaunchPlan {
                trampoline: trampoline.to_owned(),
                executable: vendor.to_owned(),
                working_directory: project,
                spec: LaunchSpec {
                    executable: vendor.to_owned(),
                    arguments: Vec::new(),
                    environment_names: SYSTEM_ENVIRONMENT_NAMES
                        .iter()
                        .map(|name| (*name).to_owned())
                        .collect(),
                },
            },
            protocol: SessionConfig {
                target,
                generation_id: Uuid::new_v4(),
                working_directory,
                model: None,
            },
            protocol_limits: SessionLimits::default(),
            limits: RuntimeLimits::default(),
        },
        sink,
    )
    .expect("owned actor")
}

fn binaries() -> (PathBuf, PathBuf) {
    let test = std::env::current_exe().expect("test image");
    let profile = test.parent().expect("deps").parent().expect("profile");
    let fixture = profile.join(format!(
        "vibemux_chat_fixture{}",
        std::env::consts::EXE_SUFFIX
    ));
    let trampoline = profile.join(format!(
        "vibemux_launch_trampoline{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        fixture.is_file() && trampoline.is_file(),
        "build test-only vibemux_chat_fixture and vibemux_launch_trampoline first"
    );
    (fixture, trampoline)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit native process acceptance: build the test-only chat fixture and launch trampoline"]
async fn persistent_native_two_turns_keep_the_exact_process_and_close_only_its_owned_tree() {
    let (fixture, trampoline) = binaries();
    for claude in [false, true] {
        let temp = tempfile::tempdir().expect("test root");
        let project = temp.path().join("project 空间");
        let vendor_dir = temp.path().join("vendor 空间");
        std::fs::create_dir(&project).expect("project");
        std::fs::create_dir(&vendor_dir).expect("vendor");
        let vendor = vendor_dir.join(format!("chat_fixture{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(&fixture, &vendor).expect("copy owned fixture");
        let trace = vendor_dir.join("owned_turns.txt");
        std::fs::write(
            vendor_dir.join("chat_fixture.json"),
            json!({"mode":"normal","ready_path":trace}).to_string(),
        )
        .expect("fixture configuration");
        let sink = Arc::new(Capture::default());
        let session_id = Uuid::new_v4();
        let target = if claude {
            SessionTarget::Claude {
                session_id,
                resume: false,
            }
        } else {
            SessionTarget::Codex {
                resume_thread_id: None,
            }
        };
        let owner = launch(&project, &vendor, &trampoline, target, sink.clone());
        owner
            .handle
            .wait_ready(WAIT)
            .await
            .expect("native handshake");
        let mut requests = Vec::new();
        for ordinal in 1..=2 {
            let request = TurnRequest {
                request_id: Uuid::new_v4(),
                turn_id: Uuid::new_v4(),
                prompt: format!("synthetic native turn {ordinal}; $(never execute)"),
            };
            requests.push(request.request_id);
            owner.handle.send(request, WAIT).await.expect("native send");
            sink.wait_completed(ordinal).await;
            let ready = owner
                .handle
                .wait_ready(WAIT)
                .await
                .expect("same owner ready for another turn");
            assert_eq!(
                ready.last_turn.expect("completed turn").outcome,
                Some(TurnOutcome::Completed)
            );
        }
        assert_ne!(requests[0], requests[1]);
        let trace = std::fs::read_to_string(&trace).expect("private fixture trace");
        let lines = trace.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0].split(':').next(),
            lines[1].split(':').next(),
            "both turns reached one actual vendor process"
        );
        let other_sink = Arc::new(Capture::default());
        let other = launch(
            &project,
            &vendor,
            &trampoline,
            SessionTarget::Codex {
                resume_thread_id: None,
            },
            other_sink.clone(),
        );
        other
            .handle
            .wait_ready(WAIT)
            .await
            .expect("separate owner ready");
        let closed = owner.shutdown().await.expect("owned shutdown");
        assert!(closed.cleanup.root_reaped);
        assert!(closed.cleanup.tree_empty);
        assert!(closed.cleanup.cleanup_verified);
        other
            .handle
            .send(
                TurnRequest {
                    request_id: Uuid::new_v4(),
                    turn_id: Uuid::new_v4(),
                    prompt: "other owned process remains usable".into(),
                },
                WAIT,
            )
            .await
            .expect("closing one owner never kills the other");
        other_sink.wait_completed(1).await;
        assert!(
            other
                .shutdown()
                .await
                .expect("other shutdown")
                .cleanup
                .cleanup_verified
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit native process acceptance: build the test-only chat fixture and launch trampoline"]
async fn root_death_is_detected_while_a_descendant_keeps_stdout_open() {
    let (fixture, trampoline) = binaries();
    let temp = tempfile::tempdir().expect("native failure fixture");
    let project = temp.path().join("project");
    let vendor_dir = temp.path().join("vendor");
    std::fs::create_dir(&project).expect("project");
    std::fs::create_dir(&vendor_dir).expect("vendor");
    let vendor = vendor_dir.join(format!("chat_fixture{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(&fixture, &vendor).expect("fixture image");
    let trace = vendor_dir.join("turns.txt");
    std::fs::write(
        vendor_dir.join("chat_fixture.json"),
        json!({"mode":"descendant_pipe","ready_path":trace}).to_string(),
    )
    .expect("fixture mode");
    let owner = launch(
        &project,
        &vendor,
        &trampoline,
        SessionTarget::Codex {
            resume_thread_id: None,
        },
        Arc::new(Capture::default()),
    );
    owner.handle.wait_ready(WAIT).await.expect("ready");
    let request_id = Uuid::new_v4();
    owner
        .handle
        .send(
            TurnRequest {
                request_id,
                turn_id: Uuid::new_v4(),
                prompt: "synthetic descendant control".into(),
            },
            WAIT,
        )
        .await
        .expect("send");
    let started = std::time::Instant::now();
    let mut snapshots = owner.handle.snapshots.clone();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if snapshots.borrow().phase
                == vibemux_harness::chat_session::SessionPhase::RecoveryRequired
            {
                break;
            }
            snapshots.changed().await.expect("owned status");
        }
    })
    .await
    .expect("root death must not wait for descendant pipe EOF");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        trace.with_extension("descendant").is_file(),
        "an actual owned descendant held the output pipe"
    );
    let exit = owner.shutdown().await.expect("bounded owner join");
    assert_eq!(exit.reason, RuntimeStopReason::ProcessLost);
    assert_eq!(
        exit.snapshot
            .active_turn
            .expect("unreplayed active turn")
            .request_id,
        request_id
    );
    assert!(exit.cleanup.root_reaped && exit.cleanup.tree_empty && exit.cleanup.cleanup_verified);
}
