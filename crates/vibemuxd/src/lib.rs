#![forbid(unsafe_code)]
//! Dedicated, bounded, single-owner SQLite writer core for the future daemon.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use vibemux_events::{EventDraft, EventEnvelope};
use vibemux_store::{CommitOutcome, STORE_SCHEMA_VERSION, SqliteStore};
use vibemux_types::{Run, Task};

pub const DEFAULT_WRITER_QUEUE_CAPACITY: usize = 64;
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
pub const WRITER_LOCK_SUFFIX: &str = "writer.lock";

pub mod control;
pub mod process;
pub mod recovery;

static LOCK_NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WriterHealth {
    pub healthy: bool,
    pub store_schema_version: u32,
    pub queue_capacity: usize,
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
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidCapacity => "writer_invalid_capacity",
            Self::LockHeld => "writer_lock_held",
            Self::LockIo => "writer_lock_io",
            Self::QueueFull => "writer_queue_full",
            Self::WorkerStopped => "writer_stopped",
            Self::ResponseTimeout => "writer_response_timeout",
            Self::Store { .. } => "writer_store_error",
            Self::ThreadTerminated => "writer_thread_terminated",
        }
    }
}

enum WriterRequest {
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

pub struct WriterWorker {
    sender: Option<SyncSender<WriterRequest>>,
    thread: Option<JoinHandle<()>>,
    response_timeout: Duration,
    queue_capacity: usize,
    lock_path: PathBuf,
}

impl WriterWorker {
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
        let thread = thread::Builder::new()
            .name("vibemux_writer".to_string())
            .spawn(move || {
                writer_loop(
                    &database_path,
                    queue_capacity,
                    receiver,
                    ready_sender,
                    lifecycle_locks,
                );
            })
            .map_err(|_| WriterError::ThreadTerminated)?;
        match ready_receiver.recv_timeout(response_timeout) {
            Ok(Ok(())) => Ok(Self {
                sender: Some(sender),
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
        sender.try_send(request).map_err(|error| match error {
            TrySendError::Full(_) => WriterError::QueueFull,
            TrySendError::Disconnected(_) => WriterError::WorkerStopped,
        })
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
        match request {
            WriterRequest::Health(response) => {
                let _ = response.send(Ok(WriterHealth {
                    healthy: true,
                    store_schema_version: STORE_SCHEMA_VERSION,
                    queue_capacity,
                }));
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
        };
        let encoded = serde_json::to_string(&health).expect("health JSON");
        assert_eq!(
            serde_json::from_str::<WriterHealth>(&encoded).expect("decode health"),
            health
        );
        assert!(!encoded.contains("path"));
    }
}
