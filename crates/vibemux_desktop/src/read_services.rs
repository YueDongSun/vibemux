//! Read-only desktop services. Call these blocking functions on an owned
//! blocking task, after resolving the renderer's registered project handle.
//! No database access, daemon start/recovery, probe refresh, or inference.

use std::{
    collections::BTreeSet,
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use vibemux_client::{
    daemon_model::{self, DecisionRow, DshView, HealthView, PluginRow, RouteRow, SlotsView},
    daemon_session::DaemonSession,
    display_text::{MAX_DISPLAY_CHARACTERS, safe_text},
    view_model::format_epoch_seconds,
    workspace_inventory::{self, Workspace},
};
use vibemux_harness::HarnessRow;
use vibemux_types::{
    TaskId,
    frontend::{
        FrontendTaskDetail, FrontendTaskSummary, FrontendTasksPage, FrontendTasksQuery,
        MAX_FRONTEND_ARTIFACTS, MAX_FRONTEND_EVENTS, MAX_FRONTEND_RUNS,
    },
};
use vibemuxd::{
    control::{ControlClient, ControlError, DecisionListQuery},
    process::DaemonPaths,
};

use crate::read_contract::*;

const READ_TIMEOUT: Duration = Duration::from_secs(2);

pub struct DecisionReadPage {
    pub rows: Vec<DecisionRow>,
    pub next_after_sequence: Option<u64>,
}

/// The complete effect boundary: every operation is a fixed read. It exposes
/// no generic method, credentials, path argument, mutation, or vendor launch.
pub trait ReadPort {
    fn task_page(&mut self, query: FrontendTasksQuery) -> Result<FrontendTasksPage, String>;
    fn task_detail(&mut self, task_id: TaskId) -> Result<Option<FrontendTaskDetail>, String>;
    fn health(&mut self) -> Result<HealthView, String>;
    fn harnesses(&mut self) -> Result<Vec<HarnessRow>, String>;
    fn plugins(&mut self) -> Result<Vec<PluginRow>, String>;
    fn routes(&mut self) -> Result<Vec<RouteRow>, String>;
    fn dsh(&mut self) -> Result<DshView, String>;
    fn slots(&mut self) -> Result<SlotsView, String>;
    fn decisions(&mut self, after_sequence: u64) -> Result<DecisionReadPage, String>;
    fn workspace(&mut self) -> Result<Workspace, String>;
}

struct SharedReadPort<'a> {
    paths: &'a DaemonPaths,
    runtime: tokio::runtime::Runtime,
    client: Option<Result<ControlClient, String>>,
}

impl<'a> SharedReadPort<'a> {
    fn new(paths: &'a DaemonPaths) -> Result<Self, ReadError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| {
                ReadError::new(
                    "desktop_runtime_unavailable",
                    "The local read runtime could not start.",
                )
            })?;
        Ok(Self {
            paths,
            runtime,
            client: None,
        })
    }

    fn client(&mut self) -> Result<ControlClient, String> {
        if self.client.is_none() {
            self.client = Some(
                DaemonSession::default()
                    .connect(self.paths)
                    .map(|(client, _)| client),
            );
        }
        self.client
            .as_ref()
            .ok_or_else(|| "read_operation_failed".to_owned())?
            .clone()
    }

    fn run<T>(&self, future: impl Future<Output = Result<T, ControlError>>) -> Result<T, String> {
        self.runtime.block_on(async {
            tokio::time::timeout(READ_TIMEOUT, future)
                .await
                .map_err(|_| "control_deadline_exceeded".to_owned())?
                .map_err(|error| error.code().to_owned())
        })
    }
}

