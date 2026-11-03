use super::*;
use crate::{desktop_settings::DesktopSettings, project_registry::ProjectRegistry};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

struct Controlled {
    calls: AtomicUsize,
    outcomes: Mutex<mpsc::Receiver<ProbeOutcome>>,
}
impl ProbeRunner for Controlled {
    fn run(&self, cancel: watch::Receiver<bool>) -> ProbeOutcome {
        self.calls.fetch_add(1, Ordering::AcqRel);
        loop {
            if *cancel.borrow() {
                return ProbeOutcome {
                    report: None,
                    error_code: Some("local_probe_cancelled".into()),
                    quiescent: true,
                };
            }
            if let Ok(outcome) = self.outcomes.lock().expect("controlled outcome").try_recv() {
                return outcome;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
fn runner() -> (Arc<Controlled>, mpsc::Sender<ProbeOutcome>) {
    let (send, receive) = mpsc::channel();
    (
        Arc::new(Controlled {
            calls: AtomicUsize::new(0),
            outcomes: Mutex::new(receive),
        }),
        send,
    )
}
fn project() -> (tempfile::TempDir, Arc<RegisteredProject>) {
    let root = tempfile::tempdir().expect("owned folder");
    std::fs::create_dir(root.path().join(".git")).expect("synthetic repository marker");
    let project = ProjectRegistry::default()
        .register(root.path(), &DesktopSettings::default())
        .expect("registered project");
    (root, project)
}
fn status(
    registry: &ProbeRegistry,
    project: &Arc<RegisteredProject>,
    operation_id: Uuid,
) -> ProbeSnapshot {
    registry
        .request(
            Arc::clone(project),
            ProbeRequest::Status {
                operation_id: Some(operation_id),
            },
        )
        .expect("exact status")
}
fn settled(
    registry: &ProbeRegistry,
    project: &Arc<RegisteredProject>,
    operation_id: Uuid,
) -> ProbeSnapshot {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let view = status(registry, project, operation_id);
        if view
            .job
            .as_ref()
            .is_some_and(|job| job.phase != ProbePhase::Running)
        {
            return view;
        }
        assert!(Instant::now() < deadline, "owned worker should settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn success(value: u64) -> ProbeOutcome {
    ProbeOutcome {
        report: Some(json!({"synthetic_result":value})),
        error_code: None,
        quiescent: true,
    }
}

#[test]
fn status_is_inert_and_duplicate_start_or_other_projects_cannot_duplicate_the_worker() {
    let (_root, project) = project();
    let (_other_root, other) = self::project();
    let (runner, send) = runner();
    let registry = ProbeRegistry::new(runner.clone());
    let initial = registry
        .request(project.clone(), ProbeRequest::Status { operation_id: None })
        .expect("read only");
    assert!(initial.job.is_none() && initial.can_start);
    assert_eq!(runner.calls.load(Ordering::Acquire), 0);
    let id = Uuid::new_v4();
    registry
        .request(project.clone(), ProbeRequest::Start { operation_id: id })
        .expect("admit");
    registry
        .request(project.clone(), ProbeRequest::Start { operation_id: id })
        .expect("inert replay");
    assert_eq!(
        registry
            .request(
                other.clone(),
                ProbeRequest::Start {
                    operation_id: Uuid::new_v4()
                }
            )
            .err()
            .expect("global admission")
            .code,
        "probe_busy"
    );
    assert_eq!(
        registry
            .request(
                other,
                ProbeRequest::Status {
                    operation_id: Some(id)
                }
            )
            .err()
            .expect("foreign receipt")
            .code,
        "probe_operation_conflict"
    );
    send.send(success(1)).expect("complete once");
    let complete = settled(&registry, &project, id);
    assert_eq!(complete.job.expect("job").phase, ProbePhase::Completed);
    assert!(complete.can_start);
    assert_eq!(runner.calls.load(Ordering::Acquire), 1);
    registry
        .request(project, ProbeRequest::Start { operation_id: id })
        .expect("terminal replay");
    assert_eq!(registry.shutdown(), 0);
    assert_eq!(runner.calls.load(Ordering::Acquire), 1);
}

#[test]
fn previous_result_survives_failed_work_and_cancel_does_not_fabricate_completion() {
    let (_root, project) = project();
    let (runner, send) = runner();
    let registry = ProbeRegistry::new(runner);
    let first = Uuid::new_v4();
    registry
        .request(
            project.clone(),
            ProbeRequest::Start {
                operation_id: first,
            },
        )
        .expect("first");
    send.send(success(7)).expect("outcome");
    settled(&registry, &project, first);
    let second = Uuid::new_v4();
    registry
        .request(
            project.clone(),
            ProbeRequest::Start {
                operation_id: second,
            },
        )
        .expect("second");
    send.send(ProbeOutcome {
        report: None,
        error_code: Some("local_probe_report_invalid".into()),
        quiescent: true,
    })
    .expect("failure");
    let failure = settled(&registry, &project, second);
    assert_eq!(failure.job.expect("job").phase, ProbePhase::Failed);
    assert_eq!(failure.result.expect("prior result").operation_id, first);
    let third = Uuid::new_v4();
    registry
        .request(
            project.clone(),
            ProbeRequest::Start {
                operation_id: third,
            },
        )
        .expect("third");
    registry
        .request(
            project.clone(),
            ProbeRequest::Cancel {
                operation_id: third,
            },
        )
        .expect("cancel signal");
    assert_eq!(
        settled(&registry, &project, third).job.expect("job").phase,
        ProbePhase::Cancelled
    );
    assert_eq!(registry.shutdown(), 0);
}

#[test]
fn unconfirmed_cleanup_keeps_global_admission_fenced_and_shutdown_unconfirmed() {
    let (_root, project) = project();
    let (runner, send) = runner();
    let registry = ProbeRegistry::new(runner);
    let id = Uuid::new_v4();
    registry
        .request(project.clone(), ProbeRequest::Start { operation_id: id })
        .expect("admit");
    send.send(ProbeOutcome {
        report: Some(json!({"must_not_publish":true})),
        error_code: None,
        quiescent: false,
    })
    .expect("unknown");
    let view = settled(&registry, &project, id);
    assert_eq!(view.job.expect("job").phase, ProbePhase::Unconfirmed);
    assert!(view.result.is_none() && !view.can_start);
    assert_eq!(
        registry
            .request(
                project,
                ProbeRequest::Start {
                    operation_id: Uuid::new_v4()
                }
            )
            .err()
            .expect("no regrant")
            .code,
        "probe_busy"
    );
    assert_eq!(registry.shutdown(), 1);
}

#[test]
fn closing_the_owner_stops_admission_cancels_and_joins_the_accepted_worker() {
    let (_root, project) = project();
    let (runner, _send) = runner();
    let registry = ProbeRegistry::new(runner);
    let id = Uuid::new_v4();
    registry
        .request(project.clone(), ProbeRequest::Start { operation_id: id })
        .expect("admit");
    assert!(registry.has_unsettled(&project.key));
    registry.begin_shutdown();
    assert_eq!(
        registry
            .request(
                project.clone(),
                ProbeRequest::Start {
                    operation_id: Uuid::new_v4()
                }
            )
            .err()
            .expect("stop admission")
            .code,
        "probe_stopping"
    );
    assert_eq!(registry.shutdown(), 0);
    assert_eq!(
        status(&registry, &project, id).job.expect("receipt").phase,
        ProbePhase::Cancelled
    );
    assert!(!registry.has_unsettled(&project.key));
}

#[test]
fn history_capacity_retains_old_receipts_and_never_reexecutes_evicted_ids() {
    struct Immediate(AtomicUsize);
    impl ProbeRunner for Immediate {
        fn run(&self, _: watch::Receiver<bool>) -> ProbeOutcome {
            self.0.fetch_add(1, Ordering::AcqRel);
            success(1)
        }
    }
    let (_root, project) = project();
    let runner = Arc::new(Immediate(AtomicUsize::new(0)));
    let registry = ProbeRegistry::new(runner.clone());
    let first = Uuid::new_v4();
    for index in 0..MAX_OPERATIONS {
        let id = if index == 0 { first } else { Uuid::new_v4() };
        registry
            .request(project.clone(), ProbeRequest::Start { operation_id: id })
            .expect("bounded admission");
        settled(&registry, &project, id);
    }
    assert_eq!(
        registry
            .request(
                project.clone(),
                ProbeRequest::Start {
                    operation_id: Uuid::new_v4()
                }
            )
            .err()
            .expect("bounded history")
            .code,
        "probe_history_full"
    );
    registry
        .request(
            project,
            ProbeRequest::Start {
                operation_id: first,
            },
        )
        .expect("inert old receipt");
    assert_eq!(registry.shutdown(), 0);
    assert_eq!(runner.0.load(Ordering::Acquire), MAX_OPERATIONS);
}

#[test]
fn native_worker_panic_retains_an_unconfirmed_receipt_and_does_not_regrant() {
    struct Panics;
    impl ProbeRunner for Panics {
        fn run(&self, _: watch::Receiver<bool>) -> ProbeOutcome {
            panic!("synthetic diagnostic worker failure")
        }
    }
    let (_root, project) = project();
    let registry = ProbeRegistry::new(Arc::new(Panics));
    let id = Uuid::new_v4();
    registry
        .request(project.clone(), ProbeRequest::Start { operation_id: id })
        .expect("admit");
    let view = settled(&registry, &project, id);
    assert_eq!(view.job.expect("job").phase, ProbePhase::Unconfirmed);
    assert!(view.result.is_none() && !view.can_start);
    assert_eq!(registry.shutdown(), 1);
}

#[test]
fn repeated_shutdown_cannot_renew_an_expired_join_budget() {
    struct Held(Mutex<mpsc::Receiver<()>>);
    impl ProbeRunner for Held {
        fn run(&self, _: watch::Receiver<bool>) -> ProbeOutcome {
            let _ = self.0.lock().expect("gate").recv();
            success(1)
        }
    }
    let (_root, project) = project();
    let (release, held) = mpsc::channel();
    let registry = Arc::new(ProbeRegistry::new(Arc::new(Held(Mutex::new(held)))));
    registry
        .request(
            project,
            ProbeRequest::Start {
                operation_id: Uuid::new_v4(),
            },
        )
        .expect("admit");
    let first = registry.shutdown_with_wait(Duration::from_millis(30));
    let repeated = Arc::clone(&registry);
    let (sent, received) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let _ = sent.send(repeated.shutdown_with_wait(Duration::from_secs(2)));
    });
    let before_release = received.recv_timeout(Duration::from_millis(100));
    release
        .send(())
        .expect("always release owned worker before assertions");
    waiter.join().expect("shutdown observer joined");
    let final_deadline = Instant::now() + Duration::from_secs(1);
    let final_count = loop {
        let count = registry.shutdown();
        if count == 0 || Instant::now() >= final_deadline {
            break count;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(first, 1);
    assert_eq!(
        before_release.ok(),
        Some(1),
        "an expired first budget must not become another wait"
    );
    assert_eq!(
        final_count, 0,
        "later calls may still reap a now-finished worker"
    );
}

#[test]
fn known_owned_cancellation_remains_available_after_its_folder_moves() {
    let (root, project) = project();
    let (runner, _send) = runner();
    let registry = ProbeRegistry::new(runner);
    let id = Uuid::new_v4();
    registry
        .request(project.clone(), ProbeRequest::Start { operation_id: id })
        .expect("admit");
    let moved = root
        .path()
        .with_file_name(format!("moved-{}", Uuid::new_v4()));
    std::fs::rename(root.path(), &moved).expect("move only owned fixture folder");
    let cancel = registry.request(project.clone(), ProbeRequest::Cancel { operation_id: id });
    let joined = registry.shutdown();
    std::fs::rename(&moved, root.path()).expect("restore exact owned folder");
    cancel.expect("signal original owned job without new filesystem authority");
    assert_eq!(joined, 0);
    assert_eq!(
        status(&registry, &project, id).job.expect("receipt").phase,
        ProbePhase::Cancelled
    );
}
