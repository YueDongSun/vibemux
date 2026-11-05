use super::*;
use std::sync::mpsc::sync_channel;
use storage::{OwnedRoot, PLAN_FILE, QaRun, RUN_FILE, STOP_FILE, StopRequest};

#[test]
fn surfaces_preparation_uses_the_same_absolute_owned_root_boundary() {
    assert_eq!(
        execute(vec![
            "prepare_surfaces".into(),
            "--root".into(),
            "relative".into()
        ])
        .expect_err("surfaces must not adopt a relative root"),
        QaError::InvalidRoot
    );
}

#[test]
fn cli_refuses_unknown_actions_extra_identifiers_and_relative_roots_without_gui() {
    assert_eq!(
        execute(vec!["run".into(), "--pid".into(), "123".into()]).expect_err("no PID control"),
        QaError::InvalidArguments
    );
    assert_eq!(
        execute(vec!["unknown".into(), "--root".into(), "relative".into()])
            .expect_err("closed actions"),
        QaError::InvalidArguments
    );
    assert_eq!(
        execute(vec!["prepare".into(), "--root".into(), "relative".into()])
            .expect_err("absolute owned root"),
        QaError::InvalidRoot
    );
}

#[test]
fn unresolved_diagnostics_take_priority_over_every_window_failure_kind() {
    let temporary = tempfile::tempdir().expect("owned namespace");
    let root = OwnedRoot::create(temporary.path().join("window outcomes")).expect("root");
    for fault in [
        "panic",
        "before_ready",
        "missing_owner",
        "ownership",
        "monitor",
    ] {
        let mut report = QaReport::new(&root, QaPhase::GuiStartupFailed);
        let result = assess_window_completion(
            &mut report,
            WindowCompletion {
                monitored: if fault == "monitor" {
                    Err(QaError::OwnershipChanged)
                } else {
                    Ok(())
                },
                ready: fault != "before_ready",
                event_loop_returned: fault != "panic",
                owner_captured: fault != "missing_owner",
                ownership_changed: fault == "ownership",
                unresolved_asks: Some(0),
                unresolved_probes: 1,
            },
        );
        assert_eq!(
            result,
            Err(QaError::CleanupUnconfirmed),
            "unresolved owner plus {fault}"
        );
        assert_eq!(
            report.unresolved_asks,
            Some(0),
            "probe uncertainty must not become an Ask count"
        );
    }
}

#[test]
#[ignore = "Build the fixed chat fixture, registry mock and trampoline; owned Windows QA preparation only, no GUI or installed vendor"]
fn cleanup_review_preparation_retains_history_and_reopens_its_exact_scenario() {
    let temp = tempfile::tempdir().expect("fixture");
    let path = temp.path().join("DEMO cleanup review 空间");
    let prepared = execute(vec![
        "prepare_cleanup".into(),
        "--root".into(),
        path.clone().into_os_string(),
    ])
    .expect("prepare an explicitly finalized cleanup-review fixture");
    assert_eq!(prepared.phase, QaPhase::Prepared);
    assert_eq!(prepared.daemon_joined, Some(true));
    assert_eq!(
        prepared.baseline.as_ref().map(|summary| (
            summary.saved_records,
            summary.archived_records,
            summary.workspace_records,
            summary.process_claims_present,
        )),
        Some((4, 2, 2, false))
    );
    let checked = check(path.clone()).expect("reopen exact scenario");
    assert_eq!(checked.baseline, prepared.baseline);
    assert_eq!(checked.daemon_joined, Some(true));
    let root = OwnedRoot::open(path).expect("owned root");
    let plan: serde_json::Value = root.read(PLAN_FILE).expect("bounded plan");
    assert_eq!(plan["schema_version"], 2);
    assert_eq!(plan["scenario"], "cleanup_review");
    assert!(root.optional::<QaRun>(RUN_FILE).expect("no run").is_none());

    let (plan, scenario) = load_plan(&root).expect("validated plan");
    let layout = seed::open_layout(root.path(), scenario).expect("pinned cleanup layout");
    let runtime = runtime().expect("runtime");
    let server = start_server(&runtime, &layout).expect("registered terminal ready");
    let observed = runtime.block_on(async {
        use vibemuxd::control::{
            ControlClient, ControlError, WorkspaceCleanupRequest, WorkspaceCleanupScope,
        };
        let client = ControlClient::from_descriptor(server.descriptor_path())?;
        let status = match client
            .chat_execution(&ChatExecutionRequest::Status {
                chat_id: plan.ids.workspace_chat,
            })
            .await?
        {
            ChatExecutionResponse::Status {
                runtime: Some(status),
            } => status,
            _ => return Err(ControlError::InvalidFrame),
        };
        let scope = WorkspaceCleanupScope {
            project_id: status.owner.project_id,
            task_id: status.owner.task_id,
            run_id: status.owner.run_id,
        };
        let previous = client
            .workspace_cleanup(WorkspaceCleanupRequest::Status {
                scope,
                cleanup_id: None,
            })
            .await?;
        let preview = client
            .workspace_cleanup(WorkspaceCleanupRequest::Plan {
                scope,
                operation_id: uuid::Uuid::new_v4(),
            })
            .await?;
        Ok::<_, ControlError>((status, previous, preview))
    });
    finish_server(&runtime, server).expect("owned daemon and observer joined");
    let (status, previous, preview) = observed.expect("fresh production cleanup plan");
    assert_eq!(
        status.owner.phase,
        vibemux_types::chat_owner::ChatOwnerPhase::Finalized
    );
    assert!(!status.process_claimed);
    assert!(
        previous.is_none(),
        "preparation must not seed a cleanup Plan/Confirm/Arm"
    );
    assert!(preview.expect("plan").can_confirm);
}

