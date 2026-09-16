#![forbid(unsafe_code)]
//! Dedicated, bounded, single-owner SQLite writer core for the future daemon.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use vibemux_events::{EventDraft, EventEnvelope};
use vibemux_harness::{HarnessConfigSnapshot, HarnessDetection, HarnessRow};
use vibemux_store::{
    A2aCommitOutcome, CommitOutcome, HarnessSnapshotOutcome, STORE_SCHEMA_VERSION, SqliteStore,
};
use vibemux_types::{
    ProjectId, Run, RunId, Task, TaskId,
    a2a::{A2aRunRecord, A2aRunStart, A2aRunUpdate},
};

/// Writer queue capacity: 64 absorbs burst commits from multiple CLI clients
/// without letting a stalled store accumulate unbounded memory. Overflow is
/// fail-fast (`QueueFull`), never silent growth, per the bounded-queue rule.
pub const DEFAULT_WRITER_QUEUE_CAPACITY: usize = 64;
/// Per-request response deadline: comfortably above worst-case local SQLite
/// commit latency (WAL, single-digit ms) so only a genuinely stuck writer
/// surfaces as `ResponseTimeout`. Callers retry idempotent reads; commits
/// surface the error instead of blind retry.
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
pub const WRITER_LOCK_SUFFIX: &str = "writer.lock";

pub mod control;
pub mod model_peer_process;
pub mod plugin_configuration;
pub mod plugin_registry;
pub mod process;
pub mod recovery;
pub mod supervisor_service;
pub mod supervisor_workflow;

static LOCK_NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WriterHealth {
    pub healthy: bool,
    pub store_schema_version: u32,
    pub queue_capacity: usize,
    /// Requests currently queued for the writer thread.
    #[serde(default)]
    pub queue_depth: usize,
    /// Peak queue depth since the last health observation.
    #[serde(default)]
    pub queue_high_watermark: usize,
    /// True when this snapshot was produced without a worker round trip
    /// because the queue was full (the request itself would have been
    /// rejected with `QueueFull`).
    #[serde(default)]
    pub queue_saturated: bool,
}

/// Atomic queue telemetry shared between the enqueue side and the writer
/// thread. `pending` counts requests handed to the channel but not yet
/// dequeued by the worker, so health snapshots report waiting requests only.
/// `high_watermark` tracks the largest `pending` observed at enqueue time
/// since the last health sample (the sampling request itself contributes a
/// floor of one); the worker resets it after answering a health request.
#[derive(Debug, Default)]
struct SharedWriterState {
    pending: AtomicUsize,
    high_watermark: AtomicUsize,
}

impl SharedWriterState {
    fn record_enqueue(&self) {
        let depth = self.pending.fetch_add(1, Ordering::AcqRel) + 1;
        self.high_watermark.fetch_max(depth, Ordering::AcqRel);
    }

    fn record_dequeue(&self) {
        // Rollback for a counted enqueue whose channel publish failed, and
        // the worker-side dequeue. Telemetry is advisory, so a clamped floor
        // beats an arithmetic panic when a request traveled an uncounted
        // path (internal test channels, Drop's shutdown).
        let _ = self
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_sub(1)
            });
    }

    fn record_processed(&self) {
        // Requests sent around the counted enqueue path (internal test
        // channels) leave nothing to decrement; telemetry is advisory, so a
        // clamped floor beats an arithmetic panic.
        self.record_dequeue();
    }

    fn snapshot(&self) -> (usize, usize) {
        (
            self.pending.load(Ordering::Acquire),
            self.high_watermark.load(Ordering::Acquire),
        )
    }

    fn reset_watermark(&self, to: usize) {
        self.high_watermark.store(to, Ordering::Release);
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum WriterError {
    #[error("writer queue capacity must be greater than zero")]
    InvalidCapacity,
    #[error("authoritative writer lock is already held")]
    LockHeld,
    #[error("authoritative writer lock could not be created")]
    LockIo,
    #[error("writer request queue is full")]
    QueueFull,
    #[error("writer worker is not running")]
    WorkerStopped,
    #[error("writer response exceeded its deadline")]
    ResponseTimeout,
    #[error("writer store operation failed: {code}")]
    Store { code: String },
    #[error("writer thread terminated unexpectedly")]
    ThreadTerminated,
}

impl WriterError {
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidCapacity => "writer_invalid_capacity",
            Self::LockHeld => "writer_lock_held",
            Self::LockIo => "writer_lock_io",
            Self::QueueFull => "writer_queue_full",
            Self::WorkerStopped => "writer_stopped",
            Self::ResponseTimeout => "writer_response_timeout",
            Self::Store { code } => code.as_str(),
            Self::ThreadTerminated => "writer_thread_terminated",
        }
    }
}

