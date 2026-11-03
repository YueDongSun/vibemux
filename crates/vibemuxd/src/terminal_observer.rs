//! Ephemeral, explicit observation leases. These never own task execution.

use crate::{WriterHandle, plugin_requests::PluginRequestClient};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use vibemux_plugin_protocol::{
    terminal::{
        CLEANUP_INVENTORY_METHOD, FOCUS_METHOD, INVENTORY_METHOD, valid_inventory, valid_label,
        valid_pane_id,
    },
    wire::{
        TerminalFocusRequest, TerminalFocusResult, TerminalInventory, TerminalInventoryRequest,
        TerminalPane,
    },
};
use vibemux_types::{ProjectId, RunId, TaskId};

const MAX_LINKS: usize = 128;
const LEASE_TTL: Duration = Duration::from_secs(30 * 60);

/// Private observation, never a deletion permit or evidence about other terminal
/// namespaces. Cleanup calls this under its already-held exclusive resource gate.
pub(crate) struct CleanupTerminalProof {
    pub(crate) plugin_id: String,
    pub(crate) session_id: String,
    pub(crate) instance_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalQuery {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub plugin_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalLinkRequest {
    pub query: TerminalQuery,
    pub instance_id: String,
    pub pane_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct TerminalBinding {
    pub binding_id: String,
    pub query: TerminalQuery,
    pub instance_id: String,
    pub pane: TerminalPane,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct TerminalSnapshot {
    pub inventory: TerminalInventory,
    pub binding: Option<TerminalBinding>,
    pub checked_at_epoch_ms: u64,
}

#[derive(Clone)]
struct ObservationLease {
    binding: TerminalBinding,
    plugin_session_id: String,
    touched_at: Instant,
}

pub struct TerminalObserver {
    requests: PluginRequestClient,
    leases: Mutex<BTreeMap<String, ObservationLease>>,
    project_root: PathBuf,
    workspace_jobs: crate::workspace_jobs::WorkspaceJobs,
}

impl TerminalObserver {
    pub(crate) async fn cleanup_clear(
        &self,
        plugin_id: &str,
        run_id: RunId,
        expected_cwd: &str,
    ) -> Result<CleanupTerminalProof, &'static str> {
        if !valid_label(plugin_id, 128)
            || !valid_label(expected_cwd, 4096)
            || !Path::new(expected_cwd).is_absolute()
        {
            return Err("terminal_invalid_request");
        }
        // Expiry is not absence proof. A fresh complete query is always required.
        let reply = self
            .requests
            .request(
                plugin_id,
                CLEANUP_INVENTORY_METHOD,
                TerminalInventoryRequest {
                    expected_cwd: expected_cwd.to_owned(),
                }
                .encode_to_vec(),
            )
            .await?;
        let inventory = TerminalInventory::decode(reply.payload.as_slice())
            .map_err(|_| "terminal_invalid_response")?;
        if !valid_inventory(&inventory) {
            return Err("terminal_invalid_response");
        }
        if !inventory.panes.is_empty() {
            return Err("workspace_cleanup_terminal_present");
        }
        if self
            .leases
            .lock()
            .map_err(|_| "terminal_observer_unavailable")?
            .values()
            .any(|lease| {
                lease.binding.query.run_id == run_id && lease.touched_at.elapsed() < LEASE_TTL
            })
        {
            return Err("workspace_cleanup_terminal_linked");
        }
        if self.requests.active_session(plugin_id).as_deref() != Some(reply.session_id.as_str()) {
            return Err("terminal_session_changed");
        }
        Ok(CleanupTerminalProof {
            plugin_id: plugin_id.to_owned(),
            session_id: reply.session_id,
            instance_id: inventory.instance_id,
        })
    }

    pub(crate) async fn recheck_cleanup_clear(
        &self,
        proof: &CleanupTerminalProof,
        run_id: RunId,
        expected_cwd: &str,
    ) -> Result<(), &'static str> {
        let current = self
            .cleanup_clear(&proof.plugin_id, run_id, expected_cwd)
            .await?;
        if current.session_id != proof.session_id {
            return Err("terminal_session_changed");
        }
        if current.instance_id != proof.instance_id {
            return Err("terminal_instance_changed");
        }
        Ok(())
    }

    pub fn new(requests: PluginRequestClient, project_root: PathBuf) -> Self {
        Self::with_workspace_jobs(
            requests,
            project_root,
            crate::workspace_jobs::WorkspaceJobs::new(),
        )
    }

    pub(crate) fn with_workspace_jobs(
        requests: PluginRequestClient,
        project_root: PathBuf,
        workspace_jobs: crate::workspace_jobs::WorkspaceJobs,
    ) -> Self {
        Self {
            requests,
            leases: Mutex::new(BTreeMap::new()),
            project_root,
            workspace_jobs,
        }
    }

    pub async fn inspect(
        &self,
        writer: WriterHandle,
        query: TerminalQuery,
    ) -> Result<TerminalSnapshot, &'static str> {
        let (_guard, expected_cwd) =
            run_directory_guarded(writer, &query, &self.project_root, &self.workspace_jobs).await?;
        let (inventory, session_id) = self.inventory(&query, &expected_cwd).await?;
        let mut leases = self
            .leases
            .lock()
            .map_err(|_| "terminal_observer_unavailable")?;
        leases.retain(|_, lease| {
            lease.touched_at.elapsed() < LEASE_TTL
                && (lease.binding.query != query || matching_lease(lease, &inventory, &session_id))
        });
        let binding = leases
            .values_mut()
            .find(|lease| lease.binding.query == query)
            .map(|lease| {
                lease.touched_at = Instant::now();
                lease.binding.clone()
            });
        Ok(TerminalSnapshot {
            inventory,
            binding,
            checked_at_epoch_ms: epoch_ms(),
        })
    }

    pub async fn link(
        &self,
        writer: WriterHandle,
        request: TerminalLinkRequest,
    ) -> Result<TerminalBinding, &'static str> {
        if !valid_label(&request.instance_id, 128) || !valid_pane_id(&request.pane_id) {
            return Err("terminal_invalid_request");
        }
        let (_guard, expected_cwd) = run_directory_guarded(
            writer,
            &request.query,
            &self.project_root,
            &self.workspace_jobs,
        )
        .await?;
        let (inventory, session_id) = self.inventory(&request.query, &expected_cwd).await?;
        if inventory.instance_id != request.instance_id {
            return Err("terminal_instance_changed");
        }
        let pane = inventory
            .panes
            .iter()
            .find(|pane| pane.pane_id == request.pane_id)
            .cloned()
            .ok_or("terminal_identity_mismatch")?;
        let mut leases = self
            .leases
            .lock()
            .map_err(|_| "terminal_observer_unavailable")?;
        leases.retain(|_, lease| lease.touched_at.elapsed() < LEASE_TTL);
        if let Some(lease) = leases
            .values_mut()
            .find(|lease| lease.binding.query == request.query)
            && matching_lease(lease, &inventory, &session_id)
            && lease.binding.pane == pane
        {
            lease.touched_at = Instant::now();
            return Ok(lease.binding.clone());
        }
        // Replacement is explicit and cannot create duplicate leases per Run.
        leases.retain(|_, lease| lease.binding.query != request.query);
        if leases.len() >= MAX_LINKS {
            return Err("terminal_observation_capacity");
        }
        let binding = TerminalBinding {
            binding_id: uuid::Uuid::new_v4().to_string(),
            query: request.query,
            instance_id: inventory.instance_id,
            pane,
        };
        leases.insert(
            binding.binding_id.clone(),
            ObservationLease {
                binding: binding.clone(),
                plugin_session_id: session_id,
                touched_at: Instant::now(),
            },
        );
        Ok(binding)
    }