#[test]
fn plan_versions_preserve_legacy_history_and_require_an_explicit_v2_scenario() {
    for (version, scenario, expected) in [
        (1, None, Some(QaScenario::History)),
        (1, Some("cleanup_review"), None),
        (1, Some("surfaces"), None),
        (2, None, None),
        (2, Some("history"), Some(QaScenario::History)),
        (2, Some("cleanup_review"), Some(QaScenario::CleanupReview)),
        (2, Some("surfaces"), Some(QaScenario::Surfaces)),
        (3, Some("cleanup_review"), None),
        (2, Some("unrecognized"), None),
    ] {
        let temp = tempfile::tempdir().expect("fixture");
        let root = OwnedRoot::create(temp.path().join("versioned plan")).expect("root");
        let mut value = serde_json::json!({
            "schema_version": version, "root_id": root.id(),
            "ids": {"saved_chat":ChatId::new(), "archived_chat":ChatId::new(),
                "workspace_chat":ChatId::new(), "workspace_run":RunId::new()},
            "baseline": {"saved_records":4,"archived_records":2,"workspace_records":2,"process_claims_present":false}
        });
        if let Some(scenario) = scenario {
            value["scenario"] = scenario.into();
        }
        root.write_new(PLAN_FILE, &value).expect("owned plan");
        let actual = load_plan(&root).map(|(_, scenario)| scenario);
        match expected {
            Some(expected) => assert_eq!(actual.expect("supported scenario"), expected),
            None => assert!(actual.is_err(), "reject {version}/{scenario:?}"),
        }
    }
}

#[test]
#[ignore = "Build the fixed chat fixture and trampoline; real Control/private-body QA acceptance only, no GUI or installed vendor"]
fn surfaces_preparation_reads_retained_tools_and_reopens_without_starting_a_window() {
    let temporary = tempfile::tempdir().expect("owned namespace");
    let path = temporary.path().join("DEMO surfaces 空间");
    let prepared = execute(vec![
        "prepare_surfaces".into(),
        "--root".into(),
        path.clone().into_os_string(),
    ])
    .expect("prepare fixed surface history");
    assert_eq!(prepared.phase, QaPhase::Prepared);
    assert_eq!(prepared.daemon_joined, Some(true));
    assert!(!prepared.production_profile_qualified && prepared.fixture_only);
    assert_eq!(
        prepared.baseline.as_ref().map(|summary| (
            summary.saved_records,
            summary.archived_records,
            summary.workspace_records,
            summary.process_claims_present
        )),
        Some((8, 2, 4, false))
    );
    let checked = check(path.clone()).expect("strict private-body readback after restart");
    assert_eq!(checked.phase, QaPhase::Checked);
    assert_eq!(checked.baseline, prepared.baseline);
    assert_eq!(checked.daemon_joined, Some(true));
    assert!(checked.gui_ready_observed.is_none() && checked.run_id.is_none());
    let root = OwnedRoot::open(path).expect("owned root");
    let (_, scenario) = load_plan(&root).expect("matching private plan");
    assert_eq!(scenario, QaScenario::Surfaces);
    assert!(
        root.optional::<QaRun>(RUN_FILE)
            .expect("no GUI run record")
            .is_none()
    );
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.path().join("vendor/chat_fixture.json")).expect("fixed fixture config"),
    )
    .expect("config");
    assert_eq!(config, serde_json::json!({"mode":"native_surfaces"}));
}