enum WriterRequest {
    StartA2aRun {
        start: Box<A2aRunStart>,
        response: mpsc::Sender<Result<A2aCommitOutcome, WriterError>>,
    },
    UpdateA2aRun {
        update: A2aRunUpdate,
        response: mpsc::Sender<Result<A2aCommitOutcome, WriterError>>,
    },
    A2aRun {
        run_id: RunId,
        response: mpsc::Sender<Result<Option<A2aRunRecord>, WriterError>>,
    },
    A2aRuns {
        task_id: TaskId,
        response: mpsc::Sender<Result<Vec<A2aRunRecord>, WriterError>>,
    },
    Health(mpsc::Sender<Result<WriterHealth, WriterError>>),
    CommitTask {
        task: Task,
        draft: EventDraft,
        response: mpsc::Sender<Result<CommitOutcome, WriterError>>,
    },
    CommitRun {
        run: Run,
        draft: EventDraft,
        response: mpsc::Sender<Result<CommitOutcome, WriterError>>,
    },
    Projection {
        entity_kind: String,
        entity_id: String,
        response: mpsc::Sender<Result<Option<Value>, WriterError>>,
    },
    Events(mpsc::Sender<Result<Vec<EventEnvelope>, WriterError>>),
    CommitHarnessSnapshot {
        config: HarnessConfigSnapshot,
        detections: std::collections::BTreeMap<String, HarnessDetection>,
        draft: EventDraft,
        checked_at: String,
        response: mpsc::Sender<Result<HarnessSnapshotOutcome, WriterError>>,
    },
    CommitHarnessSwitch {
        harness: String,
        draft: EventDraft,
        response: mpsc::Sender<Result<CommitOutcome, WriterError>>,
    },
    HarnessRows {
        cached: bool,
        detections: std::collections::BTreeMap<String, HarnessDetection>,
        response: mpsc::Sender<Result<Vec<HarnessRow>, WriterError>>,
    },
    HarnessConfig(mpsc::Sender<Result<HarnessConfigSnapshot, WriterError>>),
    Shutdown(mpsc::Sender<Result<(), WriterError>>),
    #[cfg(test)]
    HoldForBackpressureTest {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    },
}

struct LifecycleLock {
    path: PathBuf,
    nonce: String,
    _file: File,
}

impl LifecycleLock {
    fn acquire(database_path: &Path) -> Result<Self, WriterError> {
        let path = writer_lock_path_for_database(database_path);
        Self::acquire_path(&path)
    }

    fn acquire_path(path: &Path) -> Result<Self, WriterError> {
        let path = path.to_path_buf();
        let nonce = lock_nonce();
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(WriterError::LockHeld);
            }
            Err(_) => return Err(WriterError::LockIo),
        };
        if writeln!(file, "{nonce}").is_err() || file.sync_all().is_err() {
            let _ = std::fs::remove_file(&path);
            return Err(WriterError::LockIo);
        }
        Ok(Self {
            path,
            nonce,
            _file: file,
        })
    }
}

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        let matches = std::fs::read_to_string(&self.path)
            .ok()
            .is_some_and(|value| value.trim() == self.nonce);
        if matches {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Cloneable bounded request capability. It owns no writer thread, lock, or
/// database and deliberately cannot shut down the authoritative writer.
#[derive(Clone)]
pub struct WriterHandle {
    sender: Weak<SyncSender<WriterRequest>>,
    shared: Weak<SharedWriterState>,
    response_timeout: Duration,
    queue_capacity: usize,
}

impl WriterHandle {
    pub fn health(&self) -> Result<WriterHealth, WriterError> {
        let (response, receiver) = mpsc::channel();
        match self.enqueue(WriterRequest::Health(response)) {
            Ok(()) => receive(receiver, self.response_timeout),
            // A saturated queue must not blind health: answer without a
            // worker round trip. The channel is full, so depth equals
            // capacity; healthy reflects the thread consuming (it is alive,
            // merely slow) while queue_saturated flags the true condition.
            Err(WriterError::QueueFull) => Ok(WriterHealth {
                healthy: true,
                store_schema_version: STORE_SCHEMA_VERSION,
                queue_capacity: self.queue_capacity,
                queue_depth: self.queue_capacity,
                queue_high_watermark: self.queue_capacity,
                queue_saturated: true,
            }),
            Err(error) => Err(error),
        }
    }

    pub fn events(&self) -> Result<Vec<EventEnvelope>, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::Events(response))?;
        receive(receiver, self.response_timeout)
    }

    pub fn projection(
        &self,
        entity_kind: impl Into<String>,
        entity_id: impl Into<String>,
    ) -> Result<Option<Value>, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::Projection {
            entity_kind: entity_kind.into(),
            entity_id: entity_id.into(),
            response,
        })?;
        receive(receiver, self.response_timeout)
    }

    pub fn start_a2a_run(&self, start: A2aRunStart) -> Result<A2aCommitOutcome, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::StartA2aRun {
            start: Box::new(start),
            response,
        })?;
        receive(receiver, self.response_timeout)
    }

    pub fn update_a2a_run(&self, update: A2aRunUpdate) -> Result<A2aCommitOutcome, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::UpdateA2aRun { update, response })?;
        receive(receiver, self.response_timeout)
    }

    pub fn a2a_run(&self, run_id: RunId) -> Result<Option<A2aRunRecord>, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::A2aRun { run_id, response })?;
        receive(receiver, self.response_timeout)
    }

    pub fn a2a_runs(&self, task_id: TaskId) -> Result<Vec<A2aRunRecord>, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::A2aRuns { task_id, response })?;
        receive(receiver, self.response_timeout)
    }

    pub fn commit_harness_snapshot(
        &self,
        config: HarnessConfigSnapshot,
        detections: std::collections::BTreeMap<String, HarnessDetection>,
        draft: EventDraft,
        checked_at: String,
    ) -> Result<HarnessSnapshotOutcome, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::CommitHarnessSnapshot {
            config,
            detections,
            draft,
            checked_at,
            response,
        })?;
        receive(receiver, self.response_timeout)
    }

    pub fn commit_harness_switch(
        &self,
        harness: String,
        draft: EventDraft,
    ) -> Result<CommitOutcome, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::CommitHarnessSwitch {
            harness,
            draft,
            response,
        })?;
        receive(receiver, self.response_timeout)
    }

    pub fn harness_rows(
        &self,
        cached: bool,
        detections: std::collections::BTreeMap<String, HarnessDetection>,
    ) -> Result<Vec<HarnessRow>, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::HarnessRows {
            cached,
            detections,
            response,
        })?;
        receive(receiver, self.response_timeout)
    }

    pub fn harness_config(&self) -> Result<HarnessConfigSnapshot, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::HarnessConfig(response))?;
        receive(receiver, self.response_timeout)
    }

    pub fn project_id(&self) -> Result<Option<ProjectId>, WriterError> {
        let events = self.events()?;
        Ok(events.first().map(|event| event.project_id()))
    }

    fn enqueue(&self, request: WriterRequest) -> Result<(), WriterError> {
        let sender = self.sender.upgrade().ok_or(WriterError::WorkerStopped)?;
        // Count before publishing: a fast worker may dequeue and process the
        // request before record_enqueue would otherwise run, leaving phantom
        // depth that inflates queue_depth and the watermark. A failed publish
        // rolls the count back so rejection leaves telemetry unchanged.
        if let Some(shared) = self.shared.upgrade() {
            shared.record_enqueue();
        }
        sender.try_send(request).map_err(|error| {
            if let Some(shared) = self.shared.upgrade() {
                shared.record_dequeue();
            }
            match error {
                TrySendError::Full(_) => WriterError::QueueFull,
                TrySendError::Disconnected(_) => WriterError::WorkerStopped,
            }
        })?;
        Ok(())
    }
}
pub struct WriterWorker {
    sender: Option<Arc<SyncSender<WriterRequest>>>,
    shared: Arc<SharedWriterState>,
    thread: Option<JoinHandle<()>>,
    response_timeout: Duration,
    queue_capacity: usize,
    lock_path: PathBuf,
}

