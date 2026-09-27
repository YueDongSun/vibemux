//! Owned background reader and bounded action channel for the GUI.
//! No daemon startup, SQLite access, provider invocation, or implicit retries
//! of focus/link actions occur here.

use crate::{supervisor_mapping, supervisor_model::*};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::{mpsc, watch};
use vibemux_types::{ProjectId, RunId, TaskId, frontend::FrontendTasksQuery};
use vibemuxd::{
    control::{ControlClient, ControlError},
    process::DaemonPaths,
    terminal_observer::{TerminalLinkRequest, TerminalQuery},
};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const ACTION_CAPACITY: usize = 16;
const MAX_LOADED_TASKS: usize = 256;
const MAX_OBSERVED_TERMINALS: usize = 16;
const TERMINAL_PLUGIN_ID: &str = "wezterm_observer";

pub struct FrontendClient {
    actions: mpsc::Sender<SupervisorAction>,
    snapshots: watch::Receiver<SupervisorSnapshot>,
    stop: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}

impl FrontendClient {
    pub fn start(project_root: PathBuf, context: egui::Context) -> Result<Self, std::io::Error> {
        let (actions, requests) = mpsc::channel(ACTION_CAPACITY);
        let mut initial = SupervisorSnapshot::unavailable();
        initial.project_name = project_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Workspace".into());
        initial.coordinator = None;
        initial.connection.status = ConnectionStatus::Connecting;
        let (updates, snapshots) = watch::channel(initial.clone());
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
                        context,
                    )),
                    Err(_) => {
                        initial.connection = connection_failure("frontend_runtime_unavailable");
                        updates.send_replace(initial);
                        context.request_repaint();
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

    pub fn take_snapshot(&mut self) -> Option<SupervisorSnapshot> {
        self.snapshots
            .has_changed()
            .ok()
            .filter(|changed| *changed)
            .map(|_| self.snapshots.borrow_and_update().clone())
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
    mut snapshot: SupervisorSnapshot,
    updates: watch::Sender<SupervisorSnapshot>,
    mut requests: mpsc::Receiver<SupervisorAction>,
    mut stopped: watch::Receiver<bool>,
    context: egui::Context,
) {
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut observed = BTreeSet::new();
    let mut last_client = None;
    loop {
        let action = tokio::select! {
            biased;
            _ = stopped.changed() => break,
            request = requests.recv() => match request {Some(action) => action,None => break},
            _ = tick.tick() => SupervisorAction::Refresh,
        };
        let mut next = snapshot.clone();
        let result = tokio::select! {
            biased;
            _ = stopped.changed() => break,
            result = apply_action(&root, &mut next, action, &mut observed, &mut last_client) => result,
        };
        if let Err(code) = result {
            if code.starts_with("frontend_") || code.starts_with("terminal_") {
                next.connection.detail = Some(format!(
                    "Action unavailable ({code}). Task observations are unchanged."
                ));
            } else {
                next.connection = connection_failure(&code);
                clear_terminal_associations(
                    &mut next,
                    "Disconnected — revalidate the native terminal",
                );
            }
        }
        next.observed_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok();
        snapshot = next;
        updates.send_replace(snapshot.clone());
        context.request_repaint();
    }
}

fn connect(root: &Path) -> Result<ControlClient, String> {
    let paths = DaemonPaths::from_project_root(root).map_err(|error| error.code().to_string())?;
    if !paths.descriptor_path().exists() {
        return Err("daemon_not_running".into());
    }
    paths
        .validate_runtime_dir()
        .map_err(|error| error.code().to_string())?;
    #[cfg(windows)]
    paths
        .verify_control_security()
        .map_err(|error| error.code().to_string())?;
    ControlClient::from_descriptor(paths.descriptor_path())
        .map_err(|error| error.code().to_string())
}

async fn apply_action(
    root: &Path,
    snapshot: &mut SupervisorSnapshot,
    action: SupervisorAction,
    observed: &mut BTreeSet<(String, String)>,
    last_client: &mut Option<ControlClient>,
) -> Result<(), String> {
    let client = connect(root)?;
    if last_client
        .as_ref()
        .is_some_and(|previous| !previous.same_generation(&client))
    {
        clear_terminal_associations(
            snapshot,
            "Daemon restarted — inspect the native terminal again",
        );
        observed.clear();
    }
    *last_client = Some(client.clone());
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
            let previous = snapshot.tasks.clone();
            snapshot.tasks = summaries
                .into_iter()
                .map(|summary| {
                    previous
                        .iter()
                        .find(|old| {
                            old.task_id == summary.task_id.to_string()
                                && old.latest_sequence == summary.latest_sequence
                        })
                        .cloned()
                        .unwrap_or_else(|| supervisor_mapping::task_summary(summary))
                })
                .collect();
            let tasks: Vec<String> = snapshot
                .tasks
                .iter()
                .map(|task| task.task_id.clone())
                .collect();
            for task_id in tasks {
                load_task(&client, snapshot, &task_id).await?;
            }
            observed.retain(|(task_id, run_id)| {
                snapshot.tasks.iter().any(|task| {
                    &task.task_id == task_id && task.runs.iter().any(|run| &run.run_id == run_id)
                })
            });
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
                Some(plugin) if plugin.state == vibemuxd::plugin_registry::PluginState::Active => {
                    None
                }
                Some(plugin) => Some(format!(
                    "Terminal plugin is {:?}; native observation unavailable",
                    plugin.state
                )),
                None => {
                    Some("Native terminal observer is not configured for this daemon".to_owned())
                }
            };
            if let Some(status) = terminal_status {
                clear_terminal_associations(snapshot, &status);
            }
            snapshot.connection = ConnectionState {status:ConnectionStatus::Connected,detail:Some("Task status connected. Continuous coordinator chat is not available in this build.".into())};
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
            for id in ids {
                load_task(&client, snapshot, &id).await?;
            }
        }
        SupervisorAction::LoadTask { task_id } => load_task(&client, snapshot, &task_id).await?,
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

async fn load_task(
    client: &ControlClient,
    snapshot: &mut SupervisorSnapshot,
    task_id: &str,
) -> Result<(), String> {
    let detail = client
        .frontend_task(parse_id::<TaskId>(task_id)?)
        .await
        .map_err(code)?;
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_types::{
        Run, RunSpec, Task, TaskSpec,
        a2a::{A2aRunStart, RunWorkspace},
    };
    use vibemuxd::{WriterWorker, control::DaemonControlServer};

    async fn wait_snapshot(
        client: &mut FrontendClient,
        predicate: impl Fn(&SupervisorSnapshot) -> bool,
    ) -> SupervisorSnapshot {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Some(snapshot) = client.take_snapshot() {
                    if predicate(&snapshot) {
                        return snapshot;
                    }
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
        let mut client =
            FrontendClient::start(temp.path().into(), egui::Context::default()).unwrap();
        let snapshot = wait_snapshot(&mut client, |snapshot| {
            snapshot.connection.status == ConnectionStatus::Connected && snapshot.tasks.len() == 1
        })
        .await;
        assert_eq!(snapshot.tasks[0].title, "Observe real task");
        assert_eq!(snapshot.tasks[0].runs[0].run_id, run_id.to_string());
        let serialized = serde_json::to_string(&snapshot).unwrap();
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

    #[test]
    fn action_queue_is_bounded_and_does_not_invoke_the_runtime() {
        let (actions, _receiver) = mpsc::channel(1);
        let (_, snapshots) = watch::channel(SupervisorSnapshot::unavailable());
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