impl ReadPort for SharedReadPort<'_> {
    fn task_page(&mut self, query: FrontendTasksQuery) -> Result<FrontendTasksPage, String> {
        let client = self.client()?;
        self.run(client.frontend_tasks(query))
    }
    fn task_detail(&mut self, task_id: TaskId) -> Result<Option<FrontendTaskDetail>, String> {
        let client = self.client()?;
        self.run(client.frontend_task(task_id))
    }
    fn health(&mut self) -> Result<HealthView, String> {
        let client = self.client()?;
        self.run(client.health_with_deadline(READ_TIMEOUT))
            .map(|value| daemon_model::project_health(&value))
    }
    fn harnesses(&mut self) -> Result<Vec<HarnessRow>, String> {
        let client = self.client()?;
        self.run(client.harness_snapshot())
    }
    fn plugins(&mut self) -> Result<Vec<PluginRow>, String> {
        let client = self.client()?;
        self.run(client.plugin_status())
            .map(|value| daemon_model::project_plugins(&value))
    }
    fn routes(&mut self) -> Result<Vec<RouteRow>, String> {
        let client = self.client()?;
        self.run(client.harness_dispatch_catalog())
            .map(|value| daemon_model::project_routes(&value))
    }
    fn dsh(&mut self) -> Result<DshView, String> {
        let client = self.client()?;
        self.run(client.dsh_session_status())
            .map(|value| daemon_model::project_dsh(&value))
    }
    fn slots(&mut self) -> Result<SlotsView, String> {
        let client = self.client()?;
        self.run(client.workflow_slots())
            .map(|value| daemon_model::project_slots(&value))
    }
    fn decisions(&mut self, after_sequence: u64) -> Result<DecisionReadPage, String> {
        let client = self.client()?;
        self.run(client.decision_list(DecisionListQuery {
            after_sequence,
            limit: DECISION_PAGE_LIMIT,
        }))
        .map(|page| DecisionReadPage {
            rows: page
                .decisions
                .iter()
                .map(daemon_model::project_decision)
                .collect(),
            next_after_sequence: page.next_after_sequence,
        })
    }
    fn workspace(&mut self) -> Result<Workspace, String> {
        workspace_inventory::load(self.paths.project_root())
    }
}

pub fn tasks_snapshot(
    paths: &DaemonPaths,
    query: TaskReadQuery,
) -> Result<TaskReadResult, ReadError> {
    query.validate()?;
    read_tasks(&mut SharedReadPort::new(paths)?, query)
}

pub fn console_section(
    paths: &DaemonPaths,
    query: ConsoleQuery,
) -> Result<ConsoleSnapshot, ReadError> {
    query.validate()?;
    read_console(&mut SharedReadPort::new(paths)?, query)
}

pub fn read_tasks(
    port: &mut impl ReadPort,
    query: TaskReadQuery,
) -> Result<TaskReadResult, ReadError> {
    query.validate()?;
    match query {
        TaskReadQuery::List { after, limit } => {
            let page = port
                .task_page(FrontendTasksQuery { after, limit })
                .map_err(|code| ReadError::unavailable(&code))?;
            let mut ids = BTreeSet::new();
            if page.tasks.len() > limit as usize
                || page.tasks.iter().any(|task| {
                    Some(task.project_id) != page.project_id || !ids.insert(task.task_id)
                })
                || page.next_cursor.is_some_and(|cursor| {
                    Some(cursor) == after || !page.tasks.iter().any(|task| task.task_id == cursor)
                })
            {
                return Err(invalid_contract());
            }
            Ok(TaskReadResult::Page {
                project_id: page.project_id,
                tasks: page.tasks.iter().map(task_card).collect(),
                next_cursor: page.next_cursor,
                observed_at: observed_at(),
            })
        }
        TaskReadQuery::Detail { task_id } => {
            let detail = port
                .task_detail(task_id)
                .map_err(|code| ReadError::unavailable(&code))?
                .ok_or_else(|| {
                    ReadError::new(
                        "frontend_task_not_found",
                        "This task is not available in the selected project.",
                    )
                })?;
            if detail.task.task_id != task_id
                || detail.runs.iter().any(|run| run.task_id != task_id)
            {
                return Err(invalid_contract());
            }
            Ok(TaskReadResult::Detail {
                task: project_detail(&detail),
                observed_at: observed_at(),
            })
        }
    }
}