impl WriterWorker {
    pub fn handle(&self) -> Result<WriterHandle, WriterError> {
        Ok(WriterHandle {
            sender: Arc::downgrade(self.sender.as_ref().ok_or(WriterError::WorkerStopped)?),
            shared: Arc::downgrade(&self.shared),
            response_timeout: self.response_timeout,
            queue_capacity: self.queue_capacity,
        })
    }

    pub fn start_a2a_run(&self, start: A2aRunStart) -> Result<A2aCommitOutcome, WriterError> {
        self.handle()?.start_a2a_run(start)
    }
    pub fn update_a2a_run(&self, update: A2aRunUpdate) -> Result<A2aCommitOutcome, WriterError> {
        self.handle()?.update_a2a_run(update)
    }
    pub fn a2a_run(&self, run_id: RunId) -> Result<Option<A2aRunRecord>, WriterError> {
        self.handle()?.a2a_run(run_id)
    }
    pub fn a2a_runs(&self, task_id: TaskId) -> Result<Vec<A2aRunRecord>, WriterError> {
        self.handle()?.a2a_runs(task_id)
    }
    pub fn start(database_path: &Path) -> Result<Self, WriterError> {
        Self::start_with_config(
            database_path,
            DEFAULT_WRITER_QUEUE_CAPACITY,
            DEFAULT_RESPONSE_TIMEOUT,
        )
    }

    pub fn start_with_config(
        database_path: &Path,
        queue_capacity: usize,
        response_timeout: Duration,
    ) -> Result<Self, WriterError> {
        if queue_capacity == 0 {
            return Err(WriterError::InvalidCapacity);
        }
        let lifecycle_lock = LifecycleLock::acquire(database_path)?;
        Self::start_with_lifecycle_locks(
            database_path,
            queue_capacity,
            response_timeout,
            vec![lifecycle_lock],
        )
    }

    pub fn start_with_lock_path(
        database_path: &Path,
        lock_path: &Path,
    ) -> Result<Self, WriterError> {
        Self::start_with_config_and_lock_path(
            database_path,
            lock_path,
            DEFAULT_WRITER_QUEUE_CAPACITY,
            DEFAULT_RESPONSE_TIMEOUT,
        )
    }

    pub fn start_with_config_and_lock_path(
        database_path: &Path,
        lock_path: &Path,
        queue_capacity: usize,
        response_timeout: Duration,
    ) -> Result<Self, WriterError> {
        if queue_capacity == 0 {
            return Err(WriterError::InvalidCapacity);
        }
        let lifecycle_lock = LifecycleLock::acquire_path(lock_path)?;
        Self::start_with_lifecycle_locks(
            database_path,
            queue_capacity,
            response_timeout,
            vec![lifecycle_lock],
        )
    }

