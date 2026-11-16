//! ADR 075 Scope A production wiring (slice 4): one actor owns the real
//! `Supervisor`; the SDK host reads through a bounded command channel, a
//! watch signal delivers cancellation with an immediate unload, a periodic
//! tick keeps the backend snapshot and tool-change events fresh, and the
//! finalizer sends the one shutdown command and joins the actor.
//!
//! Feature-gated (`rmcp`); nothing here runs on the hand-written default.

use std::{future::Future, pin::Pin, sync::Arc};

use serde_json::Value;
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::{Instant, sleep_until},
};

use crate::{
    sdk_driver::{Admission, BackendSnapshot, MetadataExecutor, SdkHost, WireState},
    supervisor::Supervisor,
    tools::ReadOperation,
};

/// Bounded command capacity for one producer call plus control commands.
pub const COMMAND_QUEUE: usize = 8;
/// Bounded capacity of the session-event channel.
pub const EVENT_QUEUE: usize = 8;

enum Command {
    Read(ReadOperation, oneshot::Sender<Result<Value, &'static str>>),
    Shutdown(oneshot::Sender<Result<(), &'static str>>),
}

/// The executor handle the SDK host reads through.
pub struct SupervisorExecutor {
    commands: mpsc::Sender<Command>,
    cancel: watch::Sender<u64>,
}

impl Clone for SupervisorExecutor {
    fn clone(&self) -> Self {
        Self {
            commands: self.commands.clone(),
            cancel: self.cancel.clone(),
        }
    }
}

impl SupervisorExecutor {
    /// Signals cancellation; the actor unloads the reader immediately.
    pub fn cancel(&self) {
        let next = self.cancel.borrow().wrapping_add(1);
        let _ = self.cancel.send(next);
    }
}

impl MetadataExecutor for SupervisorExecutor {
    fn read<'a>(
        &'a self,
        operation: ReadOperation,
    ) -> Pin<Box<dyn Future<Output = Result<Value, &'static str>> + Send + 'a>> {
        let commands = self.commands.clone();
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            commands
                .send(Command::Read(operation, reply))
                .await
                .map_err(|_| "executor_closed")?;
            response.await.map_err(|_| "request_cancelled")?
        })
    }
}

/// Everything one SDK session needs from the production adapter.
pub struct Wiring {
    pub host: SdkHost<SupervisorExecutor>,
    pub executor: SupervisorExecutor,
    pub events: mpsc::Receiver<&'static str>,
    pub finalize: Pin<Box<dyn Future<Output = Result<(), &'static str>> + Send>>,
}

fn snapshot(supervisor: &Supervisor) -> BackendSnapshot {
    let view = supervisor.view();
    BackendSnapshot {
        available: view.available,
        state: view.state,
        launches: view.launches,
        restarts: view.restarts,
        last_error: view.last_error,
    }
}

/// Starts the actor and returns the host, the cancellation handle, the
/// event channel and the finalizer the session passes to `run_session`.
pub fn start(config: crate::config::HostConfig, supervisor: Supervisor) -> Wiring {
    let state = Arc::new(WireState::new());
    let admission = Admission::new();
    let (commands, command_rx) = mpsc::channel(COMMAND_QUEUE);
    let (cancel, cancel_rx) = watch::channel(0_u64);
    let (events, event_rx) = mpsc::channel(EVENT_QUEUE);
    let executor = SupervisorExecutor {
        commands: commands.clone(),
        cancel,
    };
    let actor_state = Arc::clone(&state);
    let actor = tokio::spawn(run_actor(
        supervisor,
        config,
        command_rx,
        cancel_rx,
        events,
        actor_state,
    ));
    let host = SdkHost::new(state, admission, executor.clone());
    let finalize = Box::pin(shutdown(commands, actor));
    Wiring {
        host,
        executor,
        events: event_rx,
        finalize,
    }
}

async fn shutdown(
    commands: mpsc::Sender<Command>,
    actor: JoinHandle<()>,
) -> Result<(), &'static str> {
    let (reply, response) = oneshot::channel();
    let sent = commands.send(Command::Shutdown(reply)).await.is_ok();
    let result = if sent {
        response.await.unwrap_or(Err("executor_closed"))
    } else {
        Ok(())
    };
    // The actor exits on shutdown or channel closure; a join error is a
    // fatal internal fault, not a clean end.
    actor.await.map_err(|_| "executor_task_failed")?;
    result
}

async fn run_actor(
    mut supervisor: Supervisor,
    _config: crate::config::HostConfig,
    mut commands: mpsc::Receiver<Command>,
    mut cancel: watch::Receiver<u64>,
    events: mpsc::Sender<&'static str>,
    state: Arc<WireState>,
) {
    state.set_backend(snapshot(&supervisor));
    enum Step {
        Command(Option<Command>),
        Lifecycle(Result<bool, &'static str>),
    }
    loop {
        // `Supervisor::lifecycle` stays pending while the worker is Ready
        // (exactly the hand-written loop's select branch); polling it here
        // concurrently with commands keeps idle faults, restarts and cleanup
        // progressing without ever blocking a read behind it.
        let step = tokio::select! {
            command = commands.recv() => Step::Command(command),
            result = supervisor.lifecycle() => Step::Lifecycle(result),
        };
        match step {
            Step::Lifecycle(result) => {
                if let Ok(changed) = result {
                    state.set_backend(snapshot(&supervisor));
                    if changed {
                        let _ = events.try_send("tools_changed");
                    }
                }
            }
            Step::Command(None) => break,
            Step::Command(Some(command)) => match command {
                Command::Shutdown(reply) => {
                    let result = supervisor.shutdown().await;
                    let _ = reply.send(result);
                    break;
                }
                Command::Read(operation, reply) => {
                    enum End {
                        Finished(Result<Value, &'static str>),
                        TimedOut,
                        Cancelled,
                    }
                    let end = {
                        let budget = supervisor.read_budget();
                        let read = supervisor.read(operation);
                        tokio::pin!(read);
                        tokio::select! {
                            biased;
                            result = &mut read => End::Finished(result),
                            _ = sleep_until(Instant::now() + budget) => End::TimedOut,
                            _ = cancel.changed() => End::Cancelled,
                        }
                    };
                    let outcome = match end {
                        End::Finished(result) => result,
                        End::TimedOut => {
                            supervisor.begin_unload("reader_timeout");
                            Err("reader_timeout")
                        }
                        End::Cancelled => {
                            supervisor.begin_unload("request_cancelled");
                            Err("request_cancelled")
                        }
                    };
                    state.set_backend(snapshot(&supervisor));
                    let _ = reply.send(outcome);
                }
            },
        }
    }
    state.set_backend(snapshot(&supervisor));
    // Every exit path keeps and joins the owner exactly once.
    let _ = supervisor.shutdown().await;
}