pub fn read_console(
    port: &mut impl ReadPort,
    query: ConsoleQuery,
) -> Result<ConsoleSnapshot, ReadError> {
    query.validate()?;
    let mut next_after_sequence = None;
    let blocks = match query.section {
        ConsoleSection::Overview => vec![health_block(port)],
        ConsoleSection::Harnesses => vec![harness_block(port), route_block(port)],
        ConsoleSection::Workspace => workspace_blocks(port),
        ConsoleSection::Tasks => vec![task_block(port)],
        ConsoleSection::Daemon => vec![health_block(port), route_block(port), plugin_block(port)],
        ConsoleSection::Sessions => vec![dsh_block(port), slot_block(port)],
        ConsoleSection::Decisions => match port.decisions(query.after_sequence.unwrap_or(0)) {
            Err(code) => vec![unavailable_block("decisions", "Recorded decisions", &code)],
            Ok(page) => {
                if page.rows.len() > DECISION_PAGE_LIMIT
                    || page
                        .next_after_sequence
                        .is_some_and(|next| next <= query.after_sequence.unwrap_or(0))
                {
                    return Err(invalid_contract());
                }
                next_after_sequence = page.next_after_sequence;
                let rows = page
                    .rows
                    .iter()
                    .map(|row| {
                        row_cells(
                            row.created_sequence.to_string(),
                            vec![
                                row.decision_id.clone(),
                                row.status.clone(),
                                row.chosen.clone(),
                                row.source.clone(),
                                row.created.clone(),
                                if row.consumed {
                                    "Consumed".into()
                                } else {
                                    "Not consumed".into()
                                },
                            ],
                        )
                    })
                    .collect();
                vec![loaded_block(
                    "decisions",
                    "Recorded decisions",
                    &[
                        "ID prefix",
                        "Status",
                        "Harness",
                        "Source",
                        "Created",
                        "Consumption",
                    ],
                    rows,
                    Some(
                        "Read in ascending audit-sequence pages. Reading does not advise, consume, or run a decision.",
                    ),
                )]
            }
        },
    };
    Ok(ConsoleSnapshot {
        section: query.section,
        observed_at: observed_at(),
        blocks,
        next_after_sequence,
    })
}

fn task_card(task: &FrontendTaskSummary) -> TaskCard {
    TaskCard {
        task_id: task.task_id,
        project_id: task.project_id,
        title: safe_text(&task.title),
        title_truncated: task.title_truncated
            || task.title.chars().count() > MAX_DISPLAY_CHARACTERS,
        status: task.status,
        latest_sequence: task.latest_sequence,
        detail_revision: task.detail_revision,
    }
}

fn project_detail(detail: &FrontendTaskDetail) -> TaskDetail {
    TaskDetail {
        task: task_card(&detail.task),
        runs: detail
            .runs
            .iter()
            .take(MAX_FRONTEND_RUNS)
            .map(|run| RunFact {
                run_id: run.run_id,
                harness: safe_text(&run.harness),
                role: safe_text(&run.role),
                status: run.status,
                branch: run.branch.as_deref().map(safe_text),
                has_worktree: run.worktree_path.is_some(),
            })
            .collect(),
        events: detail
            .events
            .iter()
            .take(MAX_FRONTEND_EVENTS)
            .map(|event| EventFact {
                sequence: event.sequence,
                event_type: safe_text(&event.event_type),
                at: safe_text(&event.at),
                run_id: event.run_id,
            })
            .collect(),
        artifacts: detail
            .artifacts
            .iter()
            .take(MAX_FRONTEND_ARTIFACTS)
            .map(|artifact| ArtifactFact {
                artifact_id: artifact.artifact_id,
                run_id: artifact.run_id,
                size_bytes: artifact.size_bytes,
                media_type: safe_text(&artifact.media_type),
            })
            .collect(),
        has_more_runs: detail.has_more_runs || detail.runs.len() > MAX_FRONTEND_RUNS,
        has_more_events: detail.has_more_events || detail.events.len() > MAX_FRONTEND_EVENTS,
        has_more_artifacts: detail.has_more_artifacts
            || detail.artifacts.len() > MAX_FRONTEND_ARTIFACTS,
    }
}

fn health_block(port: &mut impl ReadPort) -> ConsoleBlock {
    match port.health() {
        Err(code) => unavailable_block("health", "Daemon health", &code),
        Ok(health) => loaded_block(
            "health",
            "Daemon health",
            &["Fact", "Observed value"],
            vec![
                row_cells("health", vec!["Health".into(), health.status]),
                row_cells("schema", vec!["Store schema".into(), health.schema]),
                row_cells("queue", vec!["Writer queue".into(), health.queue]),
            ],
            Some(
                "Authenticated daemon health. Queue activity or idle state does not establish task completion.",
            ),
        ),
    }
}

