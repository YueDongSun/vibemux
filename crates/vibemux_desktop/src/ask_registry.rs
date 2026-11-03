//! App-owned one-turn Asks, replay protection and continuously drained snapshots.
//!
//! Only the shared client submits. Display eviction never erases a submission
//! binding, and observer failure never releases uncertain execution ownership.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::RecvTimeoutError,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};
use uuid::Uuid;
use vibemux_client::{
    ask::{
        observation::{AskObservation, AskObserverState, AskOutputState, AskSubmissionState},
        worker::{ObservedAskHandle, spawn_ask_with_id, spawn_observer},
    },
    change_notifier::NoopNotifier,
};
use vibemux_harness::{
    AgentKind,
    dispatch::{
        DispatchPhase, DispatchRequest,
        request::{DISPATCH_REQUEST_SCHEMA_VERSION, MAX_PROMPT_BYTES},
    },
};
use vibemuxd::control::{
    CONTROL_PROTOCOL_VERSION, ControlOperation, ControlRequest, MAX_CONTROL_FRAME_BYTES,
};

use crate::{
    command_error::CommandError,
    event_sink::EventSink,
    project_registry::{RegisteredProject, failure},
    project_status::safe_code,
    ui_contract::{
        AskHarness, AskReceipt, AskReply, AskStatus, CanonicalAsk, LocalAskPhase, ObserverState,
        OutputState, UiEvent,
    },
};

pub const MAX_DISPLAY_ASKS: usize = 32;
pub const MAX_SUBMISSION_BINDINGS: usize = 1024;
pub const SHUTDOWN_RECONCILE: Duration = Duration::from_secs(3);
pub const SHUTDOWN_JOIN: Duration = Duration::from_secs(5);
const COLLECT_INTERVAL: Duration = Duration::from_millis(25);

pub trait AskSpawner: Send + Sync {
    fn spawn(
        &self,
        project_root: PathBuf,
        harness: AgentKind,
        prompt: String,
        request_id: Uuid,
    ) -> io::Result<ObservedAskHandle>;
    fn observe(
        &self,
        project_root: PathBuf,
        harness: AgentKind,
        request_id: Uuid,
    ) -> io::Result<ObservedAskHandle>;
}

pub struct SharedAskSpawner;
impl AskSpawner for SharedAskSpawner {
    fn spawn(
        &self,
        root: PathBuf,
        harness: AgentKind,
        prompt: String,
        id: Uuid,
    ) -> io::Result<ObservedAskHandle> {
        spawn_ask_with_id(root, harness, prompt, id, Arc::new(NoopNotifier))
    }
    fn observe(
        &self,
        root: PathBuf,
        harness: AgentKind,
        id: Uuid,
    ) -> io::Result<ObservedAskHandle> {
        spawn_observer(root, harness, id, Arc::new(NoopNotifier))
    }
}

struct DisplaySnapshot {
    status: AskStatus,
    text: AskReply,
    settled: bool,
    canonical_version: Option<u64>,
}

struct AskEntry {
    project: Arc<RegisteredProject>,
    request_id: Uuid,
    receipt: AskReceipt,
    snapshot: Mutex<DisplaySnapshot>,
    cancel: AtomicBool,
    stop: AtomicBool,
    collecting: AtomicBool,
}

struct SubmissionBinding {
    project_key: String,
    harness: AskHarness,
    prompt_fingerprint: [u8; 32],
    receipt: AskReceipt,
}

#[derive(Default)]
struct RegistryData {
    submissions: BTreeMap<Uuid, SubmissionBinding>,
    display: VecDeque<Arc<AskEntry>>,
    collectors: Vec<JoinHandle<()>>,
}

pub struct AskRegistry {
    data: Mutex<RegistryData>,
    spawner: Arc<dyn AskSpawner>,
    sink: Arc<dyn EventSink>,
    stopping: AtomicBool,
}

impl AskRegistry {
    pub fn new(spawner: Arc<dyn AskSpawner>, sink: Arc<dyn EventSink>) -> Self {
        Self {
            data: Mutex::new(RegistryData::default()),
            spawner,
            sink,
            stopping: AtomicBool::new(false),
        }
    }