    pub fn start_with_compatibility_lock_path(
        database_path: &Path,
        primary_lock_path: &Path,
        compatibility_lock_path: &Path,
    ) -> Result<Self, WriterError> {
        let mut lifecycle_locks = Vec::with_capacity(2);
        if compatibility_lock_path != primary_lock_path {
            lifecycle_locks.push(LifecycleLock::acquire_path(compatibility_lock_path)?);
        }
        lifecycle_locks.push(LifecycleLock::acquire_path(primary_lock_path)?);
        Self::start_with_lifecycle_locks(
            database_path,
            DEFAULT_WRITER_QUEUE_CAPACITY,
            DEFAULT_RESPONSE_TIMEOUT,
            lifecycle_locks,
        )
    }

    fn start_with_lifecycle_locks(
        database_path: &Path,
        queue_capacity: usize,
        response_timeout: Duration,
        lifecycle_locks: Vec<LifecycleLock>,
    ) -> Result<Self, WriterError> {
        let lock_path = lifecycle_locks
            .last()
            .ok_or(WriterError::LockIo)?
            .path
            .clone();
        let database_path = database_path.to_path_buf();
        let (sender, receiver) = mpsc::sync_channel(queue_capacity);
        let (ready_sender, ready_receiver) = mpsc::channel();
        let shared = Arc::new(SharedWriterState::default());
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("vibemux_writer".to_string())
            .spawn(move || {
                writer_loop(
                    &database_path,
                    queue_capacity,
                    receiver,
                    ready_sender,
                    lifecycle_locks,
                    worker_shared,
                );
            })
            .map_err(|_| WriterError::ThreadTerminated)?;
        match ready_receiver.recv_timeout(response_timeout) {
            Ok(Ok(())) => Ok(Self {
                sender: Some(Arc::new(sender)),
                shared,
                thread: Some(thread),
                response_timeout,
                queue_capacity,
                lock_path,
            }),
            Ok(Err(error)) => {
                drop(sender);
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                drop(sender);
                let _ = thread.join();
                Err(WriterError::ResponseTimeout)
            }
        }
    }

    #[must_use]
    pub const fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    #[must_use]
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    pub fn health(&self) -> Result<WriterHealth, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::Health(response))?;
        receive(receiver, self.response_timeout)
    }

    pub fn commit_task(&self, task: Task, draft: EventDraft) -> Result<CommitOutcome, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::CommitTask {
            task,
            draft,
            response,
        })?;
        receive(receiver, self.response_timeout)
    }

    pub fn commit_run(&self, run: Run, draft: EventDraft) -> Result<CommitOutcome, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::CommitRun {
            run,
            draft,
            response,
        })?;
        receive(receiver, self.response_timeout)
    }

    pub fn commit_harness_snapshot(
        &self,
        config: HarnessConfigSnapshot,
        detections: std::collections::BTreeMap<String, HarnessDetection>,
        draft: EventDraft,
        checked_at: String,
    ) -> Result<HarnessSnapshotOutcome, WriterError> {
        self.handle()?
            .commit_harness_snapshot(config, detections, draft, checked_at)
    }

    pub fn commit_harness_switch(
        &self,
        harness: String,
        draft: EventDraft,
    ) -> Result<CommitOutcome, WriterError> {
        self.handle()?.commit_harness_switch(harness, draft)
    }

    pub fn harness_rows(
        &self,
        cached: bool,
        detections: std::collections::BTreeMap<String, HarnessDetection>,
    ) -> Result<Vec<HarnessRow>, WriterError> {
        self.handle()?.harness_rows(cached, detections)
    }

    pub fn harness_config(&self) -> Result<HarnessConfigSnapshot, WriterError> {
        self.handle()?.harness_config()
    }

    pub fn project_id(&self) -> Result<Option<ProjectId>, WriterError> {
        self.handle()?.project_id()
    }

    pub fn projection(
        &self,
        entity_kind: impl Into<String>,
        entity_id: impl Into<String>,
    ) -> Result<Option<Value>, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::Projection {
            entity_kind: entity_kind.into(),
            entity_id: entity_id.into(),
            response,
        })?;
        receive(receiver, self.response_timeout)
    }

    pub fn events(&self) -> Result<Vec<EventEnvelope>, WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::Events(response))?;
        receive(receiver, self.response_timeout)
    }

    pub fn shutdown(mut self) -> Result<(), WriterError> {
        let (response, receiver) = mpsc::channel();
        self.enqueue(WriterRequest::Shutdown(response))?;
        let result = receive(receiver, self.response_timeout);
        self.sender.take();
        let joined = self
            .thread
            .take()
            .ok_or(WriterError::ThreadTerminated)?
            .join()
            .map_err(|_| WriterError::ThreadTerminated);
        result?;
        joined
    }

    fn enqueue(&self, request: WriterRequest) -> Result<(), WriterError> {
        let sender = self.sender.as_ref().ok_or(WriterError::WorkerStopped)?;
        // Count before publishing (see WriterHandle::enqueue); a failed
        // publish rolls the count back.
        self.shared.record_enqueue();
        sender.try_send(request).map_err(|error| {
            self.shared.record_dequeue();
            match error {
                TrySendError::Full(_) => WriterError::QueueFull,
                TrySendError::Disconnected(_) => WriterError::WorkerStopped,
            }
        })?;
        Ok(())
    }
}

