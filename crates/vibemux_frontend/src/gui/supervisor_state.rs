#![forbid(unsafe_code)]
//! Frontend-only selection, viewport, and composer state.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::supervisor_model::{RunView, SupervisorAction, SupervisorSnapshot, TaskView};

pub const ACTION_QUEUE_CAPACITY: usize = 64;
pub const MAX_OPEN_TASK_WINDOWS: usize = 24;
pub const CHAT_DRAWER_WIDTH: f32 = 320.0;
pub const CHAT_DRAWER_DOCK_THRESHOLD: f32 = 1400.0;
pub const CHAT_UNAVAILABLE_REASON: &str =
    "Sending is unavailable because no continuous chat service is connected.";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MainPage {
    #[default]
    CoordinatorChat,
    Agents,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TaskDetailTab {
    #[default]
    Overview,
    Activity,
    Artifacts,
    Terminal,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskDetailSelection {
    pub selected_run_id: Option<String>,
    pub tab: TaskDetailTab,
}

impl TaskDetailSelection {
    fn for_task(task: &TaskView) -> Self {
        Self {
            selected_run_id: task.runs.first().map(|run| run.run_id.clone()),
            tab: TaskDetailTab::Overview,
        }
    }

    fn normalize_for(&mut self, task: &TaskView) {
        if self
            .selected_run_id
            .as_ref()
            .is_some_and(|run_id| task.runs.iter().any(|run| &run.run_id == run_id))
        {
            return;
        }
        self.selected_run_id = task.runs.first().map(|run| run.run_id.clone());
    }

    #[must_use]
    pub fn selected_run<'a>(&self, task: &'a TaskView) -> Option<&'a RunView> {
        self.selected_run_id
            .as_ref()
            .and_then(|run_id| task.runs.iter().find(|run| &run.run_id == run_id))
            .or_else(|| task.runs.first())
    }
}

/// Local UI state. Closing a native task window only removes its window
/// selection; it never changes the immutable supervisor snapshot.
#[derive(Clone, Debug, Default)]
pub struct SupervisorUiState {
    page: MainPage,
    selected_agent: Option<usize>,
    selected_task_id: Option<String>,
    details_open: bool,
    details: TaskDetailSelection,
    task_windows: BTreeSet<String>,
    task_window_selections: Arc<Mutex<HashMap<String, TaskDetailSelection>>>,
    composer_draft: String,
}

impl SupervisorUiState {
    #[must_use]
    pub fn page(&self) -> MainPage {
        self.page
    }

    pub fn show_coordinator_chat(&mut self) {
        self.page = MainPage::CoordinatorChat;
        self.selected_agent = None;
    }

    pub fn show_agents(&mut self) {
        self.page = MainPage::Agents;
    }

    #[must_use]
    pub fn selected_agent(&self) -> Option<usize> {
        self.selected_agent
    }

    pub fn select_agent(&mut self, agent_index: usize, agent_count: usize) -> bool {
        if agent_index >= agent_count {
            return false;
        }
        self.page = MainPage::Agents;
        self.selected_agent = Some(agent_index);
        true
    }

    pub fn return_to_agents_list(&mut self) {
        self.page = MainPage::Agents;
        self.selected_agent = None;
    }

    pub fn select_task(&mut self, snapshot: &SupervisorSnapshot, task_id: &str) -> bool {
        let Some(task) = snapshot.tasks.iter().find(|task| task.task_id == task_id) else {
            return false;
        };
        self.selected_task_id = Some(task_id.to_string());
        self.details.normalize_for(task);
        if self.details.selected_run_id.is_none() {
            self.details = TaskDetailSelection::for_task(task);
        }
        self.details_open = true;
        true
    }

