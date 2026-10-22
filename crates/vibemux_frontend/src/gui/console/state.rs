#![forbid(unsafe_code)]
//! What the Console shows besides the probe view model, and the reads it
//! asks the live adapter for. Every request is a read: a daemon feed poll,
//! a task detail, the Git inventory, or the read-only `--version` probe.

use std::sync::Arc;

use crate::dashboard::{DaemonView, Workspace};

/// The Git inventory of the active folder.
#[derive(Clone, Debug, Default)]
pub enum WorkspaceLoad {
    #[default]
    NotLoaded,
    Loading,
    Loaded(Workspace),
    /// The stable code `vibemuxctl` reported.
    Failed(String),
}

/// A re-probe the user asked for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ProbeJob {
    #[default]
    Idle,
    Running,
    Failed(String),
}

#[derive(Debug, Default)]
pub struct ConsoleData {
    /// The newest daemon feed observation; `None` before the first one.
    pub daemon: Option<Arc<DaemonView>>,
    pub workspace: WorkspaceLoad,
    pub probe: ProbeJob,
    refresh: bool,
    reload_workspace: bool,
    reprobe: bool,
    open_task: Option<String>,
}

impl ConsoleData {
    /// Polls the daemon now and rereads the Git inventory.
    pub fn request_refresh(&mut self) {
        self.refresh = true;
        self.reload_workspace = true;
    }

    pub fn take_refresh(&mut self) -> bool {
        std::mem::take(&mut self.refresh)
    }

    /// Whether the inventory should be read now: never read, or a reload
    /// was asked for while no read runs. Marks it loading.
    pub fn take_workspace_load(&mut self) -> bool {
        let reload = std::mem::take(&mut self.reload_workspace);
        let due = match self.workspace {
            WorkspaceLoad::Loading => false,
            WorkspaceLoad::NotLoaded => true,
            WorkspaceLoad::Loaded(_) | WorkspaceLoad::Failed(_) => reload,
        };
        if due {
            self.workspace = WorkspaceLoad::Loading;
        }
        due
    }

    pub fn set_workspace(&mut self, result: Result<Workspace, String>) {
        self.workspace = match result {
            Ok(workspace) => WorkspaceLoad::Loaded(workspace),
            Err(code) => WorkspaceLoad::Failed(code),
        };
    }

    /// Queues a re-probe unless one runs.
    pub fn request_reprobe(&mut self) {
        if self.probe != ProbeJob::Running {
            self.probe = ProbeJob::Running;
            self.reprobe = true;
        }
    }

    pub fn take_reprobe(&mut self) -> bool {
        std::mem::take(&mut self.reprobe)
    }

    pub fn finish_probe(&mut self, result: Result<(), String>) {
        self.probe = match result {
            Ok(()) => ProbeJob::Idle,
            Err(code) => ProbeJob::Failed(code),
        };
    }

    /// Asks the feed to read this task's detail on every poll.
    pub fn request_task(&mut self, task_id: String) {
        self.open_task = Some(task_id);
    }

    pub fn take_task(&mut self) -> Option<String> {
        self.open_task.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inventory_loads_once_and_again_only_on_refresh() {
        let mut data = ConsoleData::default();
        assert!(data.take_workspace_load(), "first open reads it");
        assert!(matches!(data.workspace, WorkspaceLoad::Loading));
        assert!(!data.take_workspace_load(), "never twice at once");
        data.set_workspace(Err("workspace_git_query_failed".into()));
        assert!(
            !data.take_workspace_load(),
            "a failure is not retried by itself"
        );
        data.request_refresh();
        assert!(data.take_refresh());
        assert!(data.take_workspace_load(), "Refresh rereads it");
        assert!(!data.take_refresh(), "taken once");
    }

    #[test]
    fn a_reprobe_runs_one_at_a_time() {
        let mut data = ConsoleData::default();
        data.request_reprobe();
        data.request_reprobe();
        assert!(data.take_reprobe());
        assert!(!data.take_reprobe());
        assert_eq!(data.probe, ProbeJob::Running);
        data.finish_probe(Err("frontend_worker_unavailable".into()));
        assert_eq!(
            data.probe,
            ProbeJob::Failed("frontend_worker_unavailable".into())
        );
        data.request_reprobe();
        assert!(data.take_reprobe(), "a failed probe can run again");
    }
}
