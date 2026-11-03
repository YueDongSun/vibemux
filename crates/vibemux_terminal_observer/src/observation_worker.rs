//! One observer-owned blocking slot, independent of a request's lifetime.
use std::{sync::Arc, time::Duration};
use tokio::{sync::Semaphore, time::timeout};

const BLOCKING_OBSERVATION_SLOTS: usize = 1;

#[derive(Clone)]
pub struct ObservationWorker {
    slots: Arc<Semaphore>,
}

impl Default for ObservationWorker {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(BLOCKING_OBSERVATION_SLOTS)),
        }
    }
}

impl ObservationWorker {
    pub fn is_busy(&self) -> bool {
        self.slots.available_permits() == 0
    }

    pub async fn run<T, F>(&self, job: F) -> Result<T, &'static str>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, &'static str> + Send + 'static,
    {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "terminal_cleanup_busy")?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            job()
        })
        .await
        .map_err(|_| "terminal_cleanup_incomplete")?
    }

    /// Call after request admission has stopped. A timeout is not quiescence.
    pub async fn wait_idle(&self, deadline: Duration) -> Result<(), &'static str> {
        let _permit = timeout(deadline, self.slots.clone().acquire_owned())
            .await
            .map_err(|_| "terminal_cleanup_busy")?
            .map_err(|_| "terminal_cleanup_incomplete")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn request_timeout_and_cancel_cannot_admit_another_job_before_actual_completion() {
        for timed_out in [true, false] {
            let worker = ObservationWorker::default();
            let owner = worker.clone();
            let (entered_tx, entered_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel();
            let request = tokio::spawn(async move {
                let operation = owner.run(move || {
                    let _ = entered_tx.send(());
                    let _ = release_rx.blocking_recv();
                    Ok(())
                });
                if timed_out {
                    assert!(timeout(Duration::from_millis(20), operation).await.is_err());
                } else {
                    operation.await.expect("held operation");
                }
            });
            timeout(Duration::from_secs(2), entered_rx)
                .await
                .expect("blocking job started")
                .expect("entered");
            if timed_out {
                request.await.expect("request timed out");
            } else {
                request.abort();
                assert!(request.await.expect_err("cancelled request").is_cancelled());
            }
            let calls = Arc::new(AtomicUsize::new(0));
            let next_calls = calls.clone();
            let second = worker
                .run(move || {
                    next_calls.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                })
                .await;
            let shutdown = worker.wait_idle(Duration::from_millis(20)).await;
            let _ = release_tx.send(());
            worker
                .wait_idle(Duration::from_secs(2))
                .await
                .expect("actual job completed");
            assert_eq!(second, Err("terminal_cleanup_busy"));
            assert_eq!(calls.load(Ordering::Acquire), 0);
            assert_eq!(shutdown, Err("terminal_cleanup_busy"));
            assert_eq!(worker.run(|| Ok(42)).await, Ok(42));
        }
    }
}