#[test]
#[ignore = "Build the fixed chat fixture and trampoline; actual owned server teardown only, no GUI or installed vendor"]
fn failed_or_timed_out_readiness_joins_the_created_server_before_returning() {
    let temp = tempfile::tempdir().expect("fixture");
    let path = temp.path().join("DEMO startup readiness");
    prepare(path.clone(), QaScenario::History).expect("normal history");
    let root = OwnedRoot::open(path).expect("root");
    let layout = seed::open_layout(root.path(), QaScenario::History).expect("layout");
    let runtime = runtime().expect("runtime");
    for deadline in [false, true] {
        let server = start_server(&runtime, &layout).expect("fresh owned server");
        let result = finish_startup(&runtime, server, async {
            if deadline {
                tokio::time::timeout(Duration::from_millis(30), std::future::pending::<()>())
                    .await
                    .map_err(|_| QaError::RuntimeUnavailable)?;
            }
            Err(QaError::RuntimeUnavailable)
        });
        assert_eq!(result.err(), Some(QaError::RuntimeUnavailable));
        let replacement = start_server(&runtime, &layout)
            .expect("failed readiness released the exact writer and endpoint");
        finish_server(&runtime, replacement).expect("replacement joined");
    }
}
#[test]
fn stop_is_nonce_bound_idempotent_and_never_accepts_a_target_pid() {
    let temp = tempfile::tempdir().expect("fixture");
    let path = temp.path().join("owned stop");
    let root = OwnedRoot::create(path.clone()).expect("root");
    assert_eq!(
        stop(path.clone()).expect("no run").phase,
        QaPhase::NotRunning
    );
    let run = QaRun {
        schema_version: QA_SCHEMA_VERSION,
        root_id: root.id(),
        run_id: uuid::Uuid::new_v4(),
        runner_pid: std::process::id(),
    };
    root.write_new(RUN_FILE, &run).expect("own run receipt");
    assert_eq!(
        stop(path.clone()).expect("request").phase,
        QaPhase::StopRequested
    );
    assert_eq!(
        stop(path.clone()).expect("same request").phase,
        QaPhase::StopRequested
    );
    let request: StopRequest = root.read(STOP_FILE).expect("request");
    assert_eq!(request.run_id, run.run_id);
    let mut value = serde_json::to_value(&request).expect("metadata");
    value["pid"] = serde_json::json!(123);
    assert!(serde_json::from_value::<StopRequest>(value).is_err());
    std::fs::write(
        path.join(STOP_FILE),
        serde_json::to_vec(&StopRequest {
            run_id: uuid::Uuid::new_v4(),
            ..request
        })
        .expect("mismatch"),
    )
    .expect("owned corruption");
    assert_eq!(
        stop(path).expect_err("foreign run nonce"),
        QaError::OwnershipChanged
    );
}

