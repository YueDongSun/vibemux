//! Ephemeral, explicit observation leases. These never own task execution.

use crate::{WriterHandle, plugin_requests::PluginRequestClient};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use vibemux_plugin_protocol::{
    terminal::{FOCUS_METHOD, INVENTORY_METHOD, valid_inventory, valid_label, valid_pane_id},
    wire::{
        TerminalFocusRequest, TerminalFocusResult, TerminalInventory, TerminalInventoryRequest,
        TerminalPane,
    },
};
use vibemux_types::{ProjectId, RunId, TaskId};

const MAX_LINKS: usize = 128;
const LEASE_TTL: Duration = Duration::from_secs(30 * 60);

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
}

impl TerminalObserver {
    pub fn new(requests: PluginRequestClient) -> Self {
        Self {
            requests,
            leases: Mutex::new(BTreeMap::new()),
        }
    }

    pub async fn inspect(
        &self,
        writer: WriterHandle,
        query: TerminalQuery,
    ) -> Result<TerminalSnapshot, &'static str> {
        let expected_cwd = run_directory(writer, &query).await?;
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
        let expected_cwd = run_directory(writer, &request.query).await?;
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
        {
            if matching_lease(lease, &inventory, &session_id) && lease.binding.pane == pane {
                lease.touched_at = Instant::now();
                return Ok(lease.binding.clone());
            }
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
        if result.is_err() {
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
        let expected_cwd = run_directory(writer, query).await?;
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

async fn run_directory(
    writer: WriterHandle,
    query: &TerminalQuery,
) -> Result<String, &'static str> {
    if !valid_label(&query.plugin_id, 128) {
        return Err("terminal_invalid_request");
    }
    let query = query.clone();
    tokio::task::spawn_blocking(move || {
        let record = writer
            .a2a_run(query.run_id)
            .map_err(|_| "terminal_run_unavailable")?
            .ok_or("terminal_run_unavailable")?;
        if writer
            .project_id()
            .map_err(|_| "terminal_run_unavailable")?
            != Some(query.project_id)
            || record.run.project_id() != query.project_id
            || record.run.task_id() != query.task_id
        {
            return Err("terminal_project_mismatch");
        }
        let path = std::fs::canonicalize(&record.workspace.path)
            .map_err(|_| "terminal_workspace_unavailable")?;
        if !path.is_dir() {
            return Err("terminal_workspace_unavailable");
        }
        Ok(path.to_string_lossy().into_owned())
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
