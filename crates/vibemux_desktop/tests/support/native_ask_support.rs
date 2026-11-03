//! Deterministic service/worker fixtures; no vendor process or inference.

use std::{
    collections::{BTreeMap, VecDeque},
    io,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};

use uuid::Uuid;
use vibemux_client::{
    ask::{
        AskAdmission, AskCanonicalStatus, AskEnd, AskObservation, AskObserverState, AskOutputState,
        AskPhase, AskSubmissionState, ObservedAskHandle,
    },
    daemon_control::DaemonCommand,
};
use vibemux_desktop::{
    app_state::AppState,
    ask_registry::AskSpawner,
    command_error::CommandError,
    event_sink::EventSink,
    project_status::ProjectServices,
    ui_contract::{
        AskHarness, AskReceipt, AskTarget, AskTargets, DaemonState, LocalAskPhase, UiEvent,
    },
};
use vibemux_harness::{
    AgentKind,
    dispatch::{AttemptOutcome, DispatchPhase, NativeProtocol},
};
use vibemux_types::{RunId, TaskId};
use vibemuxd::process::DaemonPaths;

pub const WAIT: Duration = Duration::from_secs(10);
pub const PRIVATE_PROMPT: &str = "PRIVATE_PROMPT_ASK_TEST";
pub const PRIVATE_REPLY: &str = "PRIVATE_REPLY_ASK_TEST";

#[derive(Default)]
pub struct Gate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl Gate {
    pub fn enter_and_wait(&self) {
        let mut state = self.state.lock().expect("gate");
        state.0 = true;
        self.changed.notify_all();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, WAIT, |state| !state.1)
            .expect("gate wait");
        assert!(state.1, "test did not release its owned gate");
    }

    pub fn wait_entered(&self) {
        let state = self.state.lock().expect("gate");
        let (state, _) = self
            .changed
            .wait_timeout_while(state, WAIT, |state| !state.0)
            .expect("gate entry");
        assert!(state.0, "test worker did not enter gate");
    }

    pub fn release(&self) {
        self.state.lock().expect("gate").1 = true;
        self.changed.notify_all();
    }
}

#[derive(Default)]
pub struct EventProbe {
    events: Mutex<Vec<UiEvent>>,
    changed: Condvar,
}

impl EventProbe {
    pub fn all(&self) -> Vec<UiEvent> {
        self.events.lock().expect("events").clone()
    }

    pub fn wait_for(&self, predicate: impl Fn(&[UiEvent]) -> bool) {
        let events = self.events.lock().expect("events");
        let (events, _) = self
            .changed
            .wait_timeout_while(events, WAIT, |events| !predicate(events))
            .expect("event wait");
        assert!(
            predicate(&events),
            "expected public invalidation did not arrive"
        );
    }

    pub fn wait_phase(&self, ask_id: &str, phase: LocalAskPhase) {
        self.wait_for(|events| events.iter().any(|event| matches!(event,
            UiEvent::AskChanged { ask_id: id, phase: observed, .. } if id == ask_id && *observed == phase)));
    }
}

impl EventSink for EventProbe {
    fn emit(&self, event: UiEvent) {
        self.events.lock().expect("events").push(event);
        self.changed.notify_all();
    }
}

pub struct FakeServices {
    pub available: AtomicBool,
    pub setups: AtomicUsize,
    pub lifecycle: AtomicUsize,
    pub target_gate: Mutex<Option<Arc<Gate>>>,
}

impl Default for FakeServices {
    fn default() -> Self {
        Self {
            available: AtomicBool::new(true),
            setups: AtomicUsize::new(0),
            lifecycle: AtomicUsize::new(0),
            target_gate: Mutex::new(None),
        }
    }
}

impl ProjectServices for FakeServices {
    fn status(&self, _: &DaemonPaths) -> (DaemonState, Option<String>) {
        (DaemonState::Running, None)
    }

    fn targets(&self, _: &DaemonPaths) -> AskTargets {
        let gate = self.target_gate.lock().expect("target gate").take();
        if let Some(gate) = gate {
            gate.enter_and_wait();
        }
        let available = self.available.load(Ordering::Acquire);
        AskTargets {
            targets: [AskHarness::Claude, AskHarness::Codex]
                .into_iter()
                .map(|harness| AskTarget {
                    harness,
                    label: harness.label().to_owned(),
                    available,
                    reason: (!available).then(|| "Route unavailable in this test".to_owned()),
                })
                .collect(),
            setup_offered: false,
            code: None,
        }
    }

