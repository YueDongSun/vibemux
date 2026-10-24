//! Joins the window's Ask state with the Ask workers for the active folder:
//! it lists the daemon's routes when the daemon connects, runs the Ask the
//! user confirmed, forwards a cancel, and runs the one-click setup. Workers
//! report through channels; nothing here blocks the UI thread.

use std::{
    path::Path,
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError},
    },
};

use vibemux_harness::AgentKind;

use crate::{
    ask::{
        AskHandle, SetupOutcome, UNCONFIGURED,
        worker::{spawn_ask, spawn_catalog, spawn_setup},
    },
    gui::{AskAvailability, AskState, EguiNotifier},
};

/// The code shown when a worker thread could not start or stopped early.
const WORKER_UNAVAILABLE: &str = "frontend_worker_unavailable";
const WORKER_STOPPED: &str = "frontend_worker_stopped";

#[derive(Default)]
pub(crate) struct AskBridge {
    catalog: Option<Receiver<Result<Vec<AgentKind>, String>>>,
    running: Option<AskHandle>,
    setup: Option<Receiver<SetupOutcome>>,
    /// Whether the daemon was connected on the last frame.
    connected: bool,
}

impl AskBridge {
    /// Cancels the running Ask and forgets every worker, for a folder
    /// switch. A forgotten setup still finishes for its own folder.
    pub(crate) fn reset(&mut self) {
        if let Some(handle) = self.running.take() {
            handle.cancel();
        }
        *self = Self::default();
    }

    /// One frame: starts what the user asked for and folds in what the
    /// workers reported. `connected` is the daemon connection now.
    pub(crate) fn tick(
        &mut self,
        state: &mut AskState,
        folder: Option<&Path>,
        connected: bool,
        context: &egui::Context,
    ) {
        if connected && !self.connected {
            state.request_recheck();
        }
        if !connected && self.connected {
            self.catalog = None;
            state.set_availability(AskAvailability::Unknown);
        }
        self.connected = connected;
        self.start_requested(state, folder, context);
        if state.take_cancel()
            && let Some(handle) = self.running.as_ref()
        {
            handle.cancel();
        }
        self.poll_catalog(state);
        self.poll_ask(state);
        self.poll_setup(state);
    }

    fn start_requested(
        &mut self,
        state: &mut AskState,
        folder: Option<&Path>,
        context: &egui::Context,
    ) {
        let recheck = state.take_recheck();
        let launch = state.take_launch();
        let setup = state.take_setup_request();
        let Some(folder) = folder else {
            if launch.is_some() {
                state.fail_running("project_folder_missing");
            }
            if setup {
                state.finish_setup(SetupOutcome::Failed("Open a folder first.".to_string()));
            }
            return;
        };
        if recheck && self.connected {
            self.catalog = Some(spawn_catalog(
                folder.to_path_buf(),
                Arc::new(EguiNotifier(context.clone())),
            ));
            state.set_availability(AskAvailability::Checking);
        }
        if let Some((harness, prompt)) = launch {
            match spawn_ask(
                folder.to_path_buf(),
                harness,
                prompt,
                Arc::new(EguiNotifier(context.clone())),
            ) {
                Ok(handle) => self.running = Some(handle),
                Err(_) => state.fail_running(WORKER_UNAVAILABLE),
            }
        }
        if setup {
            self.setup = Some(spawn_setup(
                folder.to_path_buf(),
                Arc::new(EguiNotifier(context.clone())),
            ));
        }
    }

    fn poll_catalog(&mut self, state: &mut AskState) {
        let Some(receiver) = self.catalog.as_ref() else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WORKER_STOPPED.to_string()),
        };
        self.catalog = None;
        state.set_availability(match result {
            Ok(harnesses) => AskAvailability::Ready(harnesses),
            Err(code) if code == UNCONFIGURED => AskAvailability::NotConfigured,
            Err(code) => AskAvailability::Unavailable(code),
        });
    }

    fn poll_ask(&mut self, state: &mut AskState) {
        let Some(handle) = self.running.as_ref() else {
            return;
        };
        loop {
            match handle.updates.try_recv() {
                Ok(update) => {
                    let ended = update.end.is_some();
                    state.apply_update(update);
                    if ended {
                        self.running = None;
                        return;
                    }
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    state.fail_running(WORKER_STOPPED);
                    self.running = None;
                    return;
                }
            }
        }
    }

    fn poll_setup(&mut self, state: &mut AskState) {
        let Some(receiver) = self.setup.as_ref() else {
            return;
        };
        let outcome = match receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                SetupOutcome::Failed("The setup worker stopped unexpectedly.".to_string())
            }
        };
        self.setup = None;
        let done = matches!(outcome, SetupOutcome::Done { .. });
        state.finish_setup(outcome);
        if done {
            // The restarted daemon has the new routes.
            state.request_recheck();
        }
    }
}