fn harness_block(port: &mut impl ReadPort) -> ConsoleBlock {
    match port.harnesses() {
        Err(code) => unavailable_block("harnesses", "Recorded harness detection", &code),
        Ok(rows) => loaded_block(
            "harnesses",
            "Recorded harness detection",
            &["Harness", "Detection", "Version", "Provider", "Default"],
            rows.iter()
                .map(|row| {
                    row_cells(
                        &row.name,
                        vec![
                            row.name.clone(),
                            if row.available {
                                "Detected".into()
                            } else {
                                "Not detected".into()
                            },
                            row.version.clone().unwrap_or_else(|| "Not recorded".into()),
                            row.provider.clone(),
                            if row.default {
                                "Yes".into()
                            } else {
                                "No".into()
                            },
                        ],
                    )
                })
                .collect(),
            Some(
                "This is stored detection metadata. It does not verify current authentication or successful inference; no probe is run by this view.",
            ),
        ),
    }
}

fn route_block(port: &mut impl ReadPort) -> ConsoleBlock {
    match port.routes() {
        Err(code) => unavailable_block("routes", "Configured dispatch routes", &code),
        Ok(rows) => loaded_block(
            "routes",
            "Configured dispatch routes",
            &["Harness", "Protocol", "Configured mode"],
            rows.into_iter()
                .enumerate()
                .map(|(index, row)| {
                    row_cells(index.to_string(), vec![row.harness, row.protocol, row.mode])
                })
                .collect(),
            Some(
                "Configured execution permission is not evidence of authentication, inference, or writable qualification.",
            ),
        ),
    }
}

fn plugin_block(port: &mut impl ReadPort) -> ConsoleBlock {
    match port.plugins() {
        Err(code) => unavailable_block("plugins", "Plugin status", &code),
        Ok(rows) => loaded_block(
            "plugins",
            "Plugin status",
            &["Plugin ID", "State", "Restarts", "Last error code"],
            rows.into_iter()
                .map(|row| {
                    row_cells(
                        &row.plugin_id,
                        vec![
                            row.plugin_id.clone(),
                            row.state,
                            row.restarts,
                            row.last_error,
                        ],
                    )
                })
                .collect(),
            None,
        ),
    }
}

fn task_block(port: &mut impl ReadPort) -> ConsoleBlock {
    match read_tasks(
        port,
        TaskReadQuery::List {
            after: None,
            limit: TASK_PAGE_LIMIT,
        },
    ) {
        Err(error) => unavailable_block("tasks", "Canonical tasks", &error.code),
        Ok(TaskReadResult::Page {
            tasks, next_cursor, ..
        }) => {
            let mut block = loaded_block(
                "tasks",
                "Canonical tasks",
                &["Task ID", "Title", "Status"],
                tasks
                    .into_iter()
                    .map(|task| ConsoleRow {
                        key: task.task_id.to_string(),
                        cells: vec![
                            task.task_id.to_string(),
                            task.title,
                            task.status.as_str().into(),
                        ],
                        task_id: Some(task.task_id),
                    })
                    .collect(),
                Some(
                    "Open Tasks for pagination and bounded task, run, event, and artifact details.",
                ),
            );
            block.truncated |= next_cursor.is_some();
            block
        }
        Ok(_) => unavailable_block("tasks", "Canonical tasks", "read_contract_invalid"),
    }
}

fn dsh_block(port: &mut impl ReadPort) -> ConsoleBlock {
    match port.dsh() {
        Err(code) => unavailable_block("dsh", "DSH session", &code),
        Ok(session) => loaded_block(
            "dsh",
            "DSH session",
            &["Fact", "Observed value"],
            vec![
                row_cells("phase", vec!["Phase".into(), session.phase]),
                row_cells("provider", vec!["Provider".into(), session.provider]),
                row_cells("model", vec!["Model".into(), session.model]),
                row_cells("turns", vec!["Turns".into(), session.turns]),
                row_cells(
                    "transport",
                    vec![
                        "Transport".into(),
                        if session.transport_open {
                            "Open".into()
                        } else {
                            "Closed".into()
                        },
                    ],
                ),
            ],
            Some(
                "The DSH session is a separate harness capability. Its open transport does not imply a completed turn.",
            ),
        ),
    }
}

