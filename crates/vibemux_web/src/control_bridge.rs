//! Read-only projection from the daemon control pipe for the local web API.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{RwLock, watch};
use uuid::Uuid;
use vibemux_types::{TaskId, frontend::FrontendTasksQuery};
use vibemuxd::{
    control::{ControlClient, DaemonHealth},
    process::DaemonPaths,
};

use crate::content_label::{media_type_label, timestamp_label, web_label};

pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);
pub const MAX_BRIDGE_TASKS: usize = 256;
pub const MAX_TASK_TITLE_CHARS: usize = 160;
const MAX_CONTROL_PAGE: u32 = 32;
const REFRESH_DEADLINE: Duration = Duration::from_secs(30);

pub type SnapshotState = Arc<RwLock<BridgeSnapshot>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BridgeSnapshot {
    pub generation: Option<String>,
    pub observed_at: Option<String>,
    pub stale: bool,
    pub project_id: Option<String>,
    pub daemon: Option<BridgeHealth>,
    pub dsh_status: Option<BridgeDshStatus>,
    pub tasks: Vec<BridgeTask>,
    pub truncated: bool,
    /// Events and artifacts of each task in `tasks`, keyed by task id. Only
    /// the task detail route serves them.
    #[serde(default)]
    pub activity: BTreeMap<String, BridgeTaskActivity>,
}

