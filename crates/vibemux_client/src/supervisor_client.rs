//! Owned background reader and bounded action channel for the GUI.
//! No daemon startup, SQLite access, provider invocation, or implicit retries
//! of focus/link actions occur here.

use crate::{
    change_notifier::ChangeNotifier, daemon_session::DaemonSession, supervisor_mapping,
    supervisor_model::*,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, watch};
use vibemux_types::{
    ProjectId, RunId, TaskId,
    frontend::{FrontendTaskDetail, FrontendTasksQuery},
};
use vibemuxd::{
    control::{ControlClient, ControlError},
    process::DaemonPaths,
    terminal_observer::{TerminalLinkRequest, TerminalQuery},
};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Harness/plugin metadata changes far less often than task state, so it is
/// re-read on this cadence instead of on every poll.
const METADATA_INTERVAL_TICKS: u32 = 12;
const ACTION_CAPACITY: usize = 16;
const MAX_LOADED_TASKS: usize = 256;
const MAX_OBSERVED_TERMINALS: usize = 16;
const TERMINAL_PLUGIN_ID: &str = "wezterm_observer";
/// Upper bound on in-flight detail reads. The daemon serializes every read
/// behind its single writer, so a small window keeps the pipe busy without
/// queueing work that would only add latency.
const MAX_CONCURRENT_DETAIL_READS: usize = 8;

pub struct FrontendClient {
    actions: mpsc::Sender<SupervisorAction>,
    snapshots: watch::Receiver<Arc<SupervisorSnapshot>>,
    stop: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}

impl FrontendClient {
    pub fn start(
        project_root: PathBuf,
        notifier: Arc<dyn ChangeNotifier>,
    ) -> Result<Self, std::io::Error> {
        let (actions, requests) = mpsc::channel(ACTION_CAPACITY);
        let mut initial = SupervisorSnapshot::unavailable();
        initial.project_name = project_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Workspace".into());
        initial.coordinator = None;
        initial.connection.status = ConnectionStatus::Connecting;
        let (updates, snapshots) = watch::channel(Arc::new(initial.clone()));
        let (stop, stopped) = watch::channel(false);
        let worker = std::thread::Builder::new()
            .name("frontend_control_reader".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(read_loop(
                        project_root,
                        initial,
                        updates,
                        requests,
                        stopped,
                        notifier,
                    )),
                    Err(_) => {
                        let mut failed = initial;
                        failed.connection = connection_failure("frontend_runtime_unavailable");
                        updates.send_replace(Arc::new(failed));
                        notifier.notify();
                    }
                }
            })?;
        Ok(Self {
            actions,
            snapshots,
            stop,
            worker: Some(worker),
        })
    }

    /// Hand the caller a shared handle to the newest observation. The snapshot
    /// is only copied when the caller still holds a stale one.
    pub fn take_snapshot(&mut self) -> Option<Arc<SupervisorSnapshot>> {
        self.snapshots
            .has_changed()
            .ok()
            .filter(|changed| *changed)
            .map(|_| Arc::clone(&self.snapshots.borrow_and_update()))
    }

    pub fn submit(&self, action: SupervisorAction) -> Result<(), &'static str> {
        self.actions
            .try_send(action)
            .map_err(|_| "The request queue is busy. Please try again.")
    }
}

