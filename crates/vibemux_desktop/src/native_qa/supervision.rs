//! One owned stop/deadline observer, cancelled and joined after the event loop.
use super::{
    QA_SCHEMA_VERSION, QaError,
    storage::{OwnedRoot, QaRun, STOP_FILE, StopRequest},
};
use std::{
    sync::{
        Arc,
        mpsc::{RecvTimeoutError, SyncSender, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
const POLL_INTERVAL: Duration = Duration::from_millis(100);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StopReason {
    WindowReturned,
    Requested,
    Deadline,
}
pub(super) struct Monitor {
    cancel: SyncSender<()>,
    task: JoinHandle<Result<StopReason, QaError>>,
}
impl Monitor {
    pub fn start(
        root: Arc<OwnedRoot>,
        run: QaRun,
        limit: Duration,
        verify_layout: impl Fn() -> Result<(), QaError> + Send + 'static,
        exit: impl FnOnce() + Send + 'static,
    ) -> Result<Self, QaError> {
        if limit.is_zero() || run.root_id != root.id() || run.run_id.is_nil() {
            return Err(QaError::InvalidArguments);
        }
        let (cancel, receiver) = sync_channel(1);
        let task = thread::Builder::new()
            .name("native_qa_stop".into())
            .spawn(move || {
                let deadline = Instant::now()
                    .checked_add(limit)
                    .ok_or(QaError::InvalidArguments)?;
                let reason = loop {
                    match receiver.recv_timeout(POLL_INTERVAL) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                            return Ok(StopReason::WindowReturned);
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    if root.verify().is_err() || verify_layout().is_err() {
                        break Err(QaError::OwnershipChanged);
                    }
                    match root.optional::<StopRequest>(STOP_FILE) {
                        Ok(Some(stop))
                            if stop.schema_version == QA_SCHEMA_VERSION
                                && stop.root_id == run.root_id
                                && stop.run_id == run.run_id =>
                        {
                            break Ok(StopReason::Requested);
                        }
                        Ok(Some(_)) | Err(_) => break Err(QaError::OwnershipChanged),
                        Ok(None) => {}
                    }
                    if Instant::now() >= deadline {
                        break Ok(StopReason::Deadline);
                    }
                };
                exit();
                reason
            })
            .map_err(|_| QaError::RuntimeUnavailable)?;
        Ok(Self { cancel, task })
    }
    pub fn finish(self) -> Result<StopReason, QaError> {
        let _ = self.cancel.try_send(());
        self.task.join().map_err(|_| QaError::CleanupUnconfirmed)?
    }
}
