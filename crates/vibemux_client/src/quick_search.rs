//! Quick switcher entries and ranking (ADR 030 §5). No egui dependency: the
//! GUI renders what this module ranks.
use crate::supervisor_model::TaskView;

/// Results the switcher shows.
pub const MAX_RESULTS: usize = 8;

/// The fixed actions, in the order the switcher lists them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuickAction {
    NewTask,
    Settings,
    Diagnostics,
    KeyboardShortcuts,
    ToggleSidebar,
    /// Selects `claude_light`.
    Light,
    /// Selects `claude`.
    Dark,
}

impl QuickAction {
    pub const ALL: [Self; 7] = [
        Self::NewTask,
        Self::Settings,
        Self::Diagnostics,
        Self::KeyboardShortcuts,
        Self::ToggleSidebar,
        Self::Light,
        Self::Dark,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NewTask => "New task",
            Self::Settings => "Settings",
            Self::Diagnostics => "Diagnostics",
            Self::KeyboardShortcuts => "Keyboard shortcuts",
            Self::ToggleSidebar => "Toggle sidebar",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchTarget {
    Task(String),
    /// Index into the probe view model's agents.
    Agent(usize),
    Action(QuickAction),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchEntry {
    pub label: String,
    /// Secondary text that also matches: a task's id and harnesses.
    pub detail: String,
    pub target: SearchTarget,
}

/// Tasks first, then the actions, then the agents.
#[must_use]
pub fn build_entries(tasks: &[TaskView], agent_names: &[String]) -> Vec<SearchEntry> {
    let tasks = tasks.iter().map(|task| {
        let mut detail = vec![task.task_id.clone(), task.executor.clone()];
        detail.extend(task.runs.iter().map(|run| run.harness.clone()));
        SearchEntry {
            label: task.title.clone(),
            detail: detail.join(" "),
            target: SearchTarget::Task(task.task_id.clone()),
        }
    });
    let actions = QuickAction::ALL.iter().map(|action| SearchEntry {
        label: action.label().to_string(),
        detail: "action".to_string(),
        target: SearchTarget::Action(*action),
    });
    let agents = agent_names
        .iter()
        .enumerate()
        .map(|(index, name)| SearchEntry {
            label: name.clone(),
            detail: "agent".to_string(),
            target: SearchTarget::Agent(index),
        });
    tasks.chain(actions).chain(agents).collect()
}

/// How well `query` (already lowercased and non-empty) matches: a label
/// prefix, then a word start in the label, then anywhere in the label, then
/// anywhere in the detail. `None` when it does not match.
fn match_rank(entry: &SearchEntry, query: &str) -> Option<u8> {
    let label = entry.label.to_lowercase();
    if label.starts_with(query) {
        return Some(0);
    }
    let word_start = label
        .split(|character: char| !character.is_alphanumeric())
        .any(|word| word.starts_with(query));
    if word_start {
        return Some(1);
    }
    if label.contains(query) {
        return Some(2);
    }
    entry.detail.to_lowercase().contains(query).then_some(3)
}

/// The best `MAX_RESULTS` entries for `query`, ignoring case; ties keep
/// source order. An empty query lists entries in source order.
#[must_use]
pub fn rank<'a>(entries: &'a [SearchEntry], query: &str) -> Vec<&'a SearchEntry> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return entries.iter().take(MAX_RESULTS).collect();
    }
    let mut ranked: Vec<(u8, &SearchEntry)> = entries
        .iter()
        .filter_map(|entry| match_rank(entry, &query).map(|rank| (rank, entry)))
        .collect();
    // `sort_by_key` is stable, so equal ranks keep source order.
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, entry)| entry)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor_model::{RunView, TaskView};

    fn task(task_id: &str, title: &str) -> TaskView {
        TaskView {
            task_id: task_id.to_string(),
            title: title.to_string(),
            executor: "Codex".to_string(),
            runs: vec![RunView {
                harness: "Codex CLI".to_string(),
                ..RunView::default()
            }],
            ..TaskView::default()
        }
    }

    fn labels<'a>(results: &[&'a SearchEntry]) -> Vec<&'a str> {
        results.iter().map(|entry| entry.label.as_str()).collect()
    }

    #[test]
    fn prefix_beats_word_start_beats_substring() {
        let entries = build_entries(
            &[
                task("task_1", "Reset cache"),
                task("task_2", "Fix settings page"),
            ],
            &[],
        );
        let results = rank(&entries, "set");
        assert_eq!(
            labels(&results),
            ["Settings", "Fix settings page", "Reset cache"]
        );
        assert_eq!(labels(&rank(&entries, "SETTINGS"))[0], "Settings");
    }

    #[test]
    fn ranking_handles_cjk_ids_and_caps_results() {
        let entries = build_entries(
            &[
                task("task_abc123", "修复登录页面"),
                task("task_2", "Update docs"),
            ],
            &[],
        );
        assert_eq!(labels(&rank(&entries, "修复")), ["修复登录页面"]);
        assert_eq!(labels(&rank(&entries, "登录")), ["修复登录页面"]);
        // Ids and harnesses match through the detail, below every label match.
        assert_eq!(labels(&rank(&entries, "abc123")), ["修复登录页面"]);
        assert_eq!(rank(&entries, "codex cli").len(), 2);
        let many: Vec<TaskView> = (0..20)
            .map(|index| task(&format!("task_{index}"), &format!("Task {index}")))
            .collect();
        let entries = build_entries(&many, &[]);
        let results = rank(&entries, "task");
        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(results[0].label, "Task 0", "ties keep source order");
        assert!(rank(&entries, "no such entry").is_empty());
    }

    #[test]
    fn empty_query_lists_tasks_then_actions_then_agents() {
        let agents = ["Claude".to_string()];
        let entries = build_entries(&[task("task_1", "First task")], &agents);
        let results = rank(&entries, "  ");
        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(results[0].target, SearchTarget::Task("task_1".to_string()));
        assert_eq!(
            results[1..]
                .iter()
                .map(|entry| entry.target.clone())
                .collect::<Vec<_>>(),
            QuickAction::ALL
                .iter()
                .map(|action| SearchTarget::Action(*action))
                .collect::<Vec<_>>()
        );
        let entries = build_entries(&[], &agents);
        let results = rank(&entries, "");
        assert_eq!(results.last().unwrap().target, SearchTarget::Agent(0));
    }
}