    /// A repeat is resolved before current route availability is checked. A lost
    /// response must not turn into a new send when the daemon becomes unreachable.
    pub fn replay(
        &self,
        project_key: &str,
        harness: AskHarness,
        prompt: &str,
        key: Uuid,
    ) -> Result<Option<AskReceipt>, CommandError> {
        let (receipt, entry) = {
            let data = self.data.lock().map_err(|_| registry_failed())?;
            let Some(binding) = data.submissions.get(&key) else {
                return Ok(None);
            };
            if binding.project_key != project_key
                || binding.harness != harness
                || binding.prompt_fingerprint != fingerprint(prompt)
            {
                return Err(failure(
                    "ask_submission_conflict",
                    "This submission key is already bound to different content or a different target.",
                ));
            }
            let mut receipt = binding.receipt.clone();
            receipt.duplicate = true;
            let entry = data
                .display
                .iter()
                .find(|entry| entry.receipt.ask_id == receipt.ask_id)
                .cloned();
            (receipt, entry)
        };
        if let Some(entry) = entry {
            let settled = entry
                .snapshot
                .lock()
                .map_err(|_| registry_failed())?
                .settled;
            if !settled && !self.stopping.load(Ordering::Acquire) {
                self.start_collector(entry, true)?;
            }
        }
        Ok(Some(receipt))
    }

    pub fn submit(
        &self,
        project: Arc<RegisteredProject>,
        harness: AskHarness,
        prompt: String,
        key: Uuid,
    ) -> Result<AskReceipt, CommandError> {
        validate_submission(harness, &prompt, key)?;
        if let Some(receipt) = self.replay(&project.key, harness, &prompt, key)? {
            return Ok(receipt);
        }
        let request_id = Uuid::new_v4();
        let receipt = AskReceipt {
            ask_id: Uuid::new_v4().to_string(),
            request_id: request_id.to_string(),
            submission_key: key.to_string(),
            duplicate: false,
        };
        let entry = Arc::new(AskEntry {
            project: Arc::clone(&project),
            request_id,
            receipt: receipt.clone(),
            snapshot: Mutex::new(DisplaySnapshot {
                status: AskStatus {
                    ask_id: receipt.ask_id.clone(),
                    project_key: project.key.clone(),
                    harness,
                    revision: 1,
                    phase: LocalAskPhase::Submitting,
                    canonical: None,
                    terminal_observed: false,
                    observer_state: ObserverState::Observing,
                    observer_error: None,
                    output_state: OutputState::Pending,
                    reply_available: false,
                    delivery_complete: false,
                    cancel_requested: false,
                },
                text: AskReply {
                    ask_id: receipt.ask_id.clone(),
                    revision: 1,
                    prompt: prompt.clone(),
                    reply: String::new(),
                    truncated: false,
                    delivery_complete: false,
                },
                settled: false,
                canonical_version: None,
            }),
            // Reserve the initial submitter before publishing the binding.
            // A racing duplicate/read may not replace it with an observer.
            cancel: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            collecting: AtomicBool::new(true),
        });
        {
            let mut data = self.data.lock().map_err(|_| registry_failed())?;
            if self.stopping.load(Ordering::Acquire) {
                return Err(failure(
                    "desktop_shutting_down",
                    "The app is closing; no Ask was submitted.",
                ));
            }
            // Recheck inside the reservation transaction for independent callers.
            if let Some(binding) = data.submissions.get(&key) {
                if binding.project_key != project.key
                    || binding.harness != harness
                    || binding.prompt_fingerprint != fingerprint(&prompt)
                {
                    return Err(failure(
                        "ask_submission_conflict",
                        "The submission key has a different binding.",
                    ));
                }
                let mut existing = binding.receipt.clone();
                existing.duplicate = true;
                return Ok(existing);
            }
            if data.submissions.len() >= MAX_SUBMISSION_BINDINGS {
                return Err(failure(
                    "ask_dedup_capacity",
                    "This app session reached its submission budget. No replay protection was discarded.",
                ));
            }
            if has_active(&data, &project.key) {
                return Err(failure(
                    "ask_busy",
                    "This project already has an active or unresolved Ask.",
                ));
            }
            while data
                .display
                .iter()
                .filter(|entry| entry.project.key == project.key)
                .count()
                >= MAX_DISPLAY_ASKS
            {
                let oldest = data
                    .display
                    .iter()
                    .position(|entry| entry.project.key == project.key && evictable(entry));
                let Some(oldest) = oldest else {
                    return Err(failure(
                        "ask_history_busy",
                        "Active or unresolved display entries cannot be evicted.",
                    ));
                };
                data.display.remove(oldest);
            }
            data.submissions.insert(
                key,
                SubmissionBinding {
                    project_key: project.key.clone(),
                    harness,
                    prompt_fingerprint: fingerprint(&prompt),
                    receipt: receipt.clone(),
                },
            );
            data.display.push_back(Arc::clone(&entry));
        }
        // The binding and recoverable prompt exist before any worker can submit.
        if let Err(error) = self.start_collector(Arc::clone(&entry), false) {
            mark_local_failure(&entry, &self.sink, "ask_worker_unavailable", true);
            return Err(error);
        }
        self.sink.emit(UiEvent::ProjectsChanged);
        Ok(receipt)
    }