impl Drop for FrontendClient {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn read_loop(
    root: PathBuf,
    snapshot: SupervisorSnapshot,
    updates: watch::Sender<Arc<SupervisorSnapshot>>,
    mut requests: mpsc::Receiver<SupervisorAction>,
    mut stopped: watch::Receiver<bool>,
    notifier: Arc<dyn ChangeNotifier>,
) {
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut observed = BTreeSet::new();
    // The verified client is reused across polls. Re-verifying the runtime ACLs
    // and re-reading the descriptor on every 5s tick was pure overhead: the
    // descriptor only changes when the daemon restarts, which the generation
    // check below already detects.
    let mut session = DaemonSession::default();
    let mut ticks_since_metadata: u32 = METADATA_INTERVAL_TICKS;
    // Held across iterations so a poll that changes nothing republishes the same
    // allocation instead of rebuilding the task tree.
    let mut published = Arc::new(snapshot);
    loop {
        let action = tokio::select! {
            biased;
            _ = stopped.changed() => break,
            request = requests.recv() => match request {Some(action) => action,None => break},
            _ = tick.tick() => SupervisorAction::Refresh,
        };
        let refresh_metadata = ticks_since_metadata >= METADATA_INTERVAL_TICKS;
        // Copy-on-write: `make_mut` clones only while a reader still holds the
        // previous observation, so an idle poll allocates nothing.
        let result = {
            let next = Arc::make_mut(&mut published);
            tokio::select! {
                biased;
                _ = stopped.changed() => break,
                result = apply_action(&root, next, action, &mut observed, &mut session, refresh_metadata) => result,
            }
        };
        let next = Arc::make_mut(&mut published);
        match result {
            Ok(()) => ticks_since_metadata = ticks_since_metadata.saturating_add(1),
            Err(code) => {
                // A failed call may have left a stale connection cached.
                session.invalidate();
                ticks_since_metadata = METADATA_INTERVAL_TICKS;
                if code.starts_with("frontend_") || code.starts_with("terminal_") {
                    next.connection.detail = Some(format!(
                        "Action unavailable ({code}). Task observations are unchanged."
                    ));
                } else {
                    next.connection = connection_failure(&code);
                    clear_terminal_associations(
                        next,
                        "Disconnected — revalidate the native terminal",
                    );
                }
            }
        }
        Arc::make_mut(&mut published).observed_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok();
        updates.send_replace(Arc::clone(&published));
        notifier.notify();
    }
}

/// Resolve the project's daemon paths and return a usable client plus whether
/// the runtime generation changed (see `DaemonSession::connect`).
fn client_for(root: &Path, session: &mut DaemonSession) -> Result<(ControlClient, bool), String> {
    let paths = DaemonPaths::from_project_root(root).map_err(|error| error.code().to_string())?;
    session.connect(&paths)
}

async fn apply_action(
    root: &Path,
    snapshot: &mut SupervisorSnapshot,
    action: SupervisorAction,
    observed: &mut BTreeSet<(String, String)>,
    session: &mut DaemonSession,
    refresh_metadata: bool,
) -> Result<(), String> {
    let (client, restarted) = client_for(root, session)?;
    if restarted {
        clear_terminal_associations(
            snapshot,
            "Daemon restarted — inspect the native terminal again",
        );
        observed.clear();
    }
    match action {
        SupervisorAction::Refresh => {
            let mut query = FrontendTasksQuery::default();
            let previous_limit = snapshot
                .tasks
                .len()
                .max(query.limit as usize)
                .min(MAX_LOADED_TASKS);
            let mut summaries = Vec::new();
            loop {
                let page = client.frontend_tasks(query.clone()).await.map_err(code)?;
                let project_id = page.project_id.map(|id| id.to_string()).unwrap_or_default();
                if project_id != snapshot.project_id {
                    snapshot.tasks.clear();
                    observed.clear();
                }
                snapshot.project_id = project_id;
                summaries.extend(page.tasks);
                snapshot.next_cursor = page.next_cursor.map(|id| id.to_string());
                if summaries.len() >= previous_limit || page.next_cursor.is_none() {
                    break;
                }
                query.after = page.next_cursor;
            }
            // Carry forward the previous view for every task whose detail
            // revision is unchanged, so an idle poll performs no detail reads.
            let previous = snapshot.tasks.clone();
            snapshot.tasks = summaries
                .into_iter()
                .map(|summary| {
                    previous
                        .iter()
                        .find(|old| {
                            old.task_id == summary.task_id.to_string()
                                && old.latest_sequence == summary.latest_sequence
                                && old.detail_revision == summary.detail_revision
                        })
                        .cloned()
                        .unwrap_or_else(|| supervisor_mapping::task_summary(summary))
                })
                .collect();
            let stale: Vec<String> = snapshot
                .tasks
                .iter()
                .filter(|task| task.detail_revision == 0 || task.needs_detail_refresh)
                .map(|task| task.task_id.clone())
                .collect();
            load_details(&client, snapshot, &stale).await?;
            for task in &mut snapshot.tasks {
                task.needs_detail_refresh = false;
            }
            observed.retain(|(task_id, run_id)| {
                snapshot.tasks.iter().any(|task| {
                    &task.task_id == task_id && task.runs.iter().any(|run| &run.run_id == run_id)
                })
            });
            if refresh_metadata || snapshot.coordinator.is_none() {
                let harnesses = client.harness_snapshot().await.map_err(code)?;
                snapshot.coordinator = harnesses
                    .into_iter()
                    .find(|row| row.default)
                    .map(|row| row.name);
                let plugin = client
                    .plugin_status()
                    .await
                    .map_err(code)?
                    .into_iter()
                    .find(|plugin| plugin.plugin_id == TERMINAL_PLUGIN_ID);
                let terminal_status = match plugin {
                    Some(plugin)
                        if plugin.state == vibemuxd::plugin_registry::PluginState::Active =>
                    {
                        None
                    }
                    Some(plugin) => Some(format!(
                        "Terminal plugin is {:?}; native observation unavailable",
                        plugin.state
                    )),
                    None => Some(
                        "Native terminal observer is not configured for this daemon".to_owned(),
                    ),
                };
                if let Some(status) = terminal_status {
                    clear_terminal_associations(snapshot, &status);
                }
            }
            snapshot.connection = ConnectionState {status:ConnectionStatus::Connected,detail:Some("Task status connected. Continuous coordinator chat is not available in this build.".into()),code:None};
            for (task_id, run_id) in observed.iter() {
                inspect_terminal(&client, snapshot, task_id, run_id).await;
            }
        }
        SupervisorAction::LoadMoreTasks { cursor } => {
            if snapshot.next_cursor.as_deref() != Some(&cursor) {
                return Err("frontend_stale_cursor".into());
            }
            if snapshot.tasks.len() >= MAX_LOADED_TASKS {
                return Err("frontend_task_view_capacity".into());
            }
            let page = client
                .frontend_tasks(FrontendTasksQuery {
                    after: Some(parse_id::<TaskId>(&cursor)?),
                    limit: 32,
                })
                .await
                .map_err(code)?;
            if page.project_id.map(|id| id.to_string()).as_deref()
                != Some(snapshot.project_id.as_str())
            {
                return Err("frontend_project_changed".into());
            }
            let ids: Vec<String> = page
                .tasks
                .iter()
                .map(|task| task.task_id.to_string())
                .collect();
            for task in page.tasks {
                if !snapshot
                    .tasks
                    .iter()
                    .any(|old| old.task_id == task.task_id.to_string())
                {
                    snapshot.tasks.push(supervisor_mapping::task_summary(task));
                }
            }
            snapshot.next_cursor = page.next_cursor.map(|id| id.to_string());
            load_details(&client, snapshot, &ids).await?;
        }
        SupervisorAction::LoadTask { task_id } => {
            // An explicit open always reads the detail, even when the revision
            // is unchanged: the operator asked for this task specifically.
            load_task(&client, snapshot, &task_id).await?;
        }
        SupervisorAction::InspectTerminal { task_id, run_id } => {
            if observed.len() >= MAX_OBSERVED_TERMINALS
                && !observed.contains(&(task_id.clone(), run_id.clone()))
            {
                return Err("frontend_terminal_view_capacity".into());
            }
            observed.insert((task_id.clone(), run_id.clone()));
            inspect_terminal(&client, snapshot, &task_id, &run_id).await;
        }
        SupervisorAction::LinkTerminal {
            task_id,
            run_id,
            plugin_id,
            pane_id,
            instance_id,
        } => {
            let query = terminal_query(snapshot, &task_id, &run_id, &plugin_id)?;
            match client
                .terminal_link(TerminalLinkRequest {
                    query,
                    instance_id,
                    pane_id,
                })
                .await
            {
                Ok(binding) => {
                    if let Some(run) = find_run(snapshot, &task_id, &run_id) {
                        run.terminal = supervisor_mapping::binding_view(&binding);
                    }
                }
                Err(error) => terminal_failure(snapshot, &task_id, &run_id, &error),
            }
        }
        SupervisorAction::FocusTerminal { binding_id } => {
            let target = find_binding(snapshot, &binding_id).ok_or("terminal_link_expired")?;
            if let Err(error) = client.terminal_focus(&binding_id).await {
                terminal_failure(snapshot, &target.0, &target.1, &error);
            } else {
                inspect_terminal(&client, snapshot, &target.0, &target.1).await;
            }
        }
        SupervisorAction::UnlinkTerminal { binding_id } => {
            let target = find_binding(snapshot, &binding_id).ok_or("terminal_link_expired")?;
            client.terminal_unlink(&binding_id).await.map_err(code)?;
            observed.remove(&target);
            if let Some(run) = find_run(snapshot, &target.0, &target.1) {
                run.terminal = TerminalView {
                    status: "Not linked".into(),
                    ..Default::default()
                };
            }
        }
    }
    if snapshot.tasks.len() >= MAX_LOADED_TASKS && snapshot.next_cursor.take().is_some() {
        snapshot.connection.detail = Some(format!(
            "Showing the first {MAX_LOADED_TASKS} tasks. Other tasks remain in the daemon."
        ));
    }
    Ok(())
}

/// Read the listed task details with bounded concurrency.
///
/// The daemon funnels every read through one writer, so this trades a little
/// overlap for a large reduction in wall-clock when a page's worth of tasks
/// really did change. Ordering is preserved so the resulting view does not
/// depend on completion order.
async fn load_details(
    client: &ControlClient,
    snapshot: &mut SupervisorSnapshot,
    task_ids: &[String],
) -> Result<(), String> {
    if task_ids.is_empty() {
        return Ok(());
    }
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_DETAIL_READS));
    let mut in_flight = Vec::with_capacity(task_ids.len());
    for task_id in task_ids {
        let permit = Arc::clone(&permits)
            .acquire_owned()
            .await
            .map_err(|_| "frontend_closed")?;
        let client = client.clone();
        let task_id = task_id.clone();
        in_flight.push(tokio::spawn(async move {
            let _permit = permit;
            let detail = client
                .frontend_task(parse_id::<TaskId>(&task_id)?)
                .await
                .map_err(code)?;
            Ok::<_, String>((task_id, detail))
        }));
    }
    let mut results = Vec::with_capacity(in_flight.len());
    for handle in in_flight {
        match handle.await {
            Ok(result) => results.push(result?),
            Err(_) => return Err("frontend_detail_read_failed".into()),
        }
    }
    // `load_task` re-checks the project and revision, so applying results in
    // task order cannot resurrect a stale row.
    let mut by_id: BTreeMap<String, Option<FrontendTaskDetail>> = BTreeMap::new();
    for (task_id, detail) in results {
        by_id.insert(task_id, detail);
    }
    for task_id in task_ids {
        let Some(detail) = by_id.remove(task_id) else {
            continue;
        };
        apply_task_detail(snapshot, task_id, detail)?;
    }
    Ok(())
}