impl Default for BridgeSnapshot {
    fn default() -> Self {
        Self {
            generation: None,
            observed_at: None,
            stale: true,
            project_id: None,
            daemon: None,
            dsh_status: None,
            tasks: Vec::new(),
            truncated: false,
            activity: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BridgeHealth {
    pub healthy: bool,
    pub store_schema_version: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BridgeDshStatus {
    pub phase: String,
    pub provider: String,
    pub model: String,
    pub turns_started: u64,
    pub completed_turns: u64,
    pub failed_turns: u64,
    pub cancelled_turns: u64,
    pub unverified_turns: u64,
    pub records: u64,
    pub transport_open: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BridgeTask {
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub kind: String,
    pub state: String,
    pub latest_sequence: u64,
    pub runs: Vec<BridgeRun>,
    pub dispatch: Option<BridgeLinkedStatus>,
    pub workflow: Option<BridgeLinkedStatus>,
}

/// A task's latest events and artifacts, without digests or worktree data.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BridgeTaskActivity {
    pub events: Vec<BridgeEvent>,
    pub artifacts: Vec<BridgeArtifact>,
    /// Whether the daemon cut the runs, events, or artifacts of the task.
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BridgeEvent {
    pub sequence: u64,
    pub event_type: String,
    pub at: String,
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BridgeArtifact {
    pub artifact_id: String,
    pub run_id: String,
    pub media_type: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BridgeLinkedStatus {
    pub id: String,
    pub state: String,
    pub sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrackedTaskLinks {
    pub task_id: String,
    pub dispatch_id: Option<String>,
    pub workflow_id: Option<String>,
}

/// Validate and canonicalize web-local resource links before any listener is opened.
pub fn normalize_task_links(
    mut task_links: Vec<TrackedTaskLinks>,
) -> Result<Vec<TrackedTaskLinks>, BridgeError> {
    if task_links.len() > 32 {
        return Err(BridgeError::InvalidLinks);
    }
    let mut task_ids = std::collections::HashSet::new();
    for link in &mut task_links {
        link.task_id = link
            .task_id
            .parse::<TaskId>()
            .map_err(|_| BridgeError::InvalidLinks)?
            .to_string();
        if !task_ids.insert(link.task_id.clone()) {
            return Err(BridgeError::InvalidLinks);
        }
        for value in [&mut link.dispatch_id, &mut link.workflow_id]
            .into_iter()
            .flatten()
        {
            *value = value
                .parse::<Uuid>()
                .map_err(|_| BridgeError::InvalidLinks)?
                .to_string();
        }
    }
    Ok(task_links)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BridgeRun {
    pub run_id: String,
    pub harness: String,
    pub role: String,
    pub state: String,
    pub latest_sequence: u64,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum BridgeError {
    #[error("daemon is unavailable")]
    DaemonUnavailable,
    #[error("control request failed")]
    ControlUnavailable,
    #[error("control refresh exceeded its deadline")]
    Deadline,
    #[error("control response exceeded the bridge bound")]
    Capacity,
    #[error("daemon generation changed during the snapshot")]
    GenerationChanged,
    #[error("tracked task links are invalid")]
    InvalidLinks,
}

impl BridgeError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::ControlUnavailable => "control_unavailable",
            Self::Deadline => "bridge_deadline_exceeded",
            Self::Capacity => "bridge_capacity_exceeded",
            Self::GenerationChanged => "daemon_generation_changed",
            Self::InvalidLinks => "tracked_task_links_invalid",
        }
    }
}

pub struct BridgePoller {
    paths: DaemonPaths,
    poll_interval: Duration,
    last_client: Option<ControlClient>,
    task_links: Vec<TrackedTaskLinks>,
}

impl BridgePoller {
    #[must_use]
    pub fn new(paths: DaemonPaths, poll_interval: Duration) -> Self {
        Self {
            paths,
            poll_interval: if poll_interval.is_zero() {
                DEFAULT_POLL_INTERVAL
            } else {
                poll_interval
            },
            last_client: None,
            task_links: Vec::new(),
        }
    }

    pub fn with_task_links(
        mut self,
        task_links: Vec<TrackedTaskLinks>,
    ) -> Result<Self, BridgeError> {
        self.task_links = normalize_task_links(task_links)?;
        Ok(self)
    }

    pub async fn refresh(&mut self) -> Result<BridgeSnapshot, BridgeError> {
        tokio::time::timeout(REFRESH_DEADLINE, self.refresh_inner())
            .await
            .map_err(|_| BridgeError::Deadline)?
    }

    pub async fn run(mut self, snapshot_state: SnapshotState, mut stop: watch::Receiver<bool>) {
        let mut interval = tokio::time::interval(self.poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() { break; }
                }
                _ = interval.tick() => {
                    let result = tokio::select! {
                        biased;
                        changed = stop.changed() => {
                            if changed.is_err() || *stop.borrow() { break; }
                            continue;
                        }
                        result = self.refresh() => result,
                    };
                    match result {
                        Ok(snapshot) => *snapshot_state.write().await = snapshot,
                        Err(_) => {
                            let mut snapshot = snapshot_state.write().await;
                            snapshot.stale = true;
                            for task in &mut snapshot.tasks {
                                task.title = None;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn refresh_inner(&mut self) -> Result<BridgeSnapshot, BridgeError> {
        let paths = self.paths.clone();
        let (client, descriptor_generation) =
            tokio::task::spawn_blocking(move || load_validated_client(&paths))
                .await
                .map_err(|_| BridgeError::DaemonUnavailable)??;
        let generation_changed = self
            .last_client
            .as_ref()
            .is_some_and(|previous| !previous.same_generation(&client));
        let health = client
            .health()
            .await
            .map_err(|_| BridgeError::ControlUnavailable)?;
        if !health.healthy {
            return Err(BridgeError::ControlUnavailable);
        }
        let generation = generation_for(&health, &descriptor_generation);
        let dsh_status = match client.dsh_session_status().await {
            Ok(status) => Some(project_dsh_status(status)),
            Err(error)
                if matches!(
                    error.code(),
                    "dsh_session_not_found" | "dsh_session_unconfigured"
                ) =>
            {
                None
            }
            Err(_) => return Err(BridgeError::ControlUnavailable),
        };
        let mut tasks = Vec::new();
        let mut activity = BTreeMap::new();
        let mut after = None;
        let mut truncated = false;
        let mut project_id = None;
        while tasks.len() < MAX_BRIDGE_TASKS {
            let query = FrontendTasksQuery {
                after,
                limit: MAX_CONTROL_PAGE.min((MAX_BRIDGE_TASKS - tasks.len()) as u32),
            };
            let page = client
                .frontend_tasks(query)
                .await
                .map_err(|_| BridgeError::ControlUnavailable)?;
            let page_project_id = page.project_id.map(|id| id.to_string());
            if let Some(observed_project_id) = page_project_id {
                if project_id
                    .as_ref()
                    .is_some_and(|previous| previous != &observed_project_id)
                {
                    return Err(BridgeError::ControlUnavailable);
                }
                project_id = Some(observed_project_id);
            }
            let ids: Vec<TaskId> = page.tasks.iter().map(|task| task.task_id).collect();
            for id in ids {
                if let Some(detail) = client
                    .frontend_task(id)
                    .await
                    .map_err(|_| BridgeError::ControlUnavailable)?
                {
                    let task_activity = project_activity(&detail);
                    let mut projected = project_task(detail);
                    if let Some(link) = self
                        .task_links
                        .iter()
                        .find(|link| link.task_id == projected.task_id)
                    {
                        if let Some(dispatch_id) = &link.dispatch_id {
                            let status = client
                                .harness_dispatch_status(
                                    dispatch_id.parse().map_err(|_| BridgeError::InvalidLinks)?,
                                )
                                .await
                                .map_err(|_| BridgeError::ControlUnavailable)?;
                            if status.task_id.to_string() != projected.task_id {
                                return Err(BridgeError::ControlUnavailable);
                            }
                            projected.dispatch = Some(BridgeLinkedStatus {
                                id: dispatch_id.clone(),
                                state: format!("{:?}", status.phase).to_ascii_lowercase(),
                                sequence: Some(status.version),
                            });
                        }
                        if let Some(workflow_id) = &link.workflow_id {
                            let workflow_id_parsed = workflow_id
                                .parse::<Uuid>()
                                .map_err(|_| BridgeError::InvalidLinks)?;
                            let status = client
                                .workflow_status(workflow_id_parsed)
                                .await
                                .map_err(|_| BridgeError::ControlUnavailable)?;
                            if status.get("workflow_id").and_then(|value| value.as_str())
                                != Some(workflow_id.as_str())
                            {
                                return Err(BridgeError::ControlUnavailable);
                            }
                            projected.workflow = Some(BridgeLinkedStatus {
                                id: workflow_id.clone(),
                                state: status
                                    .get("phase")
                                    .and_then(|value| value.as_str())
                                    .ok_or(BridgeError::ControlUnavailable)?
                                    .to_owned(),
                                sequence: status.get("version").and_then(|value| value.as_u64()),
                            });
                        }
                    }
                    activity.insert(projected.task_id.clone(), task_activity);
                    tasks.push(projected);
                    if tasks.len() >= MAX_BRIDGE_TASKS {
                        break;
                    }
                }
            }
            if let Some(cursor) = page.next_cursor {
                after = Some(cursor);
                if tasks.len() >= MAX_BRIDGE_TASKS {
                    truncated = true;
                    break;
                }
            } else {
                break;
            }
        }
        if tasks.len() > MAX_BRIDGE_TASKS {
            return Err(BridgeError::Capacity);
        }
        let descriptor_path = self.paths.descriptor_path().to_owned();
        let observed_generation =
            tokio::task::spawn_blocking(move || descriptor_fingerprint(&descriptor_path))
                .await
                .map_err(|_| BridgeError::DaemonUnavailable)?
                .map_err(|_| BridgeError::DaemonUnavailable)?;
        if observed_generation != descriptor_generation {
            return Err(BridgeError::GenerationChanged);
        }
        self.last_client = Some(client);
        Ok(BridgeSnapshot {
            generation: Some(generation),
            observed_at: Some(now_rfc3339()),
            stale: false,
            project_id,
            daemon: Some(BridgeHealth {
                healthy: health.healthy,
                store_schema_version: health.store_schema_version,
            }),
            dsh_status,
            tasks,
            truncated: truncated && !generation_changed,
            activity,
        })
    }
}

pub(crate) fn load_validated_client(
    paths: &DaemonPaths,
) -> Result<(ControlClient, String), BridgeError> {
    if !paths.descriptor_path().exists() {
        return Err(BridgeError::DaemonUnavailable);
    }
    paths
        .validate_runtime_dir()
        .map_err(|_| BridgeError::DaemonUnavailable)?;
    #[cfg(windows)]
    paths
        .verify_control_security()
        .map_err(|_| BridgeError::DaemonUnavailable)?;
    let descriptor_generation = descriptor_fingerprint(paths.descriptor_path())?;
    let client = ControlClient::from_descriptor(paths.descriptor_path())
        .map_err(|_| BridgeError::DaemonUnavailable)?;
    if descriptor_fingerprint(paths.descriptor_path())? != descriptor_generation {
        return Err(BridgeError::GenerationChanged);
    }
    Ok((client, descriptor_generation))
}

pub(crate) fn descriptor_fingerprint(path: &std::path::Path) -> Result<String, BridgeError> {
    let contents = crate::private_state::read_bounded(path, 64 * 1024)
        .map_err(|_| BridgeError::DaemonUnavailable)?;
    Ok(hex(&Sha256::digest(contents)))
}

fn generation_for(health: &DaemonHealth, descriptor_generation: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"vibemux_web daemon generation v1\0");
    hasher.update(descriptor_generation.as_bytes());
    hasher.update(health.process_id.to_be_bytes());
    hasher.update(health.store_schema_version.to_be_bytes());
    let digest = hasher.finalize();
    hex(&digest[..16])
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

pub(crate) fn project_dsh_status(status: vibemuxd::control::DshSessionStatus) -> BridgeDshStatus {
    BridgeDshStatus {
        phase: format!("{:?}", status.phase).to_ascii_lowercase(),
        provider: sanitize_label(&status.provider),
        model: sanitize_label(&status.model),
        turns_started: status.turns_started,
        completed_turns: status.completed_turns,
        failed_turns: status.failed_turns,
        cancelled_turns: status.cancelled_turns,
        unverified_turns: status.unverified_turns,
        records: status.records,
        transport_open: status.transport_open,
    }
}

fn project_task(detail: vibemux_types::frontend::FrontendTaskDetail) -> BridgeTask {
    let task = detail.task;
    let events = detail.events;
    BridgeTask {
        task_id: task.task_id.to_string(),
        title: Some(truncate_chars(&task.title, MAX_TASK_TITLE_CHARS)),
        kind: "task".into(),
        state: task.status.as_str().into(),
        latest_sequence: task.latest_sequence,
        runs: detail
            .runs
            .into_iter()
            .map(|run| BridgeRun {
                run_id: run.run_id.to_string(),
                harness: sanitize_label(&run.harness),
                role: sanitize_label(&run.role),
                state: run.status.as_str().into(),
                latest_sequence: events
                    .iter()
                    .filter(|event| {
                        event
                            .run_id
                            .as_ref()
                            .is_some_and(|id| id.to_string() == run.run_id.to_string())
                    })
                    .map(|event| event.sequence)
                    .max()
                    .unwrap_or(task.latest_sequence),
            })
            .collect(),
        dispatch: None,
        workflow: None,
    }
}

fn project_activity(detail: &vibemux_types::frontend::FrontendTaskDetail) -> BridgeTaskActivity {
    BridgeTaskActivity {
        events: detail
            .events
            .iter()
            .map(|event| BridgeEvent {
                sequence: event.sequence,
                event_type: web_label(&event.event_type),
                at: timestamp_label(&event.at),
                run_id: event.run_id.as_ref().map(ToString::to_string),
            })
            .collect(),
        artifacts: detail
            .artifacts
            .iter()
            .map(|artifact| BridgeArtifact {
                artifact_id: artifact.artifact_id.to_string(),
                run_id: artifact.run_id.to_string(),
                media_type: media_type_label(&artifact.media_type),
                size_bytes: artifact.size_bytes,
            })
            .collect(),
        truncated: detail.has_more_runs || detail.has_more_events || detail.has_more_artifacts,
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut output: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        output.push('…');
    }
    output
}

fn sanitize_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .take(48)
        .collect()
}

pub(crate) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemuxd::control::DaemonHealth;

    #[test]
    fn generation_is_opaque_stable_and_bound_to_descriptor_identity() {
        let health = DaemonHealth {
            healthy: true,
            process_id: 42,
            store_schema_version: 5,
            queue_capacity: 16,
            queue_depth: 0,
            queue_high_watermark: 0,
            queue_saturated: false,
        };
        let first = generation_for(&health, "opaque descriptor identity");
        assert_eq!(first, generation_for(&health, "opaque descriptor identity"));
        assert_ne!(
            first,
            generation_for(&health, "replacement descriptor identity")
        );
        assert_eq!(first.len(), 32);
        assert_ne!(first, health.process_id.to_string());
    }

    #[test]
    fn title_truncation_keeps_utf8_boundaries() {
        let title = "é".repeat(MAX_TASK_TITLE_CHARS + 10);
        let truncated = truncate_chars(&title, MAX_TASK_TITLE_CHARS);
        assert_eq!(truncated.chars().count(), MAX_TASK_TITLE_CHARS + 1);
        assert!(truncated.ends_with('…'));
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn task_links_require_unique_valid_ids_and_bound_count() {
        let temp = tempfile::tempdir().expect("temporary project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        let task_id = Uuid::new_v4().to_string();
        let valid = BridgePoller::new(paths.clone(), Duration::from_secs(5)).with_task_links(vec![
            TrackedTaskLinks {
                task_id: task_id.clone(),
                dispatch_id: Some(Uuid::new_v4().to_string()),
                workflow_id: None,
            },
        ]);
        assert!(valid.is_ok());
        let duplicate =
            BridgePoller::new(paths.clone(), Duration::from_secs(5)).with_task_links(vec![
                TrackedTaskLinks {
                    task_id: task_id.clone(),
                    dispatch_id: None,
                    workflow_id: None,
                },
                TrackedTaskLinks {
                    task_id,
                    dispatch_id: None,
                    workflow_id: None,
                },
            ]);
        assert_eq!(duplicate.err(), Some(BridgeError::InvalidLinks));
        let invalid = BridgePoller::new(paths, Duration::from_secs(5)).with_task_links(vec![
            TrackedTaskLinks {
                task_id: "not-a-task-id".into(),
                dispatch_id: None,
                workflow_id: None,
            },
        ]);
        assert_eq!(invalid.err(), Some(BridgeError::InvalidLinks));
    }
}