#[test]
fn stop_refuses_persisted_schema_or_qualification_claim_changes() {
    let temp = tempfile::tempdir().expect("fixture");
    let path = temp.path().join("outcome");
    let root = OwnedRoot::create(path.clone()).expect("root");
    let run = QaRun {
        schema_version: QA_SCHEMA_VERSION,
        root_id: root.id(),
        run_id: uuid::Uuid::new_v4(),
        runner_pid: std::process::id(),
    };
    root.write_new(RUN_FILE, &run).expect("run");
    let mut report = QaReport::new(&root, QaPhase::GuiExited);
    report.run_id = Some(run.run_id);
    report.daemon_joined = Some(true);
    report.monitor_joined = Some(true);
    report.gui_ready_observed = Some(true);
    report.event_loop_returned = Some(true);
    report.unresolved_asks = Some(0);
    root.write_new(OUTCOME_FILE, &report).expect("outcome");
    assert_eq!(
        stop(path.clone()).expect("valid completed outcome").phase,
        QaPhase::AlreadyStopped
    );
    for variant in 0..4 {
        let mut changed = report.clone();
        match variant {
            0 => changed.schema_version += 1,
            1 => changed.fixture_only = false,
            2 => changed.production_profile_qualified = true,
            _ => changed.phase = QaPhase::Prepared,
        }
        std::fs::write(
            path.join(OUTCOME_FILE),
            serde_json::to_vec(&changed).expect("changed metadata"),
        )
        .expect("owned mutation");
        assert_eq!(
            stop(path.clone()).expect_err("synthetic runner never replays qualification"),
            QaError::OwnershipChanged
        );
    }
}
#[test]
fn the_owned_deadline_monitor_requests_exit_once_and_is_joined() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = Arc::new(OwnedRoot::create(temp.path().join("deadline")).expect("root"));
    let run = QaRun {
        schema_version: QA_SCHEMA_VERSION,
        root_id: root.id(),
        run_id: uuid::Uuid::new_v4(),
        runner_pid: std::process::id(),
    };
    let (sent, received) = sync_channel(1);
    let monitor = supervision::Monitor::start(
        root,
        run,
        Duration::from_millis(20),
        || Ok(()),
        move || {
            sent.send(()).expect("test exit");
        },
    )
    .expect("monitor");
    received
        .recv_timeout(Duration::from_secs(2))
        .expect("bounded deadline");
    assert_eq!(
        monitor.finish().expect("joined"),
        supervision::StopReason::Deadline
    );
    assert!(received.try_recv().is_err());
}
#[test]
fn the_owned_stop_monitor_checks_both_root_and_run_before_exit() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = Arc::new(OwnedRoot::create(temp.path().join("stop")).expect("root"));
    let run = QaRun {
        schema_version: QA_SCHEMA_VERSION,
        root_id: root.id(),
        run_id: uuid::Uuid::new_v4(),
        runner_pid: std::process::id(),
    };
    root.write_new(
        STOP_FILE,
        &StopRequest {
            schema_version: QA_SCHEMA_VERSION,
            root_id: root.id(),
            run_id: run.run_id,
        },
    )
    .expect("stop");
    let (sent, received) = sync_channel(1);
    let monitor = supervision::Monitor::start(
        root,
        run,
        Duration::from_secs(5),
        || Ok(()),
        move || {
            sent.send(()).expect("test exit");
        },
    )
    .expect("monitor");
    received
        .recv_timeout(Duration::from_secs(2))
        .expect("matching stop");
    assert_eq!(
        monitor.finish().expect("joined"),
        supervision::StopReason::Requested
    );
}

#[test]
fn observed_layout_change_requests_exit_and_retains_ownership_failure() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = Arc::new(OwnedRoot::create(temp.path().join("changed layout")).expect("root"));
    let run = QaRun {
        schema_version: QA_SCHEMA_VERSION,
        root_id: root.id(),
        run_id: uuid::Uuid::new_v4(),
        runner_pid: std::process::id(),
    };
    let (sent, received) = sync_channel(1);
    let monitor = supervision::Monitor::start(
        root,
        run,
        Duration::from_secs(5),
        || Err(QaError::OwnershipChanged),
        move || {
            sent.send(()).expect("exit");
        },
    )
    .expect("monitor");
    received
        .recv_timeout(Duration::from_secs(2))
        .expect("fail closed exit");
    assert_eq!(monitor.finish(), Err(QaError::OwnershipChanged));
}

