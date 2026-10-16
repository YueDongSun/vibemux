//! Foreground state orchestration with one owned launch or contained reader.

use crate::{
    config::{HostConfig, MAX_RESTARTS, READ_DEADLINE, RESTART_BACKOFF},
    protocol::BackendView,
    reader_owner::{PendingLaunch, ReaderProcess, WorkerLaunch},
    tools::ReadOperation,
};
use serde_json::Value;
use std::time::Duration;
use tokio::time::{Instant, sleep_until, timeout};

#[derive(Clone, Copy)]
enum Disposition {
    Recover,
    Final,
}

enum CleanupOwner {
    Launch(PendingLaunch),
    Child(Box<ReaderProcess>),
}

struct Cleanup {
    owner: CleanupOwner,
    disposition: Disposition,
}

enum State {
    Ready,
    Launching(PendingLaunch),
    Active(Box<ReaderProcess>),
    Cleaning(Cleanup),
    Backoff(Instant),
    Exhausted,
}

pub struct Supervisor {
    config: HostConfig,
    state: State,
    launches: u32,
    last_error: Option<&'static str>,
    launch: WorkerLaunch,
    read_deadline: Duration,
}

impl Supervisor {
    pub fn new(config: HostConfig) -> Self {
        Self {
            config,
            state: State::Ready,
            launches: 0,
            last_error: None,
            launch: WorkerLaunch::Native,
            read_deadline: READ_DEADLINE,
        }
    }

    #[cfg(feature = "test_helpers")]
    pub fn for_fixture(
        config: HostConfig,
        executable: std::path::PathBuf,
        mode: crate::fixture_support::FixtureMode,
        read_deadline: Duration,
    ) -> Self {
        Self {
            launch: WorkerLaunch::Fixture {
                executable,
                mode,
                control: Default::default(),
            },
            read_deadline,
            ..Self::new(config)
        }
    }

    #[cfg(feature = "test_helpers")]
    pub fn with_launch_control(mut self, control: crate::fixture_support::LaunchControl) -> Self {
        if let WorkerLaunch::Fixture {
            control: current, ..
        } = &mut self.launch
        {
            *current = control;
        }
        self
    }

    #[cfg(feature = "test_helpers")]
    pub fn owned_child_id(&self) -> Option<u32> {
        match &self.state {
            State::Active(worker)
            | State::Cleaning(Cleanup {
                owner: CleanupOwner::Child(worker),
                ..
            }) => worker.child.id(),
            _ => None,
        }
    }

    #[cfg(feature = "test_helpers")]
    pub fn terminal_observer(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        match &self.state {
            State::Active(worker)
            | State::Cleaning(Cleanup {
                owner: CleanupOwner::Child(worker),
                ..
            }) => Some(std::sync::Arc::clone(&worker.terminal_observed)),
            State::Launching(_)
            | State::Cleaning(Cleanup {
                owner: CleanupOwner::Launch(_),
                ..
            }) => match &self.launch {
                WorkerLaunch::Fixture { control, .. } => {
                    Some(std::sync::Arc::clone(&control.terminal_observed))
                }
                WorkerLaunch::Native => None,
            },
            _ => None,
        }
    }

    pub fn view(&self) -> BackendView {
        let (available, state) = match &self.state {
            State::Ready if self.launches == 0 => (true, "not_started"),
            State::Ready => (true, "ready"),
            State::Launching(_) => (true, "launching"),
            State::Active(_) => (true, "available"),
            State::Cleaning(_) => (false, "cleanup_pending"),
            State::Backoff(_) => (false, "recovering"),
            State::Exhausted => (false, "exhausted"),
        };
        BackendView {
            available,
            state,
            launches: self.launches,
            restarts: self.launches.saturating_sub(1),
            last_error: self.last_error,
        }
    }

    pub fn retry_deadline(&self) -> Option<Instant> {
        match self.state {
            State::Backoff(deadline) => Some(deadline),
            _ => None,
        }
    }

    pub fn read_budget(&self) -> Duration {
        self.read_deadline
    }