    fn lifecycle(&self, _: &DaemonPaths, _: DaemonCommand) -> Result<(), CommandError> {
        self.lifecycle.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn setup(&self, _: &DaemonPaths) -> Result<(), CommandError> {
        self.setups.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

pub enum Program {
    Complete,
    Uncertain,
    RunningThenLost,
    CompletedUnavailable,
    WaitCancel(DispatchPhase),
    CancelBeforeAdmission,
    Burst(usize),
    StaleSnapshot,
    ReplacingPartial(Arc<Gate>),
    FinalOnStop,
    WrongIdentity,
    RecoveryPending,
}

#[derive(Clone, Debug)]
pub struct Call {
    pub request_id: Uuid,
    pub observe_only: bool,
}

#[derive(Default)]
struct Progress {
    calls: Vec<Call>,
    finished: usize,
}

#[derive(Default)]
pub struct FakeSpawner {
    programs: Mutex<VecDeque<Program>>,
    observer_programs: Mutex<VecDeque<Program>>,
    identities: Mutex<BTreeMap<Uuid, (TaskId, RunId)>>,
    progress: Arc<(Mutex<Progress>, Condvar)>,
    pub cancellations: Arc<AtomicUsize>,
}

impl FakeSpawner {
    pub fn enqueue(&self, program: Program) {
        self.programs.lock().expect("programs").push_back(program);
    }
    pub fn enqueue_observer(&self, program: Program) {
        self.observer_programs
            .lock()
            .expect("observer programs")
            .push_back(program);
    }
    pub fn calls(&self) -> Vec<Call> {
        self.progress.0.lock().expect("progress").calls.clone()
    }
    pub fn submissions(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| !call.observe_only)
            .count()
    }
    pub fn observations(&self) -> usize {
        self.calls().iter().filter(|call| call.observe_only).count()
    }

    pub fn wait_calls(&self, count: usize) {
        let progress = self.progress.0.lock().expect("progress");
        let (progress, _) = self
            .progress
            .1
            .wait_timeout_while(progress, WAIT, |progress| progress.calls.len() < count)
            .expect("call wait");
        assert!(progress.calls.len() >= count, "worker was not started");
    }

    pub fn wait_finished(&self, count: usize) {
        let progress = self.progress.0.lock().expect("progress");
        let (progress, _) = self
            .progress
            .1
            .wait_timeout_while(progress, WAIT, |progress| progress.finished < count)
            .expect("finish wait");
        assert!(
            progress.finished >= count,
            "bounded worker did not finish; collector may not be draining"
        );
    }

    fn start(
        &self,
        harness: AgentKind,
        request_id: Uuid,
        observe_only: bool,
    ) -> io::Result<ObservedAskHandle> {
        let program = if observe_only {
            self.observer_programs.lock().expect("programs").pop_front()
        } else {
            self.programs.lock().expect("programs").pop_front()
        }
        .unwrap_or(Program::Complete);
        let (task_id, run_id) = *self
            .identities
            .lock()
            .expect("identities")
            .entry(request_id)
            .or_insert_with(|| {
                (
                    TaskId::from_uuid(Uuid::new_v4()).expect("task"),
                    RunId::from_uuid(Uuid::new_v4()).expect("run"),
                )
            });
        let (sender, updates) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let context = WorkerContext {
            request_id,
            harness,
            task_id,
            run_id,
            observe_only,
            sender,
            cancel: cancel.clone(),
            stop: stop.clone(),
            cancellations: self.cancellations.clone(),
        };
        let progress = self.progress.clone();
        self.progress.0.lock().expect("progress").calls.push(Call {
            request_id,
            observe_only,
        });
        self.progress.1.notify_all();
        let worker = std::thread::spawn(move || {
            context.run(program);
            progress.0.lock().expect("progress").finished += 1;
            progress.1.notify_all();
        });
        Ok(ObservedAskHandle::from_test_worker(
            request_id, updates, cancel, stop, worker,
        ))
    }
}

impl AskSpawner for FakeSpawner {
    fn spawn(
        &self,
        _: PathBuf,
        harness: AgentKind,
        _: String,
        request_id: Uuid,
    ) -> io::Result<ObservedAskHandle> {
        self.start(harness, request_id, false)
    }
    fn observe(
        &self,
        _: PathBuf,
        harness: AgentKind,
        request_id: Uuid,
    ) -> io::Result<ObservedAskHandle> {
        self.start(harness, request_id, true)
    }
}

struct WorkerContext {
    request_id: Uuid,
    harness: AgentKind,
    task_id: TaskId,
    run_id: RunId,
    observe_only: bool,
    sender: SyncSender<AskObservation>,
    cancel: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    cancellations: Arc<AtomicUsize>,
}

impl WorkerContext {
    fn snapshot(
        &self,
        phase: DispatchPhase,
        version: u64,
        reply: &str,
        output: AskOutputState,
    ) -> AskObservation {
        let protocol = if self.harness == AgentKind::Claude {
            NativeProtocol::ClaudeStreamJson
        } else {
            NativeProtocol::CodexAppServer
        };
        let mut observation = AskObservation::pending(self.request_id);
        observation.submission = AskSubmissionState::Confirmed;
        observation.observer = if phase.is_terminal() {
            AskObserverState::Finished
        } else {
            AskObserverState::Observing
        };
        observation.output = output;
        observation.update.phase = if phase.is_terminal() {
            AskPhase::Finished
        } else {
            AskPhase::Running
        };
        observation.update.reply = reply.into();
        observation.update.end = match phase {
            DispatchPhase::Completed => Some(AskEnd::Completed),
            DispatchPhase::Cancelled => Some(AskEnd::Cancelled),
            _ => None,
        };
        observation.admission = (!self.observe_only).then_some(AskAdmission {
            request_id: self.request_id,
            task_id: self.task_id,
            run_id: self.run_id,
            harness: self.harness,
            protocol,
            phase: DispatchPhase::Admitted,
            duplicate: false,
            sequence: 17,
        });
        observation.canonical = Some(AskCanonicalStatus {
            request_id: self.request_id,
            task_id: self.task_id,
            run_id: self.run_id,
            harness: self.harness,
            protocol,
            phase,
            version,
            outcome: match phase {
                DispatchPhase::Completed => Some(AttemptOutcome::Completed),
                DispatchPhase::Cancelled => Some(AttemptOutcome::Cancelled),
                _ => None,
            },
            error_code: None,
        });
        observation
    }

    fn send(&self, observation: AskObservation) {
        let _ = self.sender.send(observation);
    }

    fn wait_cancel(&self) -> bool {
        let deadline = Instant::now() + WAIT;
        while !self.cancel.load(Ordering::Acquire)
            && !self.stop.load(Ordering::Acquire)
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        if self.cancel.load(Ordering::Acquire) {
            self.cancellations.fetch_add(1, Ordering::AcqRel);
            true
        } else {
            false
        }
    }

    fn run(self, program: Program) {
        match program {
            Program::Complete => self.send(self.snapshot(
                DispatchPhase::Completed,
                4,
                PRIVATE_REPLY,
                AskOutputState::Complete,
            )),
            Program::CompletedUnavailable => {
                let mut observation =
                    self.snapshot(DispatchPhase::Completed, 4, "", AskOutputState::Unavailable);
                observation.daemon_restarted = true;
                observation.observer_error = Some("harness_dispatch_output_unavailable".into());
                self.send(observation);
            }
            Program::Uncertain => {
                let mut observation = AskObservation::pending(self.request_id);
                observation.submission = AskSubmissionState::Uncertain;
                observation.observer = AskObserverState::Failed;
                observation.observer_error = Some("control_deadline_exceeded".into());
                self.send(observation);
            }
            Program::RunningThenLost => {
                self.send(self.snapshot(
                    DispatchPhase::Running,
                    2,
                    PRIVATE_REPLY,
                    AskOutputState::Partial,
                ));
                let mut observation =
                    self.snapshot(DispatchPhase::Running, 2, "", AskOutputState::Failed);
                observation.observer = AskObserverState::Failed;
                observation.observer_error = Some("control_endpoint_unavailable".into());
                self.send(observation);
            }
            Program::WaitCancel(outcome) => {
                self.send(self.snapshot(
                    DispatchPhase::Running,
                    2,
                    "stream",
                    AskOutputState::Partial,
                ));
                if self.wait_cancel() {
                    let mut cancelling = self.snapshot(
                        DispatchPhase::CancelRequested,
                        3,
                        "stream",
                        AskOutputState::Partial,
                    );
                    cancelling.cancellation_requested = true;
                    self.send(cancelling);
                    self.send(self.snapshot(outcome, 4, PRIVATE_REPLY, AskOutputState::Complete));
                }
            }
            Program::CancelBeforeAdmission => {
                if self.wait_cancel() {
                    let mut observation = AskObservation::pending(self.request_id);
                    observation.submission = AskSubmissionState::NotSubmitted;
                    observation.observer = AskObserverState::Stopped;
                    observation.cancellation_requested = true;
                    observation.update.end = Some(AskEnd::Cancelled);
                    self.send(observation);
                }
            }
            Program::Burst(count) => {
                for index in 0..count {
                    if self.stop.load(Ordering::Acquire) {
                        return;
                    }
                    self.send(self.snapshot(
                        DispatchPhase::Running,
                        index as u64 + 2,
                        &format!("cumulative {index}"),
                        AskOutputState::Partial,
                    ));
                }
                self.send(self.snapshot(
                    DispatchPhase::Completed,
                    count as u64 + 2,
                    PRIVATE_REPLY,
                    AskOutputState::Complete,
                ));
            }
            Program::StaleSnapshot => {
                self.send(self.snapshot(
                    DispatchPhase::Running,
                    3,
                    "newer",
                    AskOutputState::Partial,
                ));
                self.send(self.snapshot(
                    DispatchPhase::Running,
                    2,
                    "stale text must never replace newer",
                    AskOutputState::Partial,
                ));
                self.send(self.snapshot(
                    DispatchPhase::Completed,
                    4,
                    PRIVATE_REPLY,
                    AskOutputState::Complete,
                ));
            }
            Program::ReplacingPartial(gate) => {
                self.send(self.snapshot(
                    DispatchPhase::Running,
                    2,
                    "a preliminary draft which is longer",
                    AskOutputState::Partial,
                ));
                self.send(self.snapshot(
                    DispatchPhase::Running,
                    3,
                    "short",
                    AskOutputState::Partial,
                ));
                gate.enter_and_wait();
                self.send(self.snapshot(
                    DispatchPhase::Completed,
                    4,
                    PRIVATE_REPLY,
                    AskOutputState::Complete,
                ));
            }
            Program::FinalOnStop => {
                self.send(self.snapshot(
                    DispatchPhase::Running,
                    2,
                    "stream",
                    AskOutputState::Partial,
                ));
                let deadline = Instant::now() + WAIT;
                while !self.stop.load(Ordering::Acquire) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(1));
                }
                // A capacity-one queue cannot fit both after the collector
                // leaves its receive loop. Closing the receiver must unblock.
                self.send(self.snapshot(
                    DispatchPhase::Running,
                    3,
                    "last partial",
                    AskOutputState::Partial,
                ));
                self.send(self.snapshot(
                    DispatchPhase::Completed,
                    4,
                    PRIVATE_REPLY,
                    AskOutputState::Complete,
                ));
            }
            Program::WrongIdentity => {
                let mut observation = self.snapshot(
                    DispatchPhase::Completed,
                    4,
                    PRIVATE_REPLY,
                    AskOutputState::Complete,
                );
                observation.update.request_id = Uuid::new_v4();
                self.send(observation);
            }
            Program::RecoveryPending => {
                let mut observation = self.snapshot(
                    DispatchPhase::RecoveryPending,
                    4,
                    "",
                    AskOutputState::Unavailable,
                );
                observation.observer = AskObserverState::Failed;
                observation.observer_error = Some("dispatch_recovery_pending".into());
                observation.daemon_restarted = true;
                self.send(observation);
            }
        }
    }
}

