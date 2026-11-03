//! Bounded daemon ownership of workspace/terminal work beyond RPC waiters.

use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex, Weak},
};
use tokio::{
    sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, oneshot, watch},
    task::JoinSet,
};

const MAX_WORKSPACE_JOBS: usize = 8;
const MAX_RESOURCE_GATES: usize = 64;

struct JobsState {
    jobs: Option<JoinSet<()>>,
    accepting: bool,
    failed: bool,
    shutdown_result: Option<bool>,
}

/// One per daemon, shared by directory/terminal reads and the cleanup owner.
/// Accepted work belongs to this owner; its response waiter may disappear.
#[derive(Clone)]
pub(crate) struct WorkspaceJobs {
    state: Arc<Mutex<JobsState>>,
    gates: Arc<Mutex<BTreeMap<String, Weak<RwLock<()>>>>>,
    stopped: watch::Sender<bool>,
}

impl WorkspaceJobs {
    pub(crate) fn cancellation(&self) -> watch::Receiver<bool> {
        self.stopped.subscribe()
    }
    pub(crate) fn new() -> Self {
        let (stopped, _) = watch::channel(false);
        Self {
            state: Arc::new(Mutex::new(JobsState {
                jobs: Some(JoinSet::new()),
                accepting: true,
                failed: false,
                shutdown_result: None,
            })),
            gates: Arc::new(Mutex::new(BTreeMap::new())),
            stopped,
        }
    }