    pub fn finish_backoff(&mut self) -> bool {
        if self
            .retry_deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.state = State::Ready;
            return true;
        }
        false
    }

    pub async fn read(&mut self, operation: ReadOperation) -> Result<Value, &'static str> {
        if !self.config.allow_metadata_read {
            return Err("metadata_read_denied");
        }
        if !self.view().available {
            return Err("reader_unavailable");
        }
        timeout(self.read_deadline, self.read_owned(operation))
            .await
            .unwrap_or(Err("reader_timeout"))
    }

    async fn read_owned(&mut self, operation: ReadOperation) -> Result<Value, &'static str> {
        if matches!(self.state, State::Ready) {
            if self.launches > MAX_RESTARTS {
                self.state = State::Exhausted;
                return Err("reader_restart_exhausted");
            }
            self.launches += 1;
            self.state = State::Launching(PendingLaunch::start(
                u64::from(self.launches),
                self.launch.clone(),
            )?);
        }
        if let State::Launching(launch) = &mut self.state {
            let result = launch.complete().await;
            // Completion and installation share a synchronous poll: no child
            // exists solely inside an awaitable that select can discard.
            self.state = match result {
                Ok(worker) => State::Active(Box::new(worker)),
                Err(code) => {
                    self.state = State::Ready;
                    return Err(code);
                }
            };
        }
        let State::Active(worker) = &mut self.state else {
            return Err("reader_unavailable");
        };
        worker.read(&self.config, operation).await
    }

    /// Remove capability immediately; cleanup retains the exact owner.
    pub fn begin_unload(&mut self, code: &'static str) {
        if matches!(self.state, State::Cleaning(_)) {
            return;
        }
        self.last_error = Some(code);
        self.begin_cleanup(Disposition::Recover);
    }

    fn begin_cleanup(&mut self, disposition: Disposition) {
        let previous = std::mem::replace(&mut self.state, State::Exhausted);
        self.state = match previous {
            State::Launching(launch) => {
                launch.cancel();
                State::Cleaning(Cleanup {
                    owner: CleanupOwner::Launch(launch),
                    disposition,
                })
            }
            State::Active(mut worker) => {
                worker.begin_shutdown();
                State::Cleaning(Cleanup {
                    owner: CleanupOwner::Child(worker),
                    disposition,
                })
            }
            State::Cleaning(mut cleanup) => {
                if matches!(disposition, Disposition::Final) {
                    cleanup.disposition = Disposition::Final;
                }
                State::Cleaning(cleanup)
            }
            State::Exhausted => State::Exhausted,
            State::Ready | State::Backoff(_) => self.after_cleanup(disposition),
        };
    }

    fn after_cleanup(&self, disposition: Disposition) -> State {
        if matches!(disposition, Disposition::Final) || self.launches > MAX_RESTARTS {
            State::Exhausted
        } else {
            State::Backoff(Instant::now() + RESTART_BACKOFF)
        }
    }

    async fn finish_cleanup(&mut self) -> Result<(), &'static str> {
        loop {
            let State::Cleaning(cleanup) = &mut self.state else {
                return Ok(());
            };
            match &mut cleanup.owner {
                CleanupOwner::Launch(launch) => {
                    let result = launch.complete().await;
                    match result {
                        Ok(mut worker) => {
                            worker.begin_shutdown();
                            cleanup.owner = CleanupOwner::Child(Box::new(worker));
                        }
                        Err(_) => {
                            // The original timeout/cancel outcome is already
                            // fixed. A late spawn error cannot replace it.
                            let disposition = cleanup.disposition;
                            self.state = self.after_cleanup(disposition);
                            return Ok(());
                        }
                    }
                }
                CleanupOwner::Child(worker) => {
                    if worker.shutdown().await.is_err() {
                        cleanup.disposition = Disposition::Final;
                        self.last_error = Some("reader_cleanup_failed");
                        return Err("reader_cleanup_failed");
                    }
                    let disposition = cleanup.disposition;
                    self.state = self.after_cleanup(disposition);
                    return Ok(());
                }
            }
        }
    }

    /// Poll only lifecycle work; returned true means the tool set changed.
    pub async fn lifecycle(&mut self) -> Result<bool, &'static str> {
        match &mut self.state {
            State::Active(worker) => {
                let code = worker.idle_fault().await;
                self.begin_unload(code);
                Ok(true)
            }
            State::Cleaning(_) => {
                self.finish_cleanup().await?;
                Ok(false)
            }
            State::Backoff(deadline) => {
                sleep_until(*deadline).await;
                Ok(self.finish_backoff())
            }
            _ => std::future::pending().await,
        }
    }

    pub async fn unload(&mut self, code: &'static str) -> Result<(), &'static str> {
        self.begin_unload(code);
        self.finish_cleanup().await
    }

    pub async fn shutdown(&mut self) -> Result<(), &'static str> {
        self.begin_cleanup(Disposition::Final);
        self.finish_cleanup().await
    }
}
