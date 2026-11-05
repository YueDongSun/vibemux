//! Process-local ownership for bounded, read-only task views.
use crate::{
    command_error::CommandError,
    project_registry::RegisteredProject,
    read_contract::{TaskReadQuery, TaskReadResult},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;
use vibemux_types::{ProjectId, TaskId};

pub const MAX_TASK_WINDOWS: usize = 16;

#[derive(Clone)]
pub struct TaskWindowBinding {
    pub project_key: String,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub(crate) project: Arc<RegisteredProject>,
}

#[derive(Clone)]
pub struct TaskWindowReservation {
    pub label: String,
    pub generation: Uuid,
    pub fresh: bool,
    pub binding: TaskWindowBinding,
}

struct Entry {
    reservation: TaskWindowReservation,
    ready: bool,
}

#[derive(Default)]
pub struct TaskWindowRegistry {
    entries: Mutex<HashMap<String, Entry>>,
}

impl TaskWindowRegistry {
    /// Reserve before native creation; no project or window I/O under this lock.
    pub fn reserve(
        &self,
        binding: TaskWindowBinding,
    ) -> Result<TaskWindowReservation, CommandError> {
        let mut entries = self.entries.lock().map_err(|_| unavailable())?;
        if let Some(entry) = entries.values().find(|entry| {
            entry.reservation.binding.project_key == binding.project_key
                && entry.reservation.binding.task_id == binding.task_id
        }) {
            if !entry.ready {
                return Err(CommandError::new(
                    "task_window_busy",
                    "The task window is opening.",
                ));
            }
            if entry.reservation.binding.project_id != binding.project_id {
                return Err(denied());
            }
            let mut reservation = entry.reservation.clone();
            reservation.fresh = false;
            return Ok(reservation);
        }
        if entries.len() >= MAX_TASK_WINDOWS {
            return Err(CommandError::new(
                "task_window_capacity",
                "Close a task window before opening another.",
            ));
        }
        let generation = Uuid::new_v4();
        let reservation = TaskWindowReservation {
            label: format!("task_{}", generation.simple()),
            generation,
            fresh: true,
            binding,
        };
        entries.insert(
            reservation.label.clone(),
            Entry {
                reservation: reservation.clone(),
                ready: false,
            },
        );
        Ok(reservation)
    }

    pub fn mark_ready(&self, reservation: &TaskWindowReservation) -> Result<(), CommandError> {
        let mut entries = self.entries.lock().map_err(|_| unavailable())?;
        let entry = entries
            .get_mut(&reservation.label)
            .filter(|entry| entry.reservation.generation == reservation.generation)
            .ok_or_else(denied)?;
        entry.ready = true;
        Ok(())
    }

    pub fn authorize(
        &self,
        label: &str,
        key: &str,
        query: &TaskReadQuery,
    ) -> Result<TaskWindowReservation, CommandError> {
        let entries = self.entries.lock().map_err(|_| unavailable())?;
        let entry = entries.get(label).ok_or_else(denied)?;
        if entry.reservation.binding.project_key != key
            || !matches!(query, TaskReadQuery::Detail {task_id} if *task_id == entry.reservation.binding.task_id)
        {
            return Err(denied());
        }
        Ok(entry.reservation.clone())
    }

    pub fn validate_result(
        &self,
        reservation: &TaskWindowReservation,
        result: &TaskReadResult,
    ) -> Result<(), CommandError> {
        let entries = self.entries.lock().map_err(|_| unavailable())?;
        let current = entries.get(&reservation.label).ok_or_else(denied)?;
        if current.reservation.generation != reservation.generation
            || !matches!(result, TaskReadResult::Detail {task, ..} if
                task.task.task_id == reservation.binding.task_id &&
                task.task.project_id == reservation.binding.project_id)
        {
            return Err(denied());
        }
        Ok(())
    }

    /// A callback from an old window cannot revoke a later reservation.
    pub fn release(&self, label: &str, generation: Uuid) {
        if let Ok(mut entries) = self.entries.lock()
            && entries
                .get(label)
                .is_some_and(|entry| entry.reservation.generation == generation)
        {
            entries.remove(label);
        }
    }
}

pub(crate) fn denied() -> CommandError {
    CommandError::new(
        "task_window_binding_invalid",
        "This window is not bound to the requested task.",
    )
}
pub(crate) fn unavailable() -> CommandError {
    CommandError::new(
        "task_window_unavailable",
        "The task window is unavailable. Try opening it again.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        desktop_settings::DesktopSettings,
        project_registry::ProjectRegistry,
        read_contract::{TaskCard, TaskDetail},
    };
    use vibemux_types::TaskStatus;

    fn binding() -> (tempfile::TempDir, TaskWindowBinding) {
        let temp = tempfile::tempdir().expect("project");
        let project = ProjectRegistry::default()
            .register(temp.path(), &DesktopSettings::default())
            .expect("register");
        (
            temp,
            TaskWindowBinding {
                project_key: project.key.clone(),
                project_id: ProjectId::new(),
                task_id: TaskId::new(),
                project,
            },
        )
    }
    fn detail(binding: &TaskWindowBinding) -> TaskReadResult {
        TaskReadResult::Detail {
            observed_at: None,
            task: TaskDetail {
                task: TaskCard {
                    task_id: binding.task_id,
                    project_id: binding.project_id,
                    title: "Task".into(),
                    title_truncated: false,
                    status: TaskStatus::Open,
                    latest_sequence: 1,
                    detail_revision: 1,
                },
                runs: vec![],
                events: vec![],
                artifacts: vec![],
                has_more_runs: false,
                has_more_events: false,
                has_more_artifacts: false,
            },
        }
    }
    #[test]
    fn opening_is_reserved_reuse_is_stable_and_old_close_cannot_revoke() {
        let (_temp, binding) = binding();
        let registry = TaskWindowRegistry::default();
        let first = registry.reserve(binding.clone()).expect("opening");
        assert!(first.fresh);
        assert_eq!(
            registry.reserve(binding.clone()).err().expect("busy").code,
            "task_window_busy"
        );
        registry.mark_ready(&first).expect("ready");
        let reused = registry.reserve(binding.clone()).expect("reuse");
        assert!(!reused.fresh);
        assert_eq!(reused.label, first.label);
        registry.release(&first.label, Uuid::new_v4());
        assert!(
            registry
                .authorize(
                    &first.label,
                    &binding.project_key,
                    &TaskReadQuery::Detail {
                        task_id: binding.task_id
                    }
                )
                .is_ok()
        );
        registry.release(&first.label, first.generation);
        assert!(registry.validate_result(&first, &detail(&binding)).is_err());
        let reopened = registry.reserve(binding).expect("reopen");
        assert_ne!(reopened.label, first.label);
    }
    #[test]
    fn a_bound_window_refuses_lists_foreign_keys_tasks_and_canonical_projects() {
        let (_temp, binding) = binding();
        let registry = TaskWindowRegistry::default();
        let first = registry.reserve(binding.clone()).expect("opening");
        let query = TaskReadQuery::Detail {
            task_id: binding.task_id,
        };
        assert!(registry.authorize(&first.label, "foreign", &query).is_err());
        assert!(
            registry
                .authorize(
                    &first.label,
                    &binding.project_key,
                    &TaskReadQuery::Detail {
                        task_id: TaskId::new()
                    }
                )
                .is_err()
        );
        assert!(
            registry
                .authorize(
                    &first.label,
                    &binding.project_key,
                    &TaskReadQuery::List {
                        after: None,
                        limit: 32
                    }
                )
                .is_err()
        );
        assert!(registry.validate_result(&first, &detail(&binding)).is_ok());
        let mut foreign = binding.clone();
        foreign.project_id = ProjectId::new();
        assert!(registry.validate_result(&first, &detail(&foreign)).is_err());
        foreign = binding.clone();
        foreign.task_id = TaskId::new();
        assert!(registry.validate_result(&first, &detail(&foreign)).is_err());
    }
    #[test]
    fn capacity_is_bounded_and_failed_open_releases_its_slot() {
        let (_temp, binding) = binding();
        let registry = TaskWindowRegistry::default();
        let mut opened = Vec::new();
        for _ in 0..MAX_TASK_WINDOWS {
            let mut next = binding.clone();
            next.task_id = TaskId::new();
            opened.push(registry.reserve(next).expect("capacity"));
        }
        assert_eq!(
            registry.reserve(binding.clone()).err().expect("full").code,
            "task_window_capacity"
        );
        registry.release(&opened[0].label, opened[0].generation);
        assert!(registry.reserve(binding).is_ok());
    }
    #[test]
    fn concurrent_open_has_one_execution_owner() {
        let (_temp, binding) = binding();
        let registry = Arc::new(TaskWindowRegistry::default());
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let joins: Vec<_> = (0..2)
            .map(|_| {
                let registry = Arc::clone(&registry);
                let barrier = Arc::clone(&barrier);
                let binding = binding.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    registry.reserve(binding).is_ok()
                })
            })
            .collect();
        assert_eq!(
            joins
                .into_iter()
                .map(|join| join.join().expect("owner"))
                .filter(|opened| *opened)
                .count(),
            1
        );
    }
}
