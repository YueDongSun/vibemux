//! Feeds the Console page for the active folder. While the page is on
//! screen it runs the content-free daemon feed (ADR 042 §2); it reads the
//! Git inventory through `vibemuxctl` and runs the read-only probe when
//! asked. Workers report through channels; nothing here blocks the UI
//! thread, writes state, or starts a daemon or a harness.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
};

use vibemux_probe::ProbeReport;

use crate::{
    ViewModel,
    dashboard::{
        DaemonView, Workspace,
        daemon_feed::{DaemonFeed, FeedRequest},
    },
    gui::ConsoleData,
    workspace_inventory,
};

/// The code shown when a worker thread could not start or stopped early.
const WORKER_UNAVAILABLE: &str = "frontend_worker_unavailable";
const WORKER_STOPPED: &str = "frontend_worker_stopped";
/// The daemon feed could not start for the folder.
const FEED_UNAVAILABLE: &str = "console_feed_unavailable";

#[derive(Default)]
pub(crate) struct ConsoleBridge {
    /// The folder the feed was started for, kept when the start failed so
    /// a failing folder is not retried every frame.
    feed_folder: Option<PathBuf>,
    /// Dropping the feed stops its thread.
    feed: Option<DaemonFeed>,
    workspace: Option<Receiver<Result<Workspace, String>>>,
    probe: Option<Receiver<ProbeReport>>,
}

impl ConsoleBridge {
    /// Forgets every worker, for a folder switch. A probe in flight is
    /// dropped too; its result would be for the same machine, but the
    /// Console's state was reset with the folder.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// One frame. Returns a fresh view model when a re-probe finished.
    pub(crate) fn tick(
        &mut self,
        data: &mut ConsoleData,
        folder: Option<&Path>,
        console_open: bool,
        context: &egui::Context,
    ) -> Option<ViewModel> {
        self.follow_feed(data, folder, console_open);
        if let Some(feed) = self.feed.as_mut() {
            if let Some(view) = feed.latest() {
                data.daemon = Some(view);
            }
            if data.take_refresh() {
                let _ = feed.request(FeedRequest::Refresh);
            }
            if let Some(task_id) = data.take_task() {
                let _ = feed.request(FeedRequest::OpenTask(task_id));
            }
        }
        if console_open
            && let Some(folder) = folder
            && data.take_workspace_load()
        {
            self.workspace = Some(spawn_workspace(folder.to_path_buf(), context.clone()));
        }
        self.poll_workspace(data);
        if data.take_reprobe() {
            match spawn_probe(context.clone()) {
                Some(receiver) => self.probe = Some(receiver),
                None => data.finish_probe(Err(WORKER_UNAVAILABLE.to_string())),
            }
        }
        self.poll_probe(data)
    }

    /// Runs the feed only while the Console is open, for the active folder.
    fn follow_feed(&mut self, data: &mut ConsoleData, folder: Option<&Path>, console_open: bool) {
        let wanted = folder.filter(|_| console_open);
        if wanted == self.feed_folder.as_deref() {
            return;
        }
        // Stopping first: dropping a feed joins its thread.
        self.feed = None;
        self.feed_folder = wanted.map(Path::to_path_buf);
        let Some(folder) = wanted else {
            return;
        };
        match DaemonFeed::start(folder) {
            Ok(feed) => self.feed = Some(feed),
            Err(_) => data.daemon = Some(Arc::new(DaemonView::not_running(FEED_UNAVAILABLE))),
        }
    }

    fn poll_workspace(&mut self, data: &mut ConsoleData) {
        let Some(receiver) = self.workspace.as_ref() else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WORKER_STOPPED.to_string()),
        };
        self.workspace = None;
        data.set_workspace(result);
    }

    fn poll_probe(&mut self, data: &mut ConsoleData) -> Option<ViewModel> {
        let receiver = self.probe.as_ref()?;
        let report = match receiver.try_recv() {
            Ok(report) => report,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                self.probe = None;
                data.finish_probe(Err(WORKER_STOPPED.to_string()));
                return None;
            }
        };
        self.probe = None;
        data.finish_probe(Ok(()));
        Some(ViewModel::from_report(&report))
    }
}

fn spawn_workspace(folder: PathBuf, context: egui::Context) -> Receiver<Result<Workspace, String>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("console_workspace".into())
        .spawn(move || {
            let _ = sender.send(workspace_inventory::load(&folder));
            context.request_repaint();
        });
    if spawned.is_err() {
        let (failed_sender, failed) = mpsc::sync_channel(1);
        let _ = failed_sender.send(Err(WORKER_UNAVAILABLE.to_string()));
        return failed;
    }
    receiver
}

/// The same read-only probe the GUI runs at startup.
fn spawn_probe(context: egui::Context) -> Option<Receiver<ProbeReport>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("console_probe".into())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let report = runtime.block_on(vibemux_probe::run_probe(
                &vibemux_probe::ProbeConfig::from_environment(),
            ));
            let _ = sender.send(report);
            context.request_repaint();
        })
        .ok()
        .map(|_| receiver)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::{dashboard::Section, gui::WorkspaceLoad};

    #[test]
    fn the_console_reads_only_while_it_is_open() {
        let folder = tempfile::tempdir().expect("folder");
        let context = egui::Context::default();
        let mut bridge = ConsoleBridge::default();
        let mut data = ConsoleData::default();
        assert!(
            bridge
                .tick(&mut data, Some(folder.path()), false, &context)
                .is_none()
        );
        assert!(bridge.feed.is_none());
        assert!(
            matches!(data.workspace, WorkspaceLoad::NotLoaded),
            "a closed Console reads nothing"
        );

        // No daemon runs for the folder, and no vibemuxctl sits next to the
        // test binary: both reads end with a reason instead of data.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            bridge.tick(&mut data, Some(folder.path()), true, &context);
            let daemon_read = data
                .daemon
                .as_ref()
                .is_some_and(|view| matches!(view.health, Section::Unavailable(_)));
            let workspace_read = matches!(data.workspace, WorkspaceLoad::Failed(_));
            if daemon_read && workspace_read {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out: {:?} {:?}",
                data.daemon,
                data.workspace
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(bridge.feed.is_some());

        bridge.tick(&mut data, Some(folder.path()), false, &context);
        assert!(bridge.feed.is_none(), "closing the Console stops the feed");
        bridge.tick(&mut data, None, true, &context);
        assert!(bridge.feed.is_none(), "no folder, no feed");
    }
}