async fn load_task(
    client: &ControlClient,
    snapshot: &mut SupervisorSnapshot,
    task_id: &str,
) -> Result<(), String> {
    let detail = client
        .frontend_task(parse_id::<TaskId>(task_id)?)
        .await
        .map_err(code)?;
    apply_task_detail(snapshot, task_id, detail)
}

/// Merge one task detail into the snapshot, preserving locally observed
/// terminal state and refusing to move a task backwards.
fn apply_task_detail(
    snapshot: &mut SupervisorSnapshot,
    task_id: &str,
    detail: Option<FrontendTaskDetail>,
) -> Result<(), String> {
    if let Some(detail) = detail {
        if detail.task.project_id.to_string() != snapshot.project_id {
            return Err("frontend_project_changed".into());
        }
        if let Some(index) = snapshot
            .tasks
            .iter()
            .position(|task| task.task_id == task_id)
        {
            if detail.task.latest_sequence >= snapshot.tasks[index].latest_sequence {
                snapshot.tasks[index] =
                    supervisor_mapping::task_detail(detail, Some(&snapshot.tasks[index]));
            }
        } else if snapshot.tasks.len() < MAX_LOADED_TASKS {
            snapshot
                .tasks
                .push(supervisor_mapping::task_detail(detail, None));
        } else {
            return Err("frontend_task_view_capacity".into());
        }
    } else {
        snapshot.tasks.retain(|task| task.task_id != task_id);
    }
    Ok(())
}