    #[must_use]
    pub fn selected_task<'a>(&self, snapshot: &'a SupervisorSnapshot) -> Option<&'a TaskView> {
        self.selected_task_id
            .as_ref()
            .and_then(|task_id| snapshot.tasks.iter().find(|task| &task.task_id == task_id))
    }

    #[must_use]
    pub fn selected_task_id(&self) -> Option<&str> {
        self.selected_task_id.as_deref()
    }

    #[must_use]
    pub fn details_open(&self) -> bool {
        self.details_open
    }

    pub fn close_details(&mut self) {
        self.details_open = false;
    }

    #[must_use]
    pub fn details_selection(&self) -> &TaskDetailSelection {
        &self.details
    }

    pub fn details_selection_mut(&mut self) -> &mut TaskDetailSelection {
        &mut self.details
    }

    pub fn open_task_window(&mut self, snapshot: &SupervisorSnapshot, task_id: &str) -> bool {
        let Some(task) = snapshot.tasks.iter().find(|task| task.task_id == task_id) else {
            return false;
        };
        if !self.task_windows.contains(task_id) && self.task_windows.len() >= MAX_OPEN_TASK_WINDOWS
        {
            return false;
        }
        let Ok(mut selections) = self.task_window_selections.lock() else {
            return false;
        };
        let selection = selections
            .entry(task_id.to_string())
            .or_insert_with(|| TaskDetailSelection::for_task(task));
        selection.normalize_for(task);
        self.task_windows.insert(task_id.to_string());
        true
    }

    pub fn close_task_window(&mut self, task_id: &str) -> bool {
        if let Ok(mut selections) = self.task_window_selections.lock() {
            selections.remove(task_id);
        }
        self.task_windows.remove(task_id)
    }

    #[must_use]
    pub fn task_window_is_open(&self, task_id: &str) -> bool {
        self.task_windows.contains(task_id)
    }

    #[must_use]
    pub fn open_task_windows(&self) -> Vec<String> {
        self.task_windows.iter().cloned().collect()
    }

    #[must_use]
    pub fn task_window_selection(&self, task_id: &str) -> Option<TaskDetailSelection> {
        self.task_window_selections
            .lock()
            .ok()
            .and_then(|selections| selections.get(task_id).cloned())
    }

    pub(crate) fn with_task_window_selection_mut<R>(
        &self,
        task_id: &str,
        task: &TaskView,
        update: impl FnOnce(&mut TaskDetailSelection) -> R,
    ) -> Option<R> {
        let mut selections = self.task_window_selections.lock().ok()?;
        let selection = selections
            .entry(task_id.to_string())
            .or_insert_with(|| TaskDetailSelection::for_task(task));
        Some(update(selection))
    }

    pub fn set_composer_draft(&mut self, draft: String) {
        self.composer_draft = draft;
    }

    #[must_use]
    pub fn composer_draft(&self) -> &str {
        &self.composer_draft
    }

    #[must_use]
    pub const fn send_enabled(&self) -> bool {
        false
    }

    #[must_use]
    pub const fn send_disabled_reason(&self) -> &'static str {
        CHAT_UNAVAILABLE_REASON
    }

    /// A disabled send never mutates the draft or emits a fake message.
    pub fn try_send(&mut self) -> bool {
        false
    }

    pub fn reconcile_snapshot(&mut self, snapshot: &SupervisorSnapshot) {
        // Page omissions and temporary disconnects are not task deletions.
        // Only an explicit window-close action removes an existing viewport.
        if let Ok(mut selections) = self.task_window_selections.lock() {
            selections.retain(|task_id, _| self.task_windows.contains(task_id));
            for task in &snapshot.tasks {
                if let Some(selection) = selections.get_mut(&task.task_id) {
                    selection.normalize_for(task);
                }
            }
        }
        if self.selected_task(snapshot).is_none() {
            self.selected_task_id = None;
            self.details_open = false;
            self.details = TaskDetailSelection::default();
        } else if let Some(task) = self.selected_task(snapshot) {
            self.details.normalize_for(task);
        }
    }
}

/// Thread-safe, bounded action queue shared with deferred native windows.
#[derive(Clone, Debug)]
pub(crate) struct UiActionQueue {
    queue: Arc<Mutex<VecDeque<SupervisorAction>>>,
    closed_task_windows: Arc<Mutex<VecDeque<String>>>,
    overflowed: Arc<AtomicBool>,
}

impl Default for UiActionQueue {
    fn default() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::with_capacity(ACTION_QUEUE_CAPACITY))),
            closed_task_windows: Arc::new(Mutex::new(VecDeque::with_capacity(
                MAX_OPEN_TASK_WINDOWS,
            ))),
            overflowed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl UiActionQueue {
    pub(crate) fn enqueue(&self, action: SupervisorAction) -> bool {
        let Ok(mut queue) = self.queue.lock() else {
            self.overflowed.store(true, Ordering::Relaxed);
            return false;
        };
        if queue.len() >= ACTION_QUEUE_CAPACITY {
            self.overflowed.store(true, Ordering::Relaxed);
            return false;
        }
        queue.push_back(action);
        true
    }

    pub(crate) fn drain(&self) -> Vec<SupervisorAction> {
        let Ok(mut queue) = self.queue.lock() else {
            self.overflowed.store(true, Ordering::Relaxed);
            return Vec::new();
        };
        queue.drain(..).collect()
    }

    pub(crate) fn take_overflowed(&self) -> bool {
        self.overflowed.swap(false, Ordering::Relaxed)
    }

    pub(crate) fn report_task_window_closed(&self, task_id: String) {
        let Ok(mut closed) = self.closed_task_windows.lock() else {
            self.overflowed.store(true, Ordering::Relaxed);
            return;
        };
        if closed.len() < MAX_OPEN_TASK_WINDOWS && !closed.contains(&task_id) {
            closed.push_back(task_id);
        } else if !closed.contains(&task_id) {
            self.overflowed.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn take_closed_task_windows(&self) -> Vec<String> {
        let Ok(mut closed) = self.closed_task_windows.lock() else {
            self.overflowed.store(true, Ordering::Relaxed);
            return Vec::new();
        };
        closed.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_queue_is_bounded_and_reports_overflow() {
        let queue = UiActionQueue::default();
        for _ in 0..ACTION_QUEUE_CAPACITY {
            assert!(queue.enqueue(SupervisorAction::Refresh));
        }
        assert!(!queue.enqueue(SupervisorAction::Refresh));
        assert!(queue.take_overflowed());
        assert_eq!(queue.drain().len(), ACTION_QUEUE_CAPACITY);
    }
}