    pub fn unlink(&self, binding_id: &str) -> Result<(), &'static str> {
        if uuid::Uuid::parse_str(binding_id).is_err() {
            return Err("terminal_invalid_request");
        }
        self.leases
            .lock()
            .map_err(|_| "terminal_observer_unavailable")?
            .remove(binding_id);
        Ok(())
    }

    pub async fn focus(&self, writer: WriterHandle, binding_id: &str) -> Result<(), &'static str> {
        let lease = self
            .leases
            .lock()
            .map_err(|_| "terminal_observer_unavailable")?
            .get(binding_id)
            .cloned()
            .ok_or("terminal_link_expired")?;
        let result = self.focus_lease(writer, &lease).await;
        if result
            .as_ref()
            .is_err_and(|code| !code.starts_with("workspace_operation_"))
        {
            self.unlink(binding_id)?;
        }
        result
    }

    async fn focus_lease(
        &self,
        writer: WriterHandle,
        lease: &ObservationLease,
    ) -> Result<(), &'static str> {
        if lease.touched_at.elapsed() >= LEASE_TTL {
            return Err("terminal_link_expired");
        }
        let query = &lease.binding.query;
        let (_guard, expected_cwd) =
            run_directory_guarded(writer, query, &self.project_root, &self.workspace_jobs).await?;
        let (inventory, session_id) = self.inventory(query, &expected_cwd).await?;
        if !matching_lease(lease, &inventory, &session_id) {
            return Err("terminal_identity_mismatch");
        }
        let request = TerminalFocusRequest {
            instance_id: lease.binding.instance_id.clone(),
            pane_id: lease.binding.pane.pane_id.clone(),
            expected_cwd,
        };
        let reply = self
            .requests
            .request(&query.plugin_id, FOCUS_METHOD, request.encode_to_vec())
            .await?;
        if reply.session_id != session_id {
            return Err("terminal_session_changed");
        }
        let result = TerminalFocusResult::decode(reply.payload.as_slice())
            .map_err(|_| "terminal_invalid_response")?;
        if !result.focused {
            return Err("terminal_focus_failed");
        }
        Ok(())
    }

    async fn inventory(
        &self,
        query: &TerminalQuery,
        expected_cwd: &str,
    ) -> Result<(TerminalInventory, String), &'static str> {
        let reply = self
            .requests
            .request(
                &query.plugin_id,
                INVENTORY_METHOD,
                TerminalInventoryRequest {
                    expected_cwd: expected_cwd.to_owned(),
                }
                .encode_to_vec(),
            )
            .await?;
        let inventory = TerminalInventory::decode(reply.payload.as_slice())
            .map_err(|_| "terminal_invalid_response")?;
        if !valid_inventory(&inventory) {
            return Err("terminal_invalid_response");
        }
        let paths: Vec<_> = inventory
            .panes
            .iter()
            .map(|pane| pane.cwd.clone())
            .collect();
        let expected_cwd = expected_cwd.to_owned();
        let matches = tokio::task::spawn_blocking(move || {
            paths
                .iter()
                .all(|path| same_file::is_same_file(path, &expected_cwd).unwrap_or(false))
        })
        .await
        .map_err(|_| "terminal_identity_mismatch")?;
        if !matches {
            return Err("terminal_identity_mismatch");
        }
        Ok((inventory, reply.session_id))
    }
}