pub struct Fixture {
    pub state: Arc<AppState>,
    pub services: Arc<FakeServices>,
    pub spawner: Arc<FakeSpawner>,
    pub sink: Arc<EventProbe>,
    pub project_key: String,
    pub directory: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary project");
        let services = Arc::new(FakeServices::default());
        let spawner = Arc::new(FakeSpawner::default());
        let sink = Arc::new(EventProbe::default());
        let state = Arc::new(AppState::with_services(
            directory.path().join("desktop_settings.json"),
            sink.clone(),
            services.clone(),
            spawner.clone(),
        ));
        let project_key = state
            .register_project(directory.path())
            .expect("register temporary project");
        Self {
            state,
            services,
            spawner,
            sink,
            project_key,
            directory,
        }
    }

    pub fn consent(&self) {
        self.state
            .ask_confirm(&self.project_key)
            .expect("project consent");
    }

    pub fn submit(&self, submission_key: Uuid) -> Result<AskReceipt, CommandError> {
        self.state.ask_submit(
            &self.project_key,
            AskHarness::Claude,
            PRIVATE_PROMPT.into(),
            submission_key,
        )
    }

    pub fn completed(&self, key: Uuid) -> AskReceipt {
        let receipt = self.submit(key).expect("local receipt");
        self.sink
            .wait_phase(&receipt.ask_id, LocalAskPhase::Finished);
        receipt
    }

    /// Retry of an existing submission is an observation-only action. A
    /// collector may still be returning when its final invalidation arrives.
    pub fn reconcile(&self, key: Uuid) -> AskReceipt {
        let deadline = Instant::now() + WAIT;
        loop {
            let receipt = self.submit(key).expect("existing receipt");
            if self.spawner.observations() > 0 {
                return receipt;
            }
            assert!(
                Instant::now() < deadline,
                "retry did not attach an observer"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.state
            .asks
            .shutdown_with_limits(Duration::ZERO, Duration::from_secs(1));
    }
}