#[test]
#[ignore = "Build the exact synthetic chat fixture and launch trampoline; native Git/process/ACL acceptance, no GUI or installed vendor"]
fn prepare_check_and_failed_check_drain_the_exact_owned_daemon_without_gui() {
    let temp = tempfile::tempdir().expect("fixture");
    let path = temp.path().join("DEMO native QA 空间");
    let prepared = prepare(path.clone(), QaScenario::History).expect("prepared native history");
    assert_eq!(prepared.phase, QaPhase::Prepared);
    assert_eq!(
        prepared.baseline.as_ref().map(|s| (
            s.saved_records,
            s.archived_records,
            s.workspace_records
        )),
        Some((4, 2, 2))
    );
    assert_eq!(prepared.daemon_joined, Some(true));
    assert_eq!(prepared.gui_ready_observed, None);
    assert_eq!(
        prepare(path.clone(), QaScenario::History).expect_err("never adopt existing namespace"),
        QaError::RootExists
    );
    let first = check(path.clone()).expect("fresh AppState authenticated check");
    let second = check(path.clone()).expect("repeat check");
    assert_eq!(first, second);
    assert_eq!(
        run(path.clone(), |_, _, _, _| Err(QaError::RuntimeUnavailable))
            .expect_err("startup failure stays a failure"),
        QaError::RuntimeUnavailable
    );
    let stopped = stop(path.clone()).expect("failed startup joined its owned daemon");
    assert_eq!(stopped.phase, QaPhase::AlreadyStopped);
    assert_eq!(stopped.daemon_joined, Some(true));
    assert_eq!(stopped.gui_ready_observed, None);
    let root = OwnedRoot::open(path.clone()).expect("owned root");
    let mut plan: QaPlan = root.read(PLAN_FILE).expect("plan");
    plan.ids.saved_chat = ChatId::new();
    std::fs::write(
        path.join(PLAN_FILE),
        serde_json::to_vec(&plan).expect("changed known metadata"),
    )
    .expect("owned mutation");
    assert!(check(path.clone()).is_err());
    let layout = seed::open_layout(&path, QaScenario::History).expect("layout retained");
    let runtime = runtime().expect("runtime");
    let server =
        start_server(&runtime, &layout).expect("failed check released its writer and endpoint");
    finish_server(&runtime, server).expect("joined replacement observation");
    let panic_path = temp.path().join("DEMO panic before Ready");
    prepare(panic_path.clone(), QaScenario::History).expect("prepare independent panic case");
    assert_eq!(
        run(panic_path.clone(), |_, _, _, _| panic!(
            "injected native startup panic"
        ))
        .expect_err("panic stays failed"),
        QaError::RuntimeUnavailable
    );
    let panic_stopped = stop(panic_path).expect("panic still joined owned daemon");
    assert_eq!(panic_stopped.phase, QaPhase::AlreadyStopped);
    assert_eq!(panic_stopped.daemon_joined, Some(true));
    assert_eq!(panic_stopped.unresolved_asks, None);
    assert!(path.join("project/.git/HEAD").is_file());
    assert!(path.join("webview").is_dir());
}

fn stop_publication_fixture(name: &str) -> (tempfile::TempDir, Arc<OwnedRoot>, QaRun, StopRequest) {
    let temp = tempfile::tempdir().expect("owned stop fixture");
    let root = Arc::new(OwnedRoot::create(temp.path().join(name)).expect("owned root"));
    let run = QaRun {
        schema_version: QA_SCHEMA_VERSION,
        root_id: root.id(),
        run_id: uuid::Uuid::new_v4(),
        runner_pid: std::process::id(),
    };
    root.write_new(RUN_FILE, &run).expect("owned run");
    let request = StopRequest {
        schema_version: QA_SCHEMA_VERSION,
        root_id: root.id(),
        run_id: run.run_id,
    };
    (temp, root, run, request)
}