enum DirectorySource {
    A2a(vibemux_types::a2a::RunWorkspace),
    WorkspaceOwner(Box<vibemux_types::chat_owner::ChatExecutionOwner>),
}
impl DirectorySource {
    fn same_binding(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::A2a(left), Self::A2a(right)) => left == right,
            (Self::WorkspaceOwner(left), Self::WorkspaceOwner(right)) => {
                left.project_id == right.project_id
                    && left.chat_id == right.chat_id
                    && left.task_id == right.task_id
                    && left.run_id == right.run_id
                    && left.workspace == right.workspace
                    && left.resource_key == right.resource_key
            }
            _ => false,
        }
    }
    async fn resource_key(&self) -> Result<String, &'static str> {
        match self {
            Self::WorkspaceOwner(owner) => owner
                .resource_key
                .clone()
                .ok_or("terminal_workspace_unavailable"),
            Self::A2a(workspace) => directory_resource_key(PathBuf::from(&workspace.path)).await,
        }
    }
    async fn directory(&self, project_root: &Path) -> Result<String, &'static str> {
        match self {
            Self::WorkspaceOwner(owner) => {
                crate::chat_workspace::inspect_owner_workspace(project_root, owner)
                    .await
                    .map(|path| path.to_string_lossy().into_owned())
                    .map_err(|_| "terminal_workspace_unavailable")
            }
            Self::A2a(workspace) => {
                let path = PathBuf::from(&workspace.path);
                tokio::task::spawn_blocking(move || {
                    let path = std::fs::canonicalize(path)
                        .map_err(|_| "terminal_workspace_unavailable")?;
                    if !path.is_dir() {
                        return Err("terminal_workspace_unavailable");
                    }
                    Ok(path.to_string_lossy().into_owned())
                })
                .await
                .map_err(|_| "terminal_workspace_unavailable")?
            }
        }
    }
}

async fn directory_resource_key(path: PathBuf) -> Result<String, &'static str> {
    tokio::task::spawn_blocking(move || {
        crate::harness_dispatch::HarnessDispatchService::workflow_resource_key(&path)
    })
    .await
    .map_err(|_| "terminal_workspace_unavailable")?
    .map(|(_, key)| key.to_hex())
    .ok_or("terminal_workspace_unavailable")
}