impl Drop for WriterWorker {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let (response, _receiver) = mpsc::channel();
            let _ = sender.try_send(WriterRequest::Shutdown(response));
        }
        self.thread.take();
    }
}

fn writer_loop(
    database_path: &Path,
    queue_capacity: usize,
    receiver: Receiver<WriterRequest>,
    ready_sender: mpsc::Sender<Result<(), WriterError>>,
    _lifecycle_locks: Vec<LifecycleLock>,
    shared: Arc<SharedWriterState>,
) {
    let mut store = match SqliteStore::open(database_path) {
        Ok(store) => store,
        Err(error) => {
            let _ = ready_sender.send(Err(store_error(error.code())));
            return;
        }
    };
    if ready_sender.send(Ok(())).is_err() {
        return;
    }
    while let Ok(request) = receiver.recv() {
        // A dequeued request leaves the waiting queue immediately, so health
        // snapshots report only requests still waiting, never the one being
        // processed.
        shared.record_processed();
        match request {
            WriterRequest::StartA2aRun { start, response } => {
                let result = store
                    .start_a2a_run(*start)
                    .map_err(|error| store_error(error.code()));
                let _ = response.send(result);
            }
            WriterRequest::UpdateA2aRun { update, response } => {
                let result = store
                    .update_a2a_run(update)
                    .map_err(|error| store_error(error.code()));
                let _ = response.send(result);
            }
            WriterRequest::A2aRun { run_id, response } => {
                let result = store
                    .a2a_run(run_id)
                    .map_err(|error| store_error(error.code()));
                let _ = response.send(result);
            }
            WriterRequest::A2aRuns { task_id, response } => {
                let result = store
                    .a2a_runs(task_id)
                    .map_err(|error| store_error(error.code()));
                let _ = response.send(result);
            }
            WriterRequest::Health(response) => {
                let (depth, watermark) = shared.snapshot();
                let _ = response.send(Ok(WriterHealth {
                    healthy: true,
                    store_schema_version: STORE_SCHEMA_VERSION,
                    queue_capacity,
                    queue_depth: depth,
                    queue_high_watermark: watermark,
                    queue_saturated: depth >= queue_capacity,
                }));
                // Restart the observation window from the current depth.
                shared.reset_watermark(depth);
            }
            WriterRequest::CommitTask {
                task,
                draft,
                response,
            } => {
                let result = store
                    .commit_task(&task, draft)
                    .map_err(|error| store_error(error.code()));
                let _ = response.send(result);
            }
            WriterRequest::CommitRun {
                run,
                draft,
                response,
            } => {
                let result = store
                    .commit_run(&run, draft)
                    .map_err(|error| store_error(error.code()));
                let _ = response.send(result);
            }
            WriterRequest::Projection {
                entity_kind,
                entity_id,
                response,
            } => {
                let result = store
                    .projection(&entity_kind, &entity_id)
                    .map_err(|error| store_error(error.code()));
                let _ = response.send(result);
            }
            WriterRequest::Events(response) => {
                let result = store.events().map_err(|error| store_error(error.code()));
                let _ = response.send(result);
            }
            WriterRequest::CommitHarnessSnapshot {
                config,
                detections,
                draft,
                checked_at,
                response,
            } => {
                let result = store
                    .commit_harness_snapshot(&config, draft, &detections, &checked_at)
                    .map_err(|error| store_error(error.code()));
                let _ = response.send(result);
            }
            WriterRequest::CommitHarnessSwitch {
                harness,
                draft,
                response,
            } => {
                let result = store
                    .commit_harness_switch(&harness, draft)
                    .map_err(|error| WriterError::Store {
                        code: error.code().to_string(),
                    });
                let _ = response.send(result);
            }
            WriterRequest::HarnessRows {
                cached,
                detections,
                response,
            } => {
                let result = (|| {
                    let registry = store
                        .harness_registry()
                        .map_err(|e| store_error(e.code()))?;
                    let config = store.harness_config().map_err(|e| store_error(e.code()))?;
                    // For a cached read the detection column comes from the
                    // persisted snapshot; for a live read the daemon injected
                    // fresh probe results.
                    let detections = if cached {
                        registry
                            .harnesses
                            .iter()
                            .map(|entry| {
                                (
                                    entry.name.clone(),
                                    HarnessDetection {
                                        detected: entry.state.detected,
                                        path: entry.state.path.clone(),
                                        launcher: None,
                                        version: None,
                                    },
                                )
                            })
                            .collect()
                    } else {
                        detections
                    };
                    Ok(vibemux_harness::build_rows(&registry, &config, &detections))
                })();
                let _ = response.send(result);
            }
            WriterRequest::HarnessConfig(response) => {
                let result = store
                    .harness_config()
                    .map_err(|error| store_error(error.code()));
                let _ = response.send(result);
            }
            WriterRequest::Shutdown(response) => {
                let _ = response.send(Ok(()));
                break;
            }
            #[cfg(test)]
            WriterRequest::HoldForBackpressureTest { entered, release } => {
                let _ = entered.send(());
                let _ = release.recv();
            }
        }
    }
}