    pub(crate) async fn execute<T, F>(&self, operation: F) -> Result<T, &'static str>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, &'static str>> + Send + 'static,
    {
        let (reply, result) = oneshot::channel();
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "workspace_operation_unavailable")?;
            if !state.accepting {
                return Err("workspace_operation_shutting_down");
            }
            let jobs = state
                .jobs
                .as_mut()
                .ok_or("workspace_operation_shutting_down")?;
            let mut failed = false;
            while let Some(result) = jobs.try_join_next() {
                failed |= result.is_err();
            }
            let full = jobs.len() >= MAX_WORKSPACE_JOBS;
            state.failed |= failed;
            if state.failed {
                return Err("workspace_operation_failed");
            }
            if full {
                return Err("workspace_operation_capacity");
            }
            state
                .jobs
                .as_mut()
                .ok_or("workspace_operation_shutting_down")?
                .spawn(async move {
                    let outcome = operation.await;
                    // A lost response does not cancel or replay the accepted job.
                    let _ = reply.send(outcome);
                });
        }
        result.await.map_err(|_| "workspace_operation_failed")?
    }

    fn gate(&self, key: &str) -> Result<Arc<RwLock<()>>, &'static str> {
        if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("workspace_resource_invalid");
        }
        let mut gates = self
            .gates
            .lock()
            .map_err(|_| "workspace_operation_unavailable")?;
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(key).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        if gates.len() >= MAX_RESOURCE_GATES {
            return Err("workspace_operation_capacity");
        }
        let gate = Arc::new(RwLock::new(()));
        gates.insert(key.to_owned(), Arc::downgrade(&gate));
        Ok(gate)
    }

    pub(crate) async fn read(&self, key: &str) -> Result<OwnedRwLockReadGuard<()>, &'static str> {
        let mut stopped = self.stopped.subscribe();
        if *stopped.borrow() {
            return Err("workspace_operation_shutting_down");
        }
        let gate = self.gate(key)?;
        tokio::select! {biased;
            _=stopped.changed()=>Err("workspace_operation_shutting_down"),
            guard=gate.read_owned()=>Ok(guard),
        }
    }

    pub(crate) async fn write(&self, key: &str) -> Result<OwnedRwLockWriteGuard<()>, &'static str> {
        let mut stopped = self.stopped.subscribe();
        if *stopped.borrow() {
            return Err("workspace_operation_shutting_down");
        }
        let gate = self.gate(key)?;
        tokio::select! {biased;
            _=stopped.changed()=>Err("workspace_operation_shutting_down"),
            guard=gate.write_owned()=>Ok(guard),
        }
    }

    pub(crate) fn begin_shutdown(&self) -> Result<(), &'static str> {
        self.stopped.send_replace(true);
        let mut state = self
            .state
            .lock()
            .map_err(|_| "workspace_operation_unavailable")?;
        state.accepting = false;
        Ok(())
    }

    pub(crate) async fn shutdown(&self) -> Result<(), &'static str> {
        self.begin_shutdown()?;
        let (mut jobs, mut failed) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "workspace_operation_unavailable")?;
            match state.jobs.take() {
                Some(jobs) => (jobs, state.failed),
                None => {
                    return match state.shutdown_result {
                        Some(false) => Ok(()),
                        Some(true) => Err("workspace_operation_failed"),
                        None => Err("workspace_operation_shutdown_pending"),
                    };
                }
            }
        };
        while let Some(result) = jobs.join_next().await {
            failed |= result.is_err();
        }
        self.state
            .lock()
            .map_err(|_| "workspace_operation_unavailable")?
            .shutdown_result = Some(failed);
        if failed {
            Err("workspace_operation_failed")
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_for_test(&self) -> usize {
        let mut state = self.state.lock().expect("test owner");
        let mut failed = false;
        let count = if let Some(jobs) = state.jobs.as_mut() {
            while let Some(result) = jobs.try_join_next() {
                failed |= result.is_err();
            }
            jobs.len()
        } else {
            0
        };
        state.failed |= failed;
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn dropping_a_reply_waiter_does_not_release_its_running_read_guard() {
        let jobs = WorkspaceJobs::new();
        let owner = jobs.clone();
        let work = jobs.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, blocked) = tokio::sync::oneshot::channel();
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let waiter = tokio::spawn(async move {
            owner
                .execute(async move {
                    let _guard = work.read(&"a".repeat(64)).await?;
                    calls.fetch_add(1, Ordering::SeqCst);
                    let _ = started.send(());
                    let _ = blocked.await;
                    Ok(())
                })
                .await
        });
        ready.await.expect("owned job started");
        waiter.abort();
        let _ = waiter.await;
        assert!(
            timeout(Duration::from_millis(30), jobs.write(&"a".repeat(64)))
                .await
                .is_err()
        );
        release.send(()).expect("release owned read");
        let exclusive = timeout(Duration::from_secs(2), jobs.write(&"a".repeat(64)))
            .await
            .expect("read eventually finishes")
            .expect("exclusive");
        drop(exclusive);
        jobs.shutdown().await.expect("owned jobs joined");
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "no automatic operation retry"
        );
    }

    #[tokio::test]
    async fn capacity_is_reserved_before_waiting_for_a_gate_and_shutdown_cancels_waiters() {
        let jobs = WorkspaceJobs::new();
        let exclusive = jobs.write(&"a".repeat(64)).await.expect("held write");
        let mut waiters = Vec::new();
        for _ in 0..MAX_WORKSPACE_JOBS {
            let owner = jobs.clone();
            let work = jobs.clone();
            waiters.push(tokio::spawn(async move {
                owner
                    .execute(async move {
                        let _guard = work.read(&"a".repeat(64)).await?;
                        Ok(())
                    })
                    .await
            }));
        }
        // Observe actual admission, without a sleep-based ordering assumption.
        loop {
            if jobs.pending_for_test() == MAX_WORKSPACE_JOBS {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            jobs.execute(async { Ok(()) }).await,
            Err("workspace_operation_capacity")
        );
        timeout(Duration::from_secs(2), jobs.shutdown())
            .await
            .expect("queued readers do not deadlock shutdown")
            .expect("joined");
        for waiter in waiters {
            assert_eq!(
                waiter.await.expect("waiter"),
                Err("workspace_operation_shutting_down")
            );
        }
        drop(exclusive);
        assert_eq!(
            jobs.execute(async { Ok(()) }).await,
            Err("workspace_operation_shutting_down")
        );
    }

    #[tokio::test]
    async fn unrelated_resources_can_proceed_and_active_work_is_joined_on_shutdown() {
        let jobs = WorkspaceJobs::new();
        let exclusive = jobs.write(&"a".repeat(64)).await.expect("write a");
        let unrelated = timeout(Duration::from_secs(1), jobs.read(&"b".repeat(64)))
            .await
            .expect("unrelated gate")
            .expect("read b");
        drop(unrelated);
        drop(exclusive);
        let owner = jobs.clone();
        let work = jobs.clone();
        let (release, blocked) = tokio::sync::oneshot::channel();
        let (started, ready) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move {
            owner
                .execute(async move {
                    let _guard = work.read(&"a".repeat(64)).await?;
                    let _ = started.send(());
                    let _ = blocked.await;
                    Ok(())
                })
                .await
        });
        ready.await.expect("running");
        let closer = jobs.clone();
        let mut shutdown = tokio::spawn(async move { closer.shutdown().await });
        assert!(
            timeout(Duration::from_millis(30), &mut shutdown)
                .await
                .is_err(),
            "running work is retained"
        );
        release.send(()).expect("release work");
        waiter.await.expect("waiter").expect("operation");
        shutdown.await.expect("shutdown task").expect("joined");
    }
}