fn slot_block(port: &mut impl ReadPort) -> ConsoleBlock {
    match port.slots() {
        Err(code) => unavailable_block("slots", "Workflow slots", &code),
        Ok(slots) => {
            let mut rows = vec![row_cells(
                "evidence",
                vec!["Evidence class".into(), slots.evidence_class],
            )];
            rows.extend(
                slots
                    .rows
                    .into_iter()
                    .enumerate()
                    .map(|(index, row)| row_cells(index.to_string(), vec!["Route".into(), row])),
            );
            loaded_block(
                "slots",
                "Workflow slots",
                &["Fact", "Observed value"],
                rows,
                None,
            )
        }
    }
}

fn workspace_blocks(port: &mut impl ReadPort) -> Vec<ConsoleBlock> {
    match port.workspace() {
        Err(code) => vec![unavailable_block("workspace", "Git workspace", &code)],
        Ok(workspace) => vec![
            loaded_block(
                "workspace",
                "Git workspace",
                &["Fact", "Observed value"],
                vec![
                    row_cells(
                        "branch",
                        vec!["Current branch".into(), workspace.current_branch],
                    ),
                    row_cells("head", vec!["HEAD prefix".into(), workspace.head]),
                    row_cells("baseline", vec!["Baseline".into(), workspace.baseline]),
                    row_cells(
                        "comparison",
                        vec![
                            "Cached upstream comparison".into(),
                            format!("{} ahead / {} behind", workspace.ahead, workspace.behind),
                        ],
                    ),
                    row_cells(
                        "changes",
                        vec![
                            "Working tree".into(),
                            format!(
                                "{} staged / {} unstaged / {} untracked",
                                workspace.staged, workspace.unstaged, workspace.untracked
                            ),
                        ],
                    ),
                    row_cells(
                        "worktrees",
                        vec![
                            "Registered worktrees".into(),
                            workspace.worktrees.len().to_string(),
                        ],
                    ),
                    row_cells(
                        "detection",
                        vec!["Change detection".into(), workspace.change_detection],
                    ),
                ],
                Some(
                    "Read through the existing workspace CLI using cached refs. No fetch, merge, cleanup, or worktree mutation occurs.",
                ),
            ),
            loaded_block(
                "branches",
                "Branches",
                &["Name", "Role", "Commit prefix", "Upstream"],
                workspace
                    .branches
                    .into_iter()
                    .enumerate()
                    .map(|(index, branch)| {
                        row_cells(
                            index.to_string(),
                            vec![branch.name, branch.role, branch.commit, branch.upstream],
                        )
                    })
                    .collect(),
                None,
            ),
        ],
    }
}

fn row_cells(key: impl ToString, cells: Vec<String>) -> ConsoleRow {
    ConsoleRow {
        key: safe_text(&key.to_string()),
        cells: cells.iter().map(|cell| safe_text(cell)).collect(),
        task_id: None,
    }
}

fn loaded_block(
    key: &str,
    title: &str,
    columns: &[&str],
    mut rows: Vec<ConsoleRow>,
    hint: Option<&str>,
) -> ConsoleBlock {
    let truncated = rows.len() > CONSOLE_ROW_LIMIT;
    rows.truncate(CONSOLE_ROW_LIMIT);
    ConsoleBlock {
        key: key.into(),
        title: title.into(),
        availability: ReadAvailability::Loaded,
        code: None,
        hint: hint.map(str::to_owned),
        columns: columns.iter().map(|column| (*column).to_owned()).collect(),
        rows,
        truncated,
    }
}

fn unavailable_block(key: &str, title: &str, code: &str) -> ConsoleBlock {
    let error = ReadError::unavailable(code);
    ConsoleBlock {
        key: key.into(),
        title: title.into(),
        availability: ReadAvailability::Unavailable,
        code: Some(error.code),
        hint: Some(error.message),
        columns: Vec::new(),
        rows: Vec::new(),
        truncated: false,
    }
}

fn observed_at() -> Option<String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| format_epoch_seconds(elapsed.as_secs()))
}
fn invalid_contract() -> ReadError {
    ReadError::new(
        "read_contract_invalid",
        "The daemon read returned inconsistent or oversized metadata.",
    )
}
