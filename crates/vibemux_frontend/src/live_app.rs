//! Native application adapter joining presentation with the control reader.
//!
//! It owns the observation client for the active project folder, restarts
//! it when the user switches folders, runs the daemon starts and stops the
//! user asks for on a worker thread, and drives read-only Asks through the
//! Ask bridge, so the UI never blocks on them.
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
};

use crate::{
    daemon_control::{self, DaemonCommand, DaemonControlResult},
    gui::{DaemonJob, EguiNotifier, VibeMuxApp},
    live_ask::AskBridge,
    live_console::ConsoleBridge,
    supervisor_client::FrontendClient,
    supervisor_model::{ConnectionStatus, SupervisorAction, SupervisorSnapshot},
};

pub struct LiveApp {
    app: VibeMuxApp,
    client: Option<FrontendClient>,
    /// Shared handle to the last observation. Publishing moves this instead of
    /// deep-copying the snapshot, so a poll costs one atomic increment.
    snapshot: Arc<SupervisorSnapshot>,
    context: egui::Context,
    /// The result of the start or stop in flight, if any.
    daemon_result: Option<Receiver<DaemonControlResult>>,
    ask: AskBridge,
    console: ConsoleBridge,
}

impl LiveApp {
    pub fn new(mut app: VibeMuxApp, context: egui::Context) -> Self {
        let (client, snapshot) = connect(app.active_project().map(PathBuf::from), &context);
        app.replace_snapshot(Arc::clone(&snapshot));
        Self {
            app,
            client,
            snapshot,
            context,
            daemon_result: None,
            ask: AskBridge::default(),
            console: ConsoleBridge::default(),
        }
    }

    /// Replaces the observation client with one for `folder`.
    fn switch_folder(&mut self, folder: PathBuf) {
        // Dropping the old client cancels its reader before the new one starts.
        self.client = None;
        self.ask.reset();
        self.console.reset();
        let (client, snapshot) = connect(Some(folder), &self.context);
        self.client = client;
        self.snapshot = snapshot;
        self.app.replace_snapshot(Arc::clone(&self.snapshot));
    }

    fn start_daemon_job(&mut self, command: DaemonCommand) {
        let Some(folder) = self.app.active_project().map(PathBuf::from) else {
            self.app
                .set_daemon_job(DaemonJob::Failed("Open a folder first.".to_string()));
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let context = self.context.clone();
        let spawned = std::thread::Builder::new()
            .name("gui_daemon_control".into())
            .spawn(move || {
                let _ = sender.send(daemon_control::run(command, &folder));
                context.request_repaint();
            });
        match spawned {
            Ok(_) => self.daemon_result = Some(receiver),
            Err(_) => self.app.set_daemon_job(DaemonJob::Failed(
                "Could not start the daemon control worker.".to_string(),
            )),
        }
    }

    fn poll_daemon_job(&mut self) {
        let Some(receiver) = self.daemon_result.as_ref() else {
            return;
        };
        let outcome = match receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => DaemonControlResult::Failed {
                code: "daemon_control_worker_stopped".into(),
                message: "The daemon control worker stopped unexpectedly.".into(),
            },
        };
        self.daemon_result = None;
        match outcome {
            DaemonControlResult::Done { .. } => {
                self.app.set_daemon_job(DaemonJob::Idle);
                // Ask for a fresh observation instead of waiting for the next poll.
                if let Some(client) = self.client.as_ref() {
                    let _ = client.submit(SupervisorAction::Refresh);
                }
            }
            DaemonControlResult::Failed { message, .. } => {
                self.app.set_daemon_job(DaemonJob::Failed(message));
            }
        }
    }
}

/// An observation client for `folder`, and the snapshot to show until it
/// reports.
fn connect(
    folder: Option<PathBuf>,
    context: &egui::Context,
) -> (Option<FrontendClient>, Arc<SupervisorSnapshot>) {
    let mut snapshot = SupervisorSnapshot::unavailable();
    let Some(folder) = folder else {
        snapshot.connection.detail = Some("Open a folder to observe its tasks.".into());
        return (None, Arc::new(snapshot));
    };
    match FrontendClient::start(folder, Arc::new(EguiNotifier(context.clone()))) {
        Ok(client) => (Some(client), Arc::new(snapshot)),
        Err(_) => {
            snapshot.connection.detail =
                Some("Unable to start the workspace observation client.".into());
            (None, Arc::new(snapshot))
        }
    }
}

impl eframe::App for LiveApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if let Some(client) = self.client.as_mut()
            && let Some(snapshot) = client.take_snapshot()
        {
            self.snapshot = snapshot;
            self.app.replace_snapshot(Arc::clone(&self.snapshot));
        }
        self.poll_daemon_job();
        self.app.ui(ui, frame);
        if let Some(folder) = self.app.take_project_switch() {
            self.switch_folder(folder);
        }
        if let Some(command) = self.app.take_daemon_request() {
            self.start_daemon_job(command);
        }
        let connected = self.snapshot.connection.status == ConnectionStatus::Connected;
        let folder = self.app.active_project().map(PathBuf::from);
        self.ask.tick(
            self.app.ask_state(),
            folder.as_deref(),
            connected,
            &self.context,
        );
        let console_open = self.app.console_open();
        if let Some(view_model) = self.console.tick(
            self.app.console_data(),
            folder.as_deref(),
            console_open,
            &self.context,
        ) {
            self.app.set_view_model(view_model);
        }
        for action in self.app.take_supervisor_actions() {
            let result = self
                .client
                .as_ref()
                .ok_or("The workspace client is unavailable.")
                .and_then(|client| client.submit(action));
            if let Err(detail) = result {
                let snapshot = Arc::make_mut(&mut self.snapshot);
                snapshot.connection.detail = Some(detail.into());
                self.app.replace_snapshot(Arc::clone(&self.snapshot));
                ui.ctx().request_repaint();
            }
        }
    }
    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.app.on_exit(gl);
        // Cancels the reader's select branch before joining; no task/process
        // cancellation or shutdown request is sent to the daemon.
        self.client.take();
        // Best effort: raises a running Ask's cancel flag. If the process
        // exits before the worker reaches the daemon, the route's deadline
        // still bounds the daemon-owned attempt.
        self.ask.reset();
        // Stops the Console's daemon feed thread.
        self.console.reset();
    }
}