    fn start_collector(
        &self,
        entry: Arc<AskEntry>,
        observe_only: bool,
    ) -> Result<bool, CommandError> {
        self.reap_collectors()?;
        if observe_only
            && entry
                .collecting
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Ok(false);
        }
        let spawner = Arc::clone(&self.spawner);
        let sink = Arc::clone(&self.sink);
        let collector_entry = Arc::clone(&entry);
        let worker = std::thread::Builder::new()
            .name("desktop_ask_collector".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    collect(&collector_entry, spawner.as_ref(), &sink, observe_only);
                }));
                if result.is_err() {
                    mark_local_failure(&collector_entry, &sink, "ask_collector_failed", false);
                }
                collector_entry.collecting.store(false, Ordering::Release);
            });
        match worker {
            Ok(worker) => {
                self.data
                    .lock()
                    .map_err(|_| registry_failed())?
                    .collectors
                    .push(worker);
                Ok(true)
            }
            Err(_) => {
                entry.collecting.store(false, Ordering::Release);
                Err(failure(
                    "ask_worker_unavailable",
                    "The Ask collector could not start. The prompt remains recoverable.",
                ))
            }
        }
    }

    fn reap_collectors(&self) -> Result<(), CommandError> {
        let finished = {
            let mut data = self.data.lock().map_err(|_| registry_failed())?;
            let (finished, running): (Vec<_>, Vec<_>) = std::mem::take(&mut data.collectors)
                .into_iter()
                .partition(JoinHandle::is_finished);
            data.collectors = running;
            finished
        };
        for collector in finished {
            let _ = collector.join();
        }
        Ok(())
    }

    pub fn has_active(&self, project_key: &str) -> bool {
        self.data
            .lock()
            .map(|data| has_active(&data, project_key))
            .unwrap_or(true)
    }

    pub fn ids(&self, project_key: &str) -> Vec<String> {
        self.data
            .lock()
            .map(|data| {
                data.display
                    .iter()
                    .filter(|entry| entry.project.key == project_key)
                    .map(|entry| entry.receipt.ask_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn entry(&self, ask_id: &str) -> Result<Arc<AskEntry>, CommandError> {
        if Uuid::parse_str(ask_id).ok().is_none_or(|id| id.is_nil()) {
            return Err(failure(
                "invalid_identifier",
                "A valid Ask identifier is required.",
            ));
        }
        let data = self.data.lock().map_err(|_| registry_failed())?;
        data.display.iter().find(|entry| entry.receipt.ask_id == ask_id).cloned()
            .ok_or_else(|| failure("ask_not_available", "This Ask is unknown or its settled display entry was evicted. It will not be replayed."))
    }

    pub fn status(&self, ask_id: &str) -> Result<AskStatus, CommandError> {
        let entry = self.entry(ask_id)?;
        let result = entry
            .snapshot
            .lock()
            .map_err(|_| registry_failed())?
            .status
            .clone();
        Ok(result)
    }

    /// One observation-only attach on webview recovery or an explicit refresh.
    /// Periodic cached reads do not create an unbounded automatic retry loop.
    pub fn reconcile(&self, ask_id: &str) -> Result<(), CommandError> {
        let entry = self.entry(ask_id)?;
        if self.stopping.load(Ordering::Acquire) {
            return Ok(());
        }
        entry.project.resolve()?;
        let snapshot = entry.snapshot.lock().map_err(|_| registry_failed())?;
        let needs_observer = !snapshot.settled
            || (snapshot.status.canonical.is_some() && !snapshot.status.delivery_complete);
        drop(snapshot);
        if needs_observer {
            self.start_collector(entry, true)?;
        }
        Ok(())
    }

    pub fn reply(&self, ask_id: &str) -> Result<AskReply, CommandError> {
        let entry = self.entry(ask_id)?;
        let result = entry
            .snapshot
            .lock()
            .map_err(|_| registry_failed())?
            .text
            .clone();
        Ok(result)
    }

    pub fn cancel(&self, ask_id: &str) -> Result<(), CommandError> {
        let entry = self.entry(ask_id)?;
        entry.project.resolve()?;
        {
            let mut snapshot = entry.snapshot.lock().map_err(|_| registry_failed())?;
            if snapshot
                .status
                .canonical
                .as_ref()
                .is_some_and(|canonical| canonical.phase == DispatchPhase::RecoveryPending)
            {
                return Err(failure(
                    "ask_recovery_pending",
                    "This attempt requires an explicit daemon recovery decision outside this Ask view.",
                ));
            }
            if snapshot.settled {
                return Ok(());
            }
            entry.cancel.store(true, Ordering::Release);
            snapshot.status.cancel_requested = true;
            snapshot.status.phase = LocalAskPhase::Cancelling;
            advance_revision(&mut snapshot);
        }
        emit_entry(&entry, &self.sink);
        self.start_collector(entry, true).map(|_| ())
    }

    pub fn forget_display(&self, project_key: &str) -> Result<(), CommandError> {
        let mut data = self.data.lock().map_err(|_| registry_failed())?;
        if has_active(&data, project_key) {
            return Err(failure(
                "ask_busy",
                "An active or unresolved Ask still owns this project.",
            ));
        }
        data.display
            .retain(|entry| entry.project.key != project_key);
        Ok(())
    }

    /// Bounded app shutdown; the daemon continues to own reservation/deadline/
    /// process containment even when a final status cannot be reconciled.
    pub fn shutdown(&self) -> usize {
        self.shutdown_with_limits(SHUTDOWN_RECONCILE, SHUTDOWN_JOIN)
    }

    pub fn shutdown_with_limits(&self, reconcile: Duration, join_budget: Duration) -> usize {
        if self.stopping.swap(true, Ordering::AcqRel) {
            return self.unresolved_count();
        }
        let entries = self
            .data
            .lock()
            .map(|data| data.display.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut attached = BTreeSet::new();
        let deadline = Instant::now() + reconcile;
        loop {
            for entry in &entries {
                let can_cancel = entry
                    .snapshot
                    .lock()
                    .map(|snapshot| {
                        !snapshot.settled
                            && !snapshot.status.canonical.as_ref().is_some_and(|canonical| {
                                canonical.phase == DispatchPhase::RecoveryPending
                            })
                    })
                    .unwrap_or(false);
                if can_cancel {
                    entry.cancel.store(true, Ordering::Release);
                    // A finishing collector may clear its flag during this
                    // bounded period. Attach once after ownership is released.
                    if !attached.contains(&entry.receipt.ask_id)
                        && self
                            .start_collector(Arc::clone(entry), true)
                            .unwrap_or(false)
                    {
                        attached.insert(entry.receipt.ask_id.clone());
                    }
                }
            }
            if self.unresolved_count() == 0 || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(COLLECT_INTERVAL);
        }
        for entry in &entries {
            entry.stop.store(true, Ordering::Release);
        }
        let deadline = Instant::now() + join_budget;
        loop {
            let (finished, pending) = {
                let mut data = match self.data.lock() {
                    Ok(data) => data,
                    Err(_) => return entries.len(),
                };
                let (finished, pending): (Vec<_>, Vec<_>) = std::mem::take(&mut data.collectors)
                    .into_iter()
                    .partition(JoinHandle::is_finished);
                let count = pending.len();
                data.collectors = pending;
                (finished, count)
            };
            for worker in finished {
                let _ = worker.join();
            }
            if pending == 0 || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(COLLECT_INTERVAL);
        }
        for entry in &entries {
            if entry
                .snapshot
                .lock()
                .map(|snapshot| !snapshot.settled)
                .unwrap_or(true)
            {
                mark_local_failure(entry, &self.sink, "ask_shutdown_unreconciled", false);
            }
        }
        self.unresolved_count()
    }

    fn unresolved_count(&self) -> usize {
        self.data
            .lock()
            .map(|data| {
                data.display
                    .iter()
                    .filter(|entry| {
                        entry
                            .snapshot
                            .lock()
                            .map(|snapshot| !snapshot.settled)
                            .unwrap_or(true)
                    })
                    .count()
            })
            .unwrap_or(1)
    }
}

fn has_active(data: &RegistryData, project_key: &str) -> bool {
    data.display.iter().any(|entry| {
        entry.project.key == project_key
            && entry
                .snapshot
                .lock()
                .map(|snapshot| !snapshot.settled)
                .unwrap_or(true)
    })
}
fn evictable(entry: &AskEntry) -> bool {
    !entry.collecting.load(Ordering::Acquire)
        && entry
            .snapshot
            .lock()
            .map(|snapshot| snapshot.settled)
            .unwrap_or(false)
}
fn fingerprint(prompt: &str) -> [u8; 32] {
    Sha256::digest(prompt.as_bytes()).into()
}
fn registry_failed() -> CommandError {
    failure(
        "ask_registry_unavailable",
        "The Ask registry is unavailable; no replacement attempt was started.",
    )
}

pub fn validate_submission(
    harness: AskHarness,
    prompt: &str,
    key: Uuid,
) -> Result<(), CommandError> {
    if key.is_nil() {
        return Err(failure(
            "invalid_identifier",
            "A non-nil submission identifier is required.",
        ));
    }
    if prompt.trim().is_empty() {
        return Err(failure("prompt_empty", "Enter a text prompt."));
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        return Err(failure(
            "prompt_too_large",
            "The prompt exceeds the 32 KiB UTF-8 limit.",
        ));
    }
    // The argument is JSON inside the Control envelope's JSON string. Measure
    // both escaping passes using the actual public wire types and fixed-length
    // placeholder IDs/token; no credential is read or exposed for validation.
    let request = DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id: key,
        harness: harness.agent(),
        prompt: prompt.to_owned(),
    };
    let argument = serde_json::to_string(&request).map_err(|_| registry_failed())?;
    let envelope = ControlRequest {
        version: CONTROL_PROTOCOL_VERSION,
        request_id: key.to_string(),
        token: "0".repeat(64),
        operation: ControlOperation::HarnessDispatchSubmit,
        argument: Some(argument),
    };
    if serde_json::to_vec(&envelope)
        .map_err(|_| registry_failed())?
        .len()
        > MAX_CONTROL_FRAME_BYTES
    {
        return Err(failure(
            "prompt_frame_too_large",
            "The escaped prompt exceeds the authenticated IPC frame limit.",
        ));
    }
    Ok(())
}

fn collect(
    entry: &Arc<AskEntry>,
    spawner: &dyn AskSpawner,
    sink: &Arc<dyn EventSink>,
    observe_only: bool,
) {
    let paths = match entry.project.resolve() {
        Ok(paths) => paths,
        Err(_) => {
            mark_local_failure(entry, sink, "project_identity_changed", !observe_only);
            return;
        }
    };
    let (harness, prompt) = match entry.snapshot.lock() {
        Ok(snapshot) => (snapshot.status.harness, snapshot.text.prompt.clone()),
        Err(_) => return,
    };
    if entry.stop.load(Ordering::Acquire) || (!observe_only && entry.cancel.load(Ordering::Acquire))
    {
        mark_local_failure(entry, sink, "ask_not_submitted", !observe_only);
        return;
    }
    let handle = if observe_only {
        spawner.observe(
            paths.project_root().to_owned(),
            harness.agent(),
            entry.request_id,
        )
    } else {
        spawner.spawn(
            paths.project_root().to_owned(),
            harness.agent(),
            prompt,
            entry.request_id,
        )
    };
    let handle = match handle {
        Ok(handle) => handle,
        Err(_) => {
            mark_local_failure(entry, sink, "ask_worker_unavailable", !observe_only);
            return;
        }
    };
    loop {
        if entry.cancel.load(Ordering::Acquire) {
            handle.cancel();
        }
        if entry.stop.load(Ordering::Acquire) {
            handle.control().stop_observing();
            break;
        }
        match handle.updates.recv_timeout(COLLECT_INTERVAL) {
            Ok(observation) => apply_observation(entry, sink, observation),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    // join closes the receiver before waiting, including terminal-send races.
    let panicked = handle.join().is_err();
    let needs_failure = entry
        .snapshot
        .lock()
        .map(|snapshot| snapshot.status.observer_state == ObserverState::Observing)
        .unwrap_or(false);
    if panicked || needs_failure {
        mark_local_failure(
            entry,
            sink,
            if panicked {
                "ask_worker_failed"
            } else {
                "ask_observer_stopped"
            },
            false,
        );
    }
}

fn apply_observation(entry: &AskEntry, sink: &Arc<dyn EventSink>, observation: AskObservation) {
    if observation.update.request_id != entry.request_id {
        mark_local_failure(entry, sink, "ask_identity_mismatch", false);
        return;
    }
    {
        let Ok(mut snapshot) = entry.snapshot.lock() else {
            return;
        };
        if let Some(status) = &observation.canonical {
            if status.request_id != entry.request_id
                || status.harness != snapshot.status.harness.agent()
            {
                drop(snapshot);
                mark_local_failure(entry, sink, "ask_identity_mismatch", false);
                return;
            }
            if snapshot.status.canonical.as_ref().is_some_and(|known| {
                known.task_id != status.task_id.to_string()
                    || known.run_id != status.run_id.to_string()
            }) {
                drop(snapshot);
                mark_local_failure(entry, sink, "ask_identity_mismatch", false);
                return;
            }
            if snapshot
                .canonical_version
                .is_some_and(|version| status.version < version)
            {
                return;
            }
            snapshot.canonical_version = Some(status.version);
            snapshot.status.canonical = Some(CanonicalAsk {
                request_id: status.request_id.to_string(),
                task_id: status.task_id.to_string(),
                run_id: status.run_id.to_string(),
                protocol: status.protocol,
                phase: status.phase,
                sequence: observation
                    .admission
                    .as_ref()
                    .map(|admission| admission.sequence)
                    .or_else(|| {
                        snapshot
                            .status
                            .canonical
                            .as_ref()
                            .and_then(|known| known.sequence)
                    }),
                error_code: status.error_code.as_deref().map(safe_code),
            });
        } else if snapshot.status.canonical.is_none()
            && let Some(admission) = &observation.admission
        {
            if admission.request_id != entry.request_id
                || admission.harness != snapshot.status.harness.agent()
            {
                drop(snapshot);
                mark_local_failure(entry, sink, "ask_identity_mismatch", false);
                return;
            }
            snapshot.status.canonical = Some(CanonicalAsk {
                request_id: admission.request_id.to_string(),
                task_id: admission.task_id.to_string(),
                run_id: admission.run_id.to_string(),
                protocol: admission.protocol,
                phase: admission.phase,
                sequence: Some(admission.sequence),
                error_code: None,
            });
        }
        snapshot.status.terminal_observed = snapshot
            .status
            .canonical
            .as_ref()
            .is_some_and(|canonical| canonical.phase.is_terminal());
        let not_submitted = matches!(
            observation.submission,
            AskSubmissionState::Rejected | AskSubmissionState::NotSubmitted
        );
        snapshot.settled = snapshot.status.terminal_observed || not_submitted;
        snapshot.status.observer_state = match observation.observer {
            AskObserverState::Submitting | AskObserverState::Observing => ObserverState::Observing,
            AskObserverState::Failed => ObserverState::Failed,
            AskObserverState::Finished | AskObserverState::Stopped => ObserverState::Stopped,
        };
        snapshot.status.observer_error = observation.observer_error.as_deref().map(safe_code);
        snapshot.status.output_state = match observation.output {
            AskOutputState::Pending => OutputState::Pending,
            AskOutputState::Partial => OutputState::Partial,
            AskOutputState::Complete => OutputState::Complete,
            AskOutputState::Unavailable => OutputState::Unavailable,
            AskOutputState::Failed => OutputState::Failed,
        };
        snapshot.status.cancel_requested =
            entry.cancel.load(Ordering::Acquire) || observation.cancellation_requested;
        snapshot.status.phase = if snapshot.status.terminal_observed {
            LocalAskPhase::Finished
        } else if not_submitted {
            LocalAskPhase::Rejected
        } else if matches!(observation.submission, AskSubmissionState::Uncertain)
            || snapshot
                .status
                .canonical
                .as_ref()
                .is_some_and(|canonical| canonical.phase == DispatchPhase::RecoveryPending)
            || observation.observer_finished()
        {
            LocalAskPhase::Uncertain
        } else if snapshot.status.cancel_requested {
            LocalAskPhase::Cancelling
        } else if snapshot.status.canonical.is_some() {
            LocalAskPhase::Running
        } else {
            LocalAskPhase::Submitting
        };
        // Snapshots replace text; cumulative vendor text is never appended here.
        if !observation.daemon_restarted
            && matches!(
                observation.output,
                AskOutputState::Complete | AskOutputState::Partial
            )
        {
            snapshot.text.reply = observation.update.reply;
            snapshot.text.truncated = observation.update.truncated;
        }
        snapshot.status.reply_available = !snapshot.text.reply.is_empty();
        snapshot.status.delivery_complete = observation.output == AskOutputState::Complete;
        snapshot.text.delivery_complete = snapshot.status.delivery_complete;
        advance_revision(&mut snapshot);
    }
    emit_entry(entry, sink);
}

fn mark_local_failure(
    entry: &AskEntry,
    sink: &Arc<dyn EventSink>,
    code: &str,
    not_submitted: bool,
) {
    if let Ok(mut snapshot) = entry.snapshot.lock() {
        snapshot.status.observer_state = ObserverState::Failed;
        snapshot.status.observer_error = Some(safe_code(code));
        if !snapshot.status.terminal_observed {
            snapshot.status.phase = if not_submitted {
                LocalAskPhase::Rejected
            } else {
                LocalAskPhase::Uncertain
            };
            snapshot.settled = not_submitted;
        }
        advance_revision(&mut snapshot);
    }
    emit_entry(entry, sink);
}

fn advance_revision(snapshot: &mut DisplaySnapshot) {
    snapshot.status.revision = snapshot.status.revision.saturating_add(1);
    snapshot.text.revision = snapshot.status.revision;
}

fn emit_entry(entry: &AskEntry, sink: &Arc<dyn EventSink>) {
    let event = entry
        .snapshot
        .lock()
        .ok()
        .map(|snapshot| UiEvent::AskChanged {
            ask_id: snapshot.status.ask_id.clone(),
            phase: snapshot.status.phase,
            revision: snapshot.status.revision,
        });
    if let Some(event) = event {
        sink.emit(event);
    }
}
