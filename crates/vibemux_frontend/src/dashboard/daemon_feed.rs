//! Background reader of content-free daemon state for the console.
//!
//! One thread with a current-thread runtime polls the daemon every
//! `POLL_INTERVAL` and on request, and publishes an immutable `DaemonView`.
//! It calls only read operations: no prompt, transcript, output page, or
//! state change, and it never starts or recovers a daemon.
use std::{io, path::Path, sync::Arc, thread::JoinHandle, time::Duration};

use tokio::sync::{mpsc, watch};
use vibemux_types::{TaskId, frontend::FrontendTasksQuery};
use vibemuxd::{
    control::{ControlClient, ControlError, DecisionListQuery},
    process::DaemonPaths,
};

use super::daemon_model::{
    DaemonView, DecisionRow, MAX_DECISIONS, Section, TaskDetailView, merge_decisions,
    project_decision, project_dsh, project_health, project_plugins, project_routes, project_slots,
    project_task_detail, project_tasks, unavailable,
};
use crate::daemon_session::DaemonSession;

pub const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Pending UI requests; a full queue drops the request rather than block.
pub const REQUEST_CAPACITY: usize = 8;
pub const MAX_DECISION_PAGES_PER_POLL: usize = 4;
pub const DECISION_PAGE_LIMIT: usize = 50;
/// Health is read first and bounds how long a crashed daemon (a descriptor
/// with nothing listening) can keep the views on their last observation.
pub const HEALTH_DEADLINE: Duration = Duration::from_secs(2);

/// What the console asks of the feed. Both are reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeedRequest {
    /// Poll now instead of at the next tick.
    Refresh,
    /// Read this task's detail on every poll until another task is opened.
    OpenTask(String),
}

pub struct DaemonFeed {
    requests: mpsc::Sender<FeedRequest>,
    views: watch::Receiver<Arc<DaemonView>>,
    stop: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}

impl DaemonFeed {
    /// Starts the feed for the daemon of `project_root`.
    pub fn start(project_root: &Path) -> io::Result<Self> {
        let paths = DaemonPaths::from_project_root(project_root)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.code()))?;
        Self::start_for_paths(paths)
    }

    pub fn start_for_paths(paths: DaemonPaths) -> io::Result<Self> {
        let (requests, receiver) = mpsc::channel(REQUEST_CAPACITY);
        let (updates, views) = watch::channel(Arc::new(DaemonView::waiting()));
        let (stop, stopped) = watch::channel(false);
        let worker = std::thread::Builder::new()
            .name("console_daemon_feed".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(feed_loop(paths, updates, receiver, stopped)),
                    Err(_) => {
                        updates.send_replace(Arc::new(DaemonView::not_running(
                            "console_runtime_unavailable",
                        )));
                    }
                }
            })?;
        Ok(Self {
            requests,
            views,
            stop,
            worker: Some(worker),
        })
    }

    /// The newest observation, once per change.
    pub fn latest(&mut self) -> Option<Arc<DaemonView>> {
        self.views
            .has_changed()
            .ok()
            .filter(|changed| *changed)
            .map(|_| Arc::clone(&self.views.borrow_and_update()))
    }

    /// Queues a request. Returns false when the queue is full or the feed
    /// stopped; the next poll still happens.
    pub fn request(&self, request: FeedRequest) -> bool {
        self.requests.try_send(request).is_ok()
    }
}

