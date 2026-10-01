//! Native application adapter joining presentation with the control reader.
use std::sync::Arc;

use crate::{
    gui::VibeMuxApp, supervisor_client::FrontendClient, supervisor_model::SupervisorSnapshot,
};

pub struct LiveApp {
    app: VibeMuxApp,
    client: Option<FrontendClient>,
    /// Shared handle to the last observation. Publishing moves this instead of
    /// deep-copying the snapshot, so a poll costs one atomic increment.
    snapshot: Arc<SupervisorSnapshot>,
}

impl LiveApp {
    pub fn new(mut app: VibeMuxApp, context: egui::Context) -> Self {
        let mut snapshot = SupervisorSnapshot::unavailable();
        let client = std::env::current_dir().and_then(|root| FrontendClient::start(root, context));
        if client.is_err() {
            snapshot.connection.detail =
                Some("Unable to start the workspace observation client.".into());
        }
        let snapshot = Arc::new(snapshot);
        app.replace_snapshot(Arc::clone(&snapshot));
        Self {
            app,
            client: client.ok(),
            snapshot,
        }
    }
}

impl eframe::App for LiveApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(client) = self.client.as_mut() {
            if let Some(snapshot) = client.take_snapshot() {
                self.snapshot = snapshot;
                self.app.replace_snapshot(Arc::clone(&self.snapshot));
            }
        }
        self.app.update(ctx, frame);
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
                ctx.request_repaint();
            }
        }
    }
    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.app.on_exit(gl);
        // Cancels the reader's select branch before joining; no task/process
        // cancellation or shutdown request is sent to the daemon.
        self.client.take();
    }
}