#[test]
fn direct_final_name_writer_exposes_a_matching_stop_before_its_exclusive_handle_closes() {
    use std::{fs::OpenOptions, io::Write, os::windows::fs::OpenOptionsExt};
    let (_temp, root, run, request) = stop_publication_fixture("direct publication window");
    let mut writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(0x0020_0000)
        .open(root.path().join(STOP_FILE))
        .expect("old direct writer");
    writer
        .write_all(&serde_json::to_vec_pretty(&request).expect("matching JSON"))
        .expect("write");
    writer.sync_all().expect("sync");
    let (sent, received) = sync_channel(1);
    let monitor = supervision::Monitor::start(
        root.clone(),
        run,
        Duration::from_secs(5),
        || Ok(()),
        move || {
            sent.send(()).expect("monitor exit");
        },
    )
    .expect("monitor");
    received
        .recv_timeout(Duration::from_secs(2))
        .expect("reader encounters held writer");
    assert_eq!(monitor.finish(), Err(QaError::OwnershipChanged));
    drop(writer);
    assert_eq!(
        root.read::<StopRequest>(STOP_FILE)
            .expect("complete matching stop after close"),
        request
    );
}

#[test]
fn atomic_stop_is_complete_when_the_monitor_observes_its_final_name() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (_temp, root, run, request) = stop_publication_fixture("atomic publication");
    let (entered, observed) = sync_channel(1);
    let (release, continue_read) = sync_channel(1);
    let (sent, received) = sync_channel(1);
    let first = AtomicBool::new(true);
    let monitor = supervision::Monitor::start(
        root.clone(),
        run,
        Duration::from_secs(5),
        move || {
            if first.swap(false, Ordering::SeqCst) {
                entered.send(()).map_err(|_| QaError::RuntimeUnavailable)?;
                continue_read
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|_| QaError::RuntimeUnavailable)?;
            }
            Ok(())
        },
        move || {
            sent.send(()).expect("monitor exit");
        },
    )
    .expect("monitor");
    observed
        .recv_timeout(Duration::from_secs(2))
        .expect("monitor reached pre-read seam");
    assert!(!root.path().join(STOP_FILE).exists());
    assert_eq!(
        root.publish_stop(&request).expect("atomic publication"),
        None
    );
    assert_eq!(
        root.read::<StopRequest>(STOP_FILE)
            .expect("published matching stop"),
        request
    );
    assert!(
        !std::fs::read_dir(root.path())
            .expect("owned entries")
            .any(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(storage::STOP_STAGE_PREFIX)
            })
    );
    release.send(()).expect("allow final-name observation");
    received
        .recv_timeout(Duration::from_secs(2))
        .expect("requested stop");
    assert_eq!(
        monitor.finish().expect("joined"),
        supervision::StopReason::Requested
    );
}

#[test]
fn stop_publication_preserves_existing_matching_foreign_and_malformed_destinations() {
    for variant in 0..3 {
        let (_temp, root, _run, request) = stop_publication_fixture("existing destination");
        let existing = match variant {
            0 => serde_json::to_vec(&request).expect("matching compact JSON"),
            1 => serde_json::to_vec(&StopRequest {
                run_id: uuid::Uuid::new_v4(),
                ..request.clone()
            })
            .expect("foreign nonce"),
            _ => b"unknown existing stop bytes".to_vec(),
        };
        std::fs::write(root.path().join(STOP_FILE), &existing).expect("existing destination");
        let result = root.publish_stop(&request);
        if variant == 0 {
            assert_eq!(result.expect("idempotent matching destination"), None);
        } else {
            assert!(result.is_err());
        }
        assert_eq!(
            std::fs::read(root.path().join(STOP_FILE)).expect("preserved destination"),
            existing
        );
        assert!(
            !std::fs::read_dir(root.path())
                .expect("owned entries")
                .any(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .starts_with(storage::STOP_STAGE_PREFIX)
                })
        );
    }
}

#[test]
fn stop_publication_rejects_foreign_schema_root_or_run_without_creating_a_final_stop() {
    for variant in 0..3 {
        let (_temp, root, _run, mut request) = stop_publication_fixture("request binding");
        match variant {
            0 => request.schema_version += 1,
            1 => request.root_id = uuid::Uuid::new_v4(),
            _ => request.run_id = uuid::Uuid::new_v4(),
        }
        assert_eq!(root.publish_stop(&request), Err(QaError::OwnershipChanged));
        assert!(!root.path().join(STOP_FILE).exists());
        assert!(
            !std::fs::read_dir(root.path())
                .expect("owned entries")
                .any(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .starts_with(storage::STOP_STAGE_PREFIX)
                })
        );
    }
}