async fn inspect_terminal(
    client: &ControlClient,
    snapshot: &mut SupervisorSnapshot,
    task_id: &str,
    run_id: &str,
) {
    let query = match terminal_query(snapshot, task_id, run_id, TERMINAL_PLUGIN_ID) {
        Ok(query) => query,
        Err(_) => return,
    };
    match client.terminal_inspect(query).await {
        Ok(value) => {
            if let Some(run) = find_run(snapshot, task_id, run_id) {
                run.terminal = supervisor_mapping::terminal_snapshot(value, TERMINAL_PLUGIN_ID);
            }
        }
        Err(error) => terminal_failure(snapshot, task_id, run_id, &error),
    }
}

fn terminal_query(
    snapshot: &SupervisorSnapshot,
    task_id: &str,
    run_id: &str,
    plugin_id: &str,
) -> Result<TerminalQuery, String> {
    if !snapshot
        .tasks
        .iter()
        .any(|task| task.task_id == task_id && task.runs.iter().any(|run| run.run_id == run_id))
    {
        return Err("terminal_run_unavailable".into());
    }
    Ok(TerminalQuery {
        project_id: parse_id::<ProjectId>(&snapshot.project_id)?,
        task_id: parse_id::<TaskId>(task_id)?,
        run_id: parse_id::<RunId>(run_id)?,
        plugin_id: plugin_id.into(),
    })
}
fn find_run<'a>(
    snapshot: &'a mut SupervisorSnapshot,
    task_id: &str,
    run_id: &str,
) -> Option<&'a mut RunView> {
    snapshot
        .tasks
        .iter_mut()
        .find(|task| task.task_id == task_id)?
        .runs
        .iter_mut()
        .find(|run| run.run_id == run_id)
}
fn find_binding(snapshot: &SupervisorSnapshot, binding_id: &str) -> Option<(String, String)> {
    snapshot.tasks.iter().find_map(|task| {
        task.runs
            .iter()
            .find(|run| run.terminal.binding_id.as_deref() == Some(binding_id))
            .map(|run| (task.task_id.clone(), run.run_id.clone()))
    })
}
fn terminal_failure(
    snapshot: &mut SupervisorSnapshot,
    task_id: &str,
    run_id: &str,
    error: &ControlError,
) {
    if let Some(run) = find_run(snapshot, task_id, run_id) {
        run.terminal = TerminalView {
            status: format!("Native terminal unavailable ({})", error.code()),
            ..Default::default()
        };
    }
}
fn clear_terminal_associations(snapshot: &mut SupervisorSnapshot, status: &str) {
    for task in &mut snapshot.tasks {
        for run in &mut task.runs {
            run.terminal = TerminalView {
                status: status.to_owned(),
                ..Default::default()
            };
        }
    }
}
fn parse_id<T: std::str::FromStr>(value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| "frontend_invalid_identity".into())
}
fn code(error: ControlError) -> String {
    error.code().into()
}
fn connection_failure(code: &str) -> ConnectionState {
    let detail = match code {
        "daemon_not_running" => {
            "The daemon is not running for this project. Start it explicitly to view tasks."
                .to_string()
        }
        "control_unsupported_version" => {
            "This daemon does not support Control v4. Update it to view tasks and native terminals."
                .to_string()
        }
        _ => format!("Connection unavailable ({code}). Last observations may be stale."),
    };
    ConnectionState {
        status: ConnectionStatus::Disconnected,
        detail: Some(detail),
        code: Some(code.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change_notifier::{CountingNotifier, NoopNotifier};
    use vibemux_types::{
        Run, RunSpec, Task, TaskSpec,
        a2a::{A2aRunAction, A2aRunStart, A2aRunUpdate, RunWorkspace},
    };
    use vibemuxd::{WriterWorker, control::DaemonControlServer};

    async fn wait_snapshot(
        client: &mut FrontendClient,
        predicate: impl Fn(&SupervisorSnapshot) -> bool,
    ) -> Arc<SupervisorSnapshot> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Some(snapshot) = client.take_snapshot()
                    && predicate(snapshot.as_ref())
                {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("frontend observation deadline")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reads_real_daemon_tasks_and_disconnects_without_owning_execution() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DaemonPaths::from_project_root(temp.path()).unwrap();
        paths.ensure_runtime_dir().unwrap();
        let writer = WriterWorker::start(paths.database_path()).unwrap();
        let project_id = writer.handle().unwrap().project_id().unwrap().unwrap();
        let task = Task::new(TaskSpec {
            project_id,
            title: "Observe real task".into(),
            description: String::new(),
        })
        .unwrap();
        let run = Run::new(RunSpec {
            project_id,
            task_id: task.task_id(),
            harness: "fixture".into(),
            role: "worker".into(),
            protocol: "a2a".into(),
            base_commit: "a".repeat(40),
        })
        .unwrap();
        let run_id = run.run_id();
        writer
            .handle()
            .unwrap()
            .start_a2a_run(A2aRunStart {
                task,
                run,
                peer_id: "fixture_peer".into(),
                external_task_id: "external_fixture".into(),
                transport: "http_json".into(),
                protocol_version: "1.0".into(),
                workspace: RunWorkspace {
                    path: temp.path().to_string_lossy().into_owned(),
                    branch: "codex/fixture".into(),
                    base_commit: "a".repeat(40),
                    ownership_token: "PRIVATE_OWNER".into(),
                },
                timestamp: time::OffsetDateTime::now_utc(),
                idempotency_key: "frontend_fixture_start".into(),
            })
            .unwrap();
        let before = writer.handle().unwrap().a2a_run(run_id).unwrap();
        writer.shutdown().unwrap();
        let server = DaemonControlServer::start_for_paths(&paths).await.unwrap();
        let notifier = Arc::new(CountingNotifier::default());
        let mut client = FrontendClient::start(temp.path().into(), notifier.clone()).unwrap();
        let snapshot = wait_snapshot(&mut client, |snapshot| {
            snapshot.connection.status == ConnectionStatus::Connected && snapshot.tasks.len() == 1
        })
        .await;
        assert!(
            notifier.count() >= 1,
            "a Connected snapshot must have been preceded by at least one notify()"
        );
        assert_eq!(snapshot.tasks[0].title, "Observe real task");
        assert_eq!(snapshot.tasks[0].runs[0].run_id, run_id.to_string());
        let serialized = serde_json::to_string(snapshot.as_ref()).unwrap();
        assert!(!serialized.contains("PRIVATE_"));
        client
            .submit(SupervisorAction::InspectTerminal {
                task_id: snapshot.tasks[0].task_id.clone(),
                run_id: run_id.to_string(),
            })
            .unwrap();
        let snapshot = wait_snapshot(&mut client, |snapshot| {
            snapshot.tasks.first().is_some_and(|task| {
                task.runs[0]
                    .terminal
                    .status
                    .contains("terminal_plugin_unavailable")
            })
        })
        .await;
        assert_eq!(snapshot.connection.status, ConnectionStatus::Connected);
        server.shutdown().await.unwrap();
        client.submit(SupervisorAction::Refresh).unwrap();
        let disconnected = wait_snapshot(&mut client, |snapshot| {
            snapshot.connection.status == ConnectionStatus::Disconnected
        })
        .await;
        assert_eq!(disconnected.tasks[0].runs[0].state, "preparing");
        drop(client);
        let writer = WriterWorker::start(paths.database_path()).unwrap();
        assert_eq!(writer.handle().unwrap().a2a_run(run_id).unwrap(), before);
        writer.shutdown().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unchanged_tasks_keep_their_detail_and_changed_ones_are_refetched() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DaemonPaths::from_project_root(temp.path()).unwrap();
        paths.ensure_runtime_dir().unwrap();
        let writer = WriterWorker::start(paths.database_path()).unwrap();
        let project_id = writer.handle().unwrap().project_id().unwrap().unwrap();
        let task = Task::new(TaskSpec {
            project_id,
            title: "Revision task".into(),
            description: String::new(),
        })
        .unwrap();
        let run = Run::new(RunSpec {
            project_id,
            task_id: task.task_id(),
            harness: "fixture".into(),
            role: "worker".into(),
            protocol: "a2a".into(),
            base_commit: "a".repeat(40),
        })
        .unwrap();
        let run_id = run.run_id();
        let started = writer
            .handle()
            .unwrap()
            .start_a2a_run(A2aRunStart {
                task,
                run,
                peer_id: "fixture_peer".into(),
                external_task_id: "external_revision".into(),
                transport: "http_json".into(),
                protocol_version: "1.0".into(),
                workspace: RunWorkspace {
                    path: temp.path().to_string_lossy().into_owned(),
                    branch: "codex/revision".into(),
                    base_commit: "a".repeat(40),
                    ownership_token: "PRIVATE_OWNER".into(),
                },
                timestamp: time::OffsetDateTime::now_utc(),
                idempotency_key: "frontend_revision_start".into(),
            })
            .unwrap()
            .record;
        writer.shutdown().unwrap();
        let server = DaemonControlServer::start_for_paths(&paths).await.unwrap();
        let mut client = FrontendClient::start(temp.path().into(), Arc::new(NoopNotifier)).unwrap();
        let first = wait_snapshot(&mut client, |snapshot| {
            snapshot.connection.status == ConnectionStatus::Connected
                && snapshot.tasks.len() == 1
                && snapshot.tasks[0].runs.len() == 1
        })
        .await;
        assert!(first.tasks[0].detail_revision > 0);
        assert!(
            !first.tasks[0].needs_detail_refresh,
            "a detail read must clear the pending flag"
        );

        // A refresh over unchanged state must reuse the carried-forward view
        // instead of refetching, which is what keeps an idle poll cheap.
        client.submit(SupervisorAction::Refresh).unwrap();
        let second = wait_snapshot(&mut client, |snapshot| {
            snapshot.connection.status == ConnectionStatus::Connected
                && snapshot.observed_at != first.observed_at
        })
        .await;
        assert_eq!(
            second.tasks[0].detail_revision, first.tasks[0].detail_revision,
            "an idle poll must not invent revision movement"
        );
        assert!(!second.tasks[0].needs_detail_refresh);

        // Once the daemon records a transition the revision moves, and the
        // client must refetch so the new state reaches the view. The server is
        // stopped first because it owns the single writer, so this also
        // exercises the client's reconnect-and-reverify path.
        server.shutdown().await.unwrap();
        let writer = WriterWorker::start(paths.database_path()).unwrap();
        writer
            .handle()
            .unwrap()
            .update_a2a_run(A2aRunUpdate {
                run_id,
                expected_version: started.version,
                timestamp: time::OffsetDateTime::now_utc(),
                idempotency_key: "frontend_revision_claim".into(),
                action: A2aRunAction::Start,
            })
            .unwrap();
        writer.shutdown().unwrap();
        let server = DaemonControlServer::start_for_paths(&paths).await.unwrap();
        client.submit(SupervisorAction::Refresh).unwrap();
        let third = wait_snapshot(&mut client, |snapshot| {
            snapshot
                .tasks
                .first()
                .is_some_and(|task| task.runs.first().is_some_and(|run| run.state == "running"))
        })
        .await;
        assert!(
            third.tasks[0].detail_revision > first.tasks[0].detail_revision,
            "a recorded transition must move the revision"
        );
        assert_eq!(third.tasks[0].runs[0].state, "running");
        server.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn client_session_reuses_one_verified_generation_and_reverifies_after_a_change() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DaemonPaths::from_project_root(temp.path()).unwrap();
        paths.ensure_runtime_dir().unwrap();
        let writer = WriterWorker::start(paths.database_path()).unwrap();
        writer.shutdown().unwrap();
        let server = DaemonControlServer::start_for_paths(&paths).await.unwrap();

        let mut session = DaemonSession::default();
        let first = client_for(temp.path(), &mut session)
            .expect("first client")
            .0;
        assert!(session.client.is_some());
        let generation = session.generation.clone();
        // A second call against the same daemon must not re-verify: the session
        // keeps its client and its recorded generation.
        let (second, restarted) = client_for(temp.path(), &mut session).expect("second client");
        assert!(!restarted, "an unchanged descriptor is not a restart");
        assert_eq!(session.generation, generation, "generation is retained");
        assert!(
            first.same_generation(&second),
            "both calls observe the same daemon"
        );

        // Restarting the daemon republishes the descriptor, which the client
        // must notice instead of trusting its cached verification.
        server.shutdown().await.unwrap();
        let restarted_server = DaemonControlServer::start_for_paths(&paths).await.unwrap();
        let (third, restarted) = client_for(temp.path(), &mut session).expect("third client");
        assert!(
            restarted,
            "a republished descriptor must be reported as a new generation"
        );
        assert_ne!(
            session.generation, generation,
            "a republished descriptor must produce a new generation"
        );
        assert!(!first.same_generation(&third));
        restarted_server.shutdown().await.unwrap();

        // A failed call clears the cache so the next attempt re-verifies.
        session.invalidate();
        assert!(session.client.is_none());
        assert!(session.generation.is_none());
    }

    #[test]
    fn action_queue_is_bounded_and_does_not_invoke_the_runtime() {
        let (actions, _receiver) = mpsc::channel(1);
        let (_, snapshots) = watch::channel(Arc::new(SupervisorSnapshot::unavailable()));
        let (stop, _) = watch::channel(false);
        let client = FrontendClient {
            actions,
            snapshots,
            stop,
            worker: None,
        };
        assert!(client.submit(SupervisorAction::Refresh).is_ok());
        assert!(client.submit(SupervisorAction::Refresh).is_err());
    }
}