async fn run_directory_guarded(
    writer: WriterHandle,
    query: &TerminalQuery,
    project_root: &Path,
    jobs: &crate::workspace_jobs::WorkspaceJobs,
) -> Result<(tokio::sync::OwnedRwLockReadGuard<()>, String), &'static str> {
    let initial = load_directory_source(writer.clone(), query).await?;
    let key = initial.resource_key().await?;
    let guard = jobs.read(&key).await?;
    let current = load_directory_source(writer, query).await?;
    if !initial.same_binding(&current) {
        return Err("terminal_identity_mismatch");
    }
    let path = current.directory(project_root).await?;
    if directory_resource_key(PathBuf::from(&path)).await? != key {
        return Err("terminal_identity_mismatch");
    }
    Ok((guard, path))
}

#[cfg(test)]
async fn run_directory(
    writer: WriterHandle,
    query: &TerminalQuery,
    project_root: &Path,
) -> Result<String, &'static str> {
    load_directory_source(writer, query)
        .await?
        .directory(project_root)
        .await
}

async fn load_directory_source(
    writer: WriterHandle,
    query: &TerminalQuery,
) -> Result<DirectorySource, &'static str> {
    if !valid_label(&query.plugin_id, 128) {
        return Err("terminal_invalid_request");
    }
    let query = query.clone();
    tokio::task::spawn_blocking(move || {
        if writer
            .project_id()
            .map_err(|_| "terminal_run_unavailable")?
            != Some(query.project_id)
        {
            return Err("terminal_project_mismatch");
        }
        let run_id = query.run_id;
        // The full loader refuses corrupt owner projections. Never fall through
        // to another run domain when that authoritative read fails.
        let owner = writer
            .with_store(move |store| store.chat_execution_owner_for_run(run_id))
            .map_err(|_| "terminal_run_unavailable")?;
        if let Some(stored) = owner {
            if stored.owner.project_id != query.project_id || stored.owner.task_id != query.task_id
            {
                return Err("terminal_project_mismatch");
            }
            return Ok(DirectorySource::WorkspaceOwner(Box::new(stored.owner)));
        }
        let record = writer
            .a2a_run(query.run_id)
            .map_err(|_| "terminal_run_unavailable")?
            .ok_or("terminal_run_unavailable")?;
        if record.run.project_id() != query.project_id || record.run.task_id() != query.task_id {
            return Err("terminal_project_mismatch");
        }
        Ok(DirectorySource::A2a(record.workspace))
    })
    .await
    .map_err(|_| "terminal_run_unavailable")?
}

fn matching_lease(
    lease: &ObservationLease,
    inventory: &TerminalInventory,
    session_id: &str,
) -> bool {
    lease.plugin_session_id == session_id
        && lease.binding.instance_id == inventory.instance_id
        && inventory
            .panes
            .iter()
            .any(|pane| pane == &lease.binding.pane)
}

fn epoch_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod owner_directory_tests {
    use super::*;
    use crate::chat_workspace::tests::OwnedFixture;
    use vibemux_store::ChatOwnerChange;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workspace_owner_runs_resolve_without_a2a_and_refuse_foreign_or_finalized_scope() {
        let mut fixture = OwnedFixture::new().await;
        let query = TerminalQuery {
            project_id: fixture.owner.project_id,
            task_id: fixture.owner.task_id,
            run_id: fixture.owner.run_id,
            plugin_id: "wezterm_observer".into(),
        };
        let reader = fixture.writer.clone();
        let before = tokio::task::spawn_blocking(move || reader.with_store(|store| store.events()))
            .await
            .expect("read task")
            .expect("events");
        let resolved = run_directory(fixture.writer.clone(), &query, &fixture.root).await;
        let foreign_task = run_directory(
            fixture.writer.clone(),
            &TerminalQuery {
                task_id: TaskId::new(),
                ..query.clone()
            },
            &fixture.root,
        )
        .await;
        let foreign_project = run_directory(
            fixture.writer.clone(),
            &TerminalQuery {
                project_id: ProjectId::new(),
                ..query.clone()
            },
            &fixture.root,
        )
        .await;
        let reader = fixture.writer.clone();
        let after = tokio::task::spawn_blocking(move || reader.with_store(|store| store.events()))
            .await
            .expect("read task")
            .expect("events after observation");
        let expected = fixture.worktree().to_string_lossy().into_owned();
        fixture.change(ChatOwnerChange::Stop).await;
        let finalized = run_directory(fixture.writer.clone(), &query, &fixture.root).await;
        fixture.finish().await;
        assert_eq!(
            resolved.expect("owned workspace without A2A mapping"),
            expected
        );
        assert_eq!(foreign_task, Err("terminal_project_mismatch"));
        assert_eq!(foreign_project, Err("terminal_project_mismatch"));
        assert_eq!(
            before, after,
            "terminal resolution creates no canonical events"
        );
        assert_eq!(finalized, Err("terminal_workspace_unavailable"));
    }
}