#[test]
fn same_nonce_competing_closed_stages_both_succeed_without_replacing_the_winner() {
    use std::{sync::Barrier, thread};
    let (_temp, root, _run, request) = stop_publication_fixture("matching competing publishers");
    let first_name = format!(
        "{}{}_{}.json",
        storage::STOP_STAGE_PREFIX,
        request.run_id,
        uuid::Uuid::new_v4()
    );
    let second_name = format!(
        "{}{}_{}.json",
        storage::STOP_STAGE_PREFIX,
        request.run_id,
        uuid::Uuid::new_v4()
    );
    root.write_new(&first_name, &request)
        .expect("first owned closed stage");
    root.write_new(&second_name, &request)
        .expect("second owned closed stage");
    // Distinct encodings make a replacement visible even though both nonces match.
    let second_bytes = serde_json::to_vec(&request).expect("compact matching request");
    std::fs::write(root.path().join(&second_name), &second_bytes)
        .expect("finish owned second stage");
    let first_bytes = std::fs::read(root.path().join(&first_name)).expect("first stage bytes");
    assert_ne!(first_bytes, second_bytes);
    let barrier = Arc::new(Barrier::new(3));
    let (winner_sent, winner_ready) = sync_channel(1);
    let (allow_loser, loser_ready) = sync_channel(1);
    let winner_root = root.clone();
    let winner_barrier = barrier.clone();
    let winner_request = request.clone();
    let winner_name = first_name.clone();
    let winner = thread::spawn(move || {
        assert!(
            !winner_root.path().join(STOP_FILE).exists(),
            "absent before competing publication"
        );
        winner_barrier.wait();
        let result = winner_root.publish_stop_stage(winner_name, &winner_request);
        winner_sent
            .send(result.clone())
            .expect("winner publication receipt");
        result
    });
    let loser_root = root.clone();
    let loser_barrier = barrier.clone();
    let loser_request = request.clone();
    let loser_name = second_name.clone();
    let loser = thread::spawn(move || {
        assert!(
            !loser_root.path().join(STOP_FILE).exists(),
            "absent before competing publication"
        );
        loser_barrier.wait();
        loser_ready
            .recv_timeout(Duration::from_secs(2))
            .expect("winner has published");
        loser_root.publish_stop_stage(loser_name, &loser_request)
    });
    barrier.wait();
    let announced = winner_ready.recv_timeout(Duration::from_secs(2));
    let winner_bytes = std::fs::read(root.path().join(STOP_FILE));
    let _ = allow_loser.send(());
    let winner_result = winner
        .join()
        .expect("winner joined")
        .expect("winner succeeds");
    let loser_result = loser
        .join()
        .expect("loser joined")
        .expect("matching duplicate succeeds");
    assert!(announced.expect("winner announced").is_ok());
    let winner_bytes = winner_bytes.expect("winner final bytes");
    assert_eq!(winner_bytes, first_bytes);
    assert_eq!(
        std::fs::read(root.path().join(STOP_FILE)).expect("retained winner"),
        winner_bytes
    );
    assert_eq!(
        root.read::<StopRequest>(STOP_FILE).expect("same nonce"),
        request
    );
    let mut accounted = Vec::new();
    for (name, result) in [(first_name, winner_result), (second_name, loser_result)] {
        match result {
            None => assert!(!root.path().join(name).exists(), "owned stage removed"),
            Some(retained) => {
                assert_eq!(
                    retained, name,
                    "only the publisher's own stage may be retained"
                );
                assert_eq!(
                    root.read::<StopRequest>(&retained)
                        .expect("owned recovery stage"),
                    request
                );
                accounted.push(retained);
            }
        }
    }
    let mut remaining = std::fs::read_dir(root.path())
        .expect("owned entries")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with(storage::STOP_STAGE_PREFIX))
        .collect::<Vec<_>>();
    remaining.sort();
    accounted.sort();
    assert_eq!(remaining, accounted, "no unaccounted staging files");
}