fn receive<T>(
    receiver: mpsc::Receiver<Result<T, WriterError>>,
    deadline: Duration,
) -> Result<T, WriterError> {
    receiver
        .recv_timeout(deadline)
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => WriterError::ResponseTimeout,
            mpsc::RecvTimeoutError::Disconnected => WriterError::WorkerStopped,
        })?
}

fn store_error(code: &str) -> WriterError {
    WriterError::Store {
        code: code.to_string(),
    }
}

#[must_use]
pub fn writer_lock_path_for_database(database_path: &Path) -> PathBuf {
    let file_name = database_path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("vibemux.sqlite3");
    database_path.with_file_name(format!("{file_name}.{WRITER_LOCK_SUFFIX}"))
}

fn lock_nonce() -> String {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0_u128, |duration| duration.as_nanos());
    let counter = LOCK_NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{epoch}-{counter}", std::process::id())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use time::OffsetDateTime;
    use vibemux_events::{ActorName, EventPayload, EventType};
    use vibemux_types::{EventId, ProjectId, Task, TaskSpec};

    use super::*;

    fn draft(project_id: ProjectId, key: &str) -> EventDraft {
        EventDraft {
            event_id: EventId::new(),
            event_type: EventType::new("task_created").expect("event type"),
            project_id,
            task_id: None,
            run_id: None,
            causation_id: None,
            actor: ActorName::new("daemon").expect("actor"),
            timestamp: OffsetDateTime::now_utc(),
            idempotency_key: Some(key.to_string()),
            payload: EventPayload::new(json!({"source": "writer_test"})).expect("payload"),
        }
    }

    fn task(project_id: ProjectId) -> Task {
        Task::new(TaskSpec {
            project_id,
            title: "writer".to_string(),
            description: String::new(),
        })
        .expect("task")
    }

    #[test]
    fn nonowning_handles_serialize_a2a_commands_and_cannot_keep_writer_alive() {
        use vibemux_types::{
            RunSpec,
            a2a::{A2aRunAction, RunWorkspace},
        };
        let directory = tempfile::tempdir().expect("directory");
        let database = directory.path().join("state.sqlite3");
        let worker = WriterWorker::start(&database).expect("writer");
        let handle = worker.handle().expect("handle");
        let task = task(ProjectId::new());
        let run = Run::new(RunSpec {
            project_id: task.project_id(),
            task_id: task.task_id(),
            harness: "mock".to_string(),
            role: "worker".to_string(),
            protocol: "a2a".to_string(),
            base_commit: "a".repeat(40),
        })
        .expect("run");
        let request = A2aRunStart {
            task: task.clone(),
            run: run.clone(),
            peer_id: "worker_peer".to_string(),
            external_task_id: "external_task".to_string(),
            transport: "http_json".to_string(),
            protocol_version: "1.0".to_string(),
            workspace: RunWorkspace {
                path: "/vibemux_fixture/run".to_string(),
                branch: "codex/writer_fixture".to_string(),
                base_commit: "a".repeat(40),
                ownership_token: "owned_fixture".to_string(),
            },
            timestamp: time::OffsetDateTime::now_utc(),
            idempotency_key: "concurrent_create".to_string(),
        };
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let joins = (0..2)
            .map(|_| {
                let handle = handle.clone();
                let request = request.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    handle.start_a2a_run(request).expect("concurrent creation")
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let outcomes = joins
            .into_iter()
            .map(|join| join.join().expect("caller joined"))
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.duplicate).count(),
            1
        );
        assert_eq!(outcomes[0].record, outcomes[1].record);
        assert_eq!(handle.events().expect("events").len(), 1);
        assert_eq!(handle.a2a_runs(task.task_id()).expect("runs").len(), 1);
        let updated = handle
            .update_a2a_run(A2aRunUpdate {
                run_id: run.run_id(),
                expected_version: 1,
                timestamp: request.timestamp,
                idempotency_key: "started".to_string(),
                action: A2aRunAction::Start,
            })
            .expect("start");
        assert_eq!(updated.record.version, 2);
        worker.shutdown().expect("shutdown remains owned by worker");
        assert_eq!(handle.health(), Err(WriterError::WorkerStopped));
        assert!(!writer_lock_path_for_database(&database).exists());
        let restarted = WriterWorker::start(&database).expect("restart");
        let retained = restarted.handle().expect("retained handle");
        drop(restarted);
        assert_eq!(retained.health(), Err(WriterError::WorkerStopped));
        for _ in 0..100 {
            if !writer_lock_path_for_database(&database).exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("non-owning capability retained writer lock");
    }
    #[test]
    fn worker_owns_store_and_commits_projection() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = directory.path().join("state.sqlite3");
        let worker = WriterWorker::start(&database).expect("start worker");
        let health = worker.health().expect("health");
        assert!(health.healthy);
        assert_eq!(health.store_schema_version, STORE_SCHEMA_VERSION);
        let project_id = ProjectId::new();
        let task = task(project_id);
        let outcome = worker
            .commit_task(task.clone(), draft(project_id, "writer_task_1"))
            .expect("commit task");
        assert_eq!(outcome.event.sequence().get(), 1);
        assert_eq!(worker.events().expect("events").len(), 1);
        assert_eq!(
            worker
                .projection("task", task.task_id().to_string())
                .expect("projection")
                .expect("task projection")["status"],
            json!("open")
        );
        worker.shutdown().expect("shutdown");
        assert!(!writer_lock_path_for_database(&database).exists());
    }

    #[test]
    fn second_writer_is_refused() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = directory.path().join("state.sqlite3");
        let first = WriterWorker::start(&database).expect("first writer");
        assert!(matches!(
            WriterWorker::start(&database),
            Err(WriterError::LockHeld)
        ));
        first.shutdown().expect("shutdown first");
    }

    #[test]
    fn compatibility_lock_blocks_legacy_writer_and_both_locks_cleanup() {
        let temp = tempfile::tempdir().expect("temp directory");
        let database = temp.path().join("state.sqlite3");
        let primary_lock = temp.path().join("protected.writer.lock");
        let legacy_lock = writer_lock_path_for_database(&database);
        let worker = WriterWorker::start_with_compatibility_lock_path(
            &database,
            &primary_lock,
            &legacy_lock,
        )
        .expect("dual-lock writer");
        assert!(primary_lock.is_file());
        assert!(legacy_lock.is_file());
        assert!(matches!(
            WriterWorker::start(&database),
            Err(WriterError::LockHeld)
        ));
        worker.shutdown().expect("shutdown dual-lock writer");
        assert!(!primary_lock.exists());
        assert!(!legacy_lock.exists());
    }

    #[test]
    fn graceful_shutdown_allows_restart_and_replay() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = directory.path().join("state.sqlite3");
        let project_id = ProjectId::new();
        let task = task(project_id);
        let first = WriterWorker::start(&database).expect("first writer");
        first
            .commit_task(task, draft(project_id, "restart_task"))
            .expect("commit task");
        first.shutdown().expect("shutdown first");
        let second = WriterWorker::start(&database).expect("restart writer");
        assert_eq!(second.events().expect("replayed events").len(), 1);
        second.shutdown().expect("shutdown second");
    }

    #[test]
    fn zero_capacity_is_rejected_before_lock_creation() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = directory.path().join("state.sqlite3");
        assert!(matches!(
            WriterWorker::start_with_config(&database, 0, DEFAULT_RESPONSE_TIMEOUT),
            Err(WriterError::InvalidCapacity)
        ));
        assert!(!writer_lock_path_for_database(&database).exists());
    }

    #[test]
    fn full_queue_returns_explicit_backpressure() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = directory.path().join("state.sqlite3");
        let worker = WriterWorker::start_with_config(&database, 1, DEFAULT_RESPONSE_TIMEOUT)
            .expect("writer");
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        worker
            .enqueue(WriterRequest::HoldForBackpressureTest {
                entered: entered_sender,
                release: release_receiver,
            })
            .expect("enqueue barrier");
        entered_receiver
            .recv_timeout(DEFAULT_RESPONSE_TIMEOUT)
            .expect("worker entered barrier");
        let (first_response, first_receiver) = mpsc::channel();
        worker
            .enqueue(WriterRequest::Health(first_response))
            .expect("fill queue");
        let (second_response, _second_receiver) = mpsc::channel();
        assert_eq!(
            worker.enqueue(WriterRequest::Health(second_response)),
            Err(WriterError::QueueFull)
        );
        release_sender.send(()).expect("release worker");
        receive(first_receiver, DEFAULT_RESPONSE_TIMEOUT).expect("drain queued health");
        worker.shutdown().expect("shutdown");
    }

    #[test]
    fn health_reports_queue_depth_and_high_watermark() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = directory.path().join("state.sqlite3");
        let worker = WriterWorker::start_with_config(&database, 4, DEFAULT_RESPONSE_TIMEOUT)
            .expect("writer");
        let baseline = worker.health().expect("idle health");
        assert_eq!(baseline.queue_depth, 0);
        // The watermark floor of one is the sampling request itself.
        assert!(baseline.queue_high_watermark <= 1);
        assert!(!baseline.queue_saturated);

        // Hold the worker so two requests stay queued simultaneously; the
        // second health request then observes depth 2 and watermark 2.
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        worker
            .enqueue(WriterRequest::HoldForBackpressureTest {
                entered: entered_sender,
                release: release_receiver,
            })
            .expect("enqueue barrier");
        entered_receiver
            .recv_timeout(DEFAULT_RESPONSE_TIMEOUT)
            .expect("worker entered barrier");
        let (blocked_response, blocked_receiver) = mpsc::channel();
        worker
            .enqueue(WriterRequest::Events(blocked_response))
            .expect("queue events");
        // Release the barrier from another thread once the health request is
        // queued, so the worker drains everything and answers the health
        // request instead of deadlocking behind the barrier.
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            let _ = release_sender.send(());
        });
        let loaded = worker.health().expect("loaded health");
        assert_eq!(
            loaded.queue_depth, 0,
            "queue drained before health was answered"
        );
        assert_eq!(
            loaded.queue_high_watermark, 2,
            "events and health waited together behind the barrier"
        );
        assert!(!loaded.queue_saturated);
        receive(blocked_receiver, DEFAULT_RESPONSE_TIMEOUT).expect("drain events");
        releaser.join().expect("releaser");

        // The watermark window restarts after the loaded sample; the drained
        // sample itself contributes the documented floor of one.
        let drained = worker.health().expect("drained health");
        assert_eq!(drained.queue_depth, 0);
        assert!(drained.queue_high_watermark <= 1);
        worker.shutdown().expect("shutdown");
    }

    #[test]
    fn saturated_queue_still_answers_health_without_worker_round_trip() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = directory.path().join("state.sqlite3");
        let worker = WriterWorker::start_with_config(&database, 1, DEFAULT_RESPONSE_TIMEOUT)
            .expect("writer");
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        worker
            .enqueue(WriterRequest::HoldForBackpressureTest {
                entered: entered_sender,
                release: release_receiver,
            })
            .expect("enqueue barrier");
        entered_receiver
            .recv_timeout(DEFAULT_RESPONSE_TIMEOUT)
            .expect("worker entered barrier");
        let (fill_response, fill_receiver) = mpsc::channel();
        worker
            .enqueue(WriterRequest::Health(fill_response))
            .expect("fill queue");
        let (overflow_sender, _overflow_receiver) = mpsc::channel();
        assert_eq!(
            worker.enqueue(WriterRequest::Health(overflow_sender)),
            Err(WriterError::QueueFull)
        );
        // The handle-level health call must not fail while the queue is full.
        let snapshot = worker
            .handle()
            .expect("handle")
            .health()
            .expect("saturated health");
        assert!(snapshot.healthy);
        assert!(snapshot.queue_saturated);
        assert_eq!(snapshot.queue_depth, snapshot.queue_capacity);
        assert_eq!(snapshot.queue_high_watermark, snapshot.queue_capacity);
        release_sender.send(()).expect("release worker");
        receive(fill_receiver, DEFAULT_RESPONSE_TIMEOUT).expect("drain queued health");
        worker.shutdown().expect("shutdown");
    }

    #[test]
    fn enqueue_rejection_leaves_queue_telemetry_unchanged() {
        // Regression: a rejected enqueue must not leave phantom depth behind.
        // The enqueue is counted before the channel publish (a fast worker
        // could otherwise dequeue before the count), and a failed publish
        // rolls the count back, so after the barrier drains the queue reports
        // exactly empty.
        let directory = tempfile::tempdir().expect("tempdir");
        let database = directory.path().join("state.sqlite3");
        let worker = WriterWorker::start_with_config(&database, 1, DEFAULT_RESPONSE_TIMEOUT)
            .expect("writer");
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        worker
            .enqueue(WriterRequest::HoldForBackpressureTest {
                entered: entered_sender,
                release: release_receiver,
            })
            .expect("enqueue barrier");
        entered_receiver
            .recv_timeout(DEFAULT_RESPONSE_TIMEOUT)
            .expect("worker entered barrier");
        let (fill_response, fill_receiver) = mpsc::channel();
        worker
            .enqueue(WriterRequest::Health(fill_response))
            .expect("fill queue");
        for _ in 0..3 {
            let (overflow_sender, _overflow_receiver) = mpsc::channel();
            assert_eq!(
                worker.enqueue(WriterRequest::Health(overflow_sender)),
                Err(WriterError::QueueFull)
            );
        }
        release_sender.send(()).expect("release worker");
        receive(fill_receiver, DEFAULT_RESPONSE_TIMEOUT).expect("drain queued health");
        let drained = worker.health().expect("drained health");
        assert_eq!(
            drained.queue_depth, 0,
            "rejected enqueues must roll their count back"
        );
        // The barrier + one queued health + the sampling floor bound the
        // watermark; the three rejections must not have raised it.
        assert!(
            drained.queue_high_watermark <= 2,
            "phantom depth inflated the watermark: {}",
            drained.queue_high_watermark
        );
        worker.shutdown().expect("shutdown");
    }

    #[test]
    fn dropped_handle_eventually_releases_lock() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = directory.path().join("state.sqlite3");
        let worker = WriterWorker::start(&database).expect("writer");
        let lock_path = worker.lock_path().to_path_buf();
        drop(worker);
        for _ in 0..100 {
            if !lock_path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("writer lock was not released after handle drop");
    }

    #[test]
    fn health_json_contains_no_database_path() {
        let health = WriterHealth {
            healthy: true,
            store_schema_version: STORE_SCHEMA_VERSION,
            queue_capacity: DEFAULT_WRITER_QUEUE_CAPACITY,
            queue_depth: 3,
            queue_high_watermark: 7,
            queue_saturated: false,
        };
        let encoded = serde_json::to_string(&health).expect("health JSON");
        assert_eq!(
            serde_json::from_str::<WriterHealth>(&encoded).expect("decode health"),
            health
        );
        assert!(!encoded.contains("path"));
    }
}