impl Drop for DaemonFeed {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// What the feed keeps between polls.
#[derive(Default)]
struct FeedState {
    session: DaemonSession,
    opened: Option<String>,
    /// Newest first, at most `MAX_DECISIONS`.
    decisions: Vec<DecisionRow>,
}

async fn feed_loop(
    paths: DaemonPaths,
    updates: watch::Sender<Arc<DaemonView>>,
    mut requests: mpsc::Receiver<FeedRequest>,
    mut stopped: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut state = FeedState::default();
    loop {
        tokio::select! {
            biased;
            _ = stopped.changed() => break,
            request = requests.recv() => match request {
                Some(FeedRequest::Refresh) => {}
                Some(FeedRequest::OpenTask(task_id)) => state.opened = Some(task_id),
                None => break,
            },
            _ = tick.tick() => {}
        }
        let view = tokio::select! {
            biased;
            _ = stopped.changed() => break,
            view = poll(&paths, &mut state) => view,
        };
        updates.send_replace(Arc::new(view));
    }
}

/// Turns read results into sections and remembers whether any failure means
/// the connection itself is suspect, so the next poll verifies the runtime
/// again.
#[derive(Default)]
struct Reads {
    suspect: bool,
}

impl Reads {
    fn section<T>(&mut self, result: Result<T, ControlError>) -> Section<T> {
        match result {
            Ok(value) => Section::Loaded(value),
            Err(error) => {
                self.suspect |= !matches!(
                    error,
                    ControlError::Remote { .. } | ControlError::UnsupportedVersion
                );
                Section::Unavailable(unavailable(error.code()))
            }
        }
    }
}

async fn poll(paths: &DaemonPaths, state: &mut FeedState) -> DaemonView {
    let client = match state.session.connect(paths) {
        Ok((client, _)) => client,
        Err(code) => {
            state.session.invalidate();
            return DaemonView::not_running(&code);
        }
    };
    // Health first: a daemon that cannot answer it cannot answer the rest.
    let health = match client.health_with_deadline(HEALTH_DEADLINE).await {
        Ok(health) => project_health(&health),
        Err(error) => {
            state.session.invalidate();
            return DaemonView::not_running(error.code());
        }
    };
    let mut reads = Reads::default();
    let plugins = reads.section(
        client
            .plugin_status()
            .await
            .map(|rows| project_plugins(&rows)),
    );
    let routes = reads.section(
        client
            .harness_dispatch_catalog()
            .await
            .map(|rows| project_routes(&rows)),
    );
    let tasks = reads.section(
        client
            .frontend_tasks(FrontendTasksQuery::default())
            .await
            .map(|page| project_tasks(&page)),
    );
    let dsh = reads.section(
        client
            .dsh_session_status()
            .await
            .map(|status| project_dsh(&status)),
    );
    let slots = reads.section(
        client
            .workflow_slots()
            .await
            .map(|value| project_slots(&value)),
    );
    let task_detail = match state.opened.as_deref() {
        None => None,
        Some(task_id) => Some(read_task_detail(&client, task_id, &mut reads).await),
    };
    let decisions = match read_decisions(&client, &state.decisions).await {
        Ok(read) => {
            merge_decisions(&mut state.decisions, read, MAX_DECISIONS);
            Section::Loaded(state.decisions.clone())
        }
        Err(error) => reads.section(Err(error)),
    };
    if reads.suspect {
        state.session.invalidate();
    }
    DaemonView {
        connection: "connected".to_string(),
        observed_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok(),
        health: Section::Loaded(health),
        plugins,
        routes,
        tasks,
        task_detail,
        dsh,
        slots,
        decisions,
    }
}

async fn read_task_detail(
    client: &ControlClient,
    task_id: &str,
    reads: &mut Reads,
) -> Section<TaskDetailView> {
    let Ok(task_id) = task_id.parse::<TaskId>() else {
        return Section::Unavailable(unavailable("frontend_invalid_task_id"));
    };
    match client.frontend_task(task_id).await {
        Ok(Some(detail)) => Section::Loaded(project_task_detail(&detail)),
        Ok(None) => Section::Unavailable(unavailable("frontend_task_not_found")),
        Err(error) => reads.section(Err(error)),
    }
}
/// Re-reads from the oldest kept decision, because a kept decision can be
/// consumed after it was recorded, then continues forward. Bounded to
/// `MAX_DECISION_PAGES_PER_POLL` pages; a longer history catches up over the
/// next polls, since the oldest kept decision moves forward each time.
async fn read_decisions(
    client: &ControlClient,
    kept: &[DecisionRow],
) -> Result<Vec<DecisionRow>, ControlError> {
    let mut after_sequence = kept
        .iter()
        .map(|row| row.created_sequence)
        .min()
        .map_or(0, |oldest| oldest.saturating_sub(1));
    let mut read = Vec::new();
    for _ in 0..MAX_DECISION_PAGES_PER_POLL {
        let page = client
            .decision_list(DecisionListQuery {
                after_sequence,
                limit: DECISION_PAGE_LIMIT,
            })
            .await?;
        read.extend(page.decisions.iter().map(project_decision));
        match page.next_after_sequence {
            Some(next) => after_sequence = next,
            None => break,
        }
    }
    Ok(read)
}
