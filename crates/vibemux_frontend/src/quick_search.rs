#![forbid(unsafe_code)]
//! Local navigation over the currently loaded snapshot. No runtime side effects.

use crate::localization::Language;
use crate::{supervisor_model::SupervisorSnapshot, view_model::ViewModel};

pub(crate) const RESULT_LIMIT: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NavigationAction {
    Task(String),
    Agent(usize),
    Search,
    NewTask,
    Conversation,
    Agents,
    Settings,
    Diagnostics,
    Light,
    Dark,
    Shortcuts,
    ToggleSidebar,
}

#[derive(Clone, Debug)]
pub(crate) struct SearchEntry {
    pub label: String,
    pub detail: String,
    pub action: NavigationAction,
    keywords: String,
}

pub(crate) fn entries(
    snapshot: &SupervisorSnapshot,
    view_model: &ViewModel,
    language: Language,
) -> Vec<SearchEntry> {
    let mut entries: Vec<_> = snapshot
        .tasks
        .iter()
        .map(|task| SearchEntry {
            label: if task.title.trim().is_empty() {
                language.text("Untitled task").into()
            } else {
                task.title.clone()
            },
            detail: language.format(
                "Task · {executor} · {id}",
                &[("executor", &task.executor), ("id", &task.task_id)],
            ),
            action: NavigationAction::Task(task.task_id.clone()),
            keywords: format!(
                "{} {} {}",
                task.task_id,
                task.executor,
                task.runs
                    .iter()
                    .map(|run| run.harness.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        })
        .collect();
    for (label, action) in [
        ("New task", NavigationAction::NewTask),
        ("Conversation", NavigationAction::Conversation),
        ("Agents", NavigationAction::Agents),
        ("Settings", NavigationAction::Settings),
        ("Diagnostics", NavigationAction::Diagnostics),
        ("Light theme", NavigationAction::Light),
        ("Dark theme", NavigationAction::Dark),
        ("Keyboard shortcuts", NavigationAction::Shortcuts),
        ("Toggle sidebar", NavigationAction::ToggleSidebar),
    ] {
        entries.push(SearchEntry {
            label: language.text(label).into(),
            detail: language.text("Workspace action").into(),
            action,
            keywords: label.to_string(),
        });
    }
    entries.extend(
        view_model
            .agents
            .iter()
            .enumerate()
            .map(|(index, agent)| SearchEntry {
                label: agent.name.clone(),
                detail: language.text("Agent · inspect availability").into(),
                action: NavigationAction::Agent(index),
                keywords: String::new(),
            }),
    );
    entries
}

pub(crate) fn search<'a>(entries: &'a [SearchEntry], query: &str) -> Vec<&'a SearchEntry> {
    let query = query.trim().to_lowercase();
    let mut matches: Vec<_> = entries
        .iter()
        .filter_map(|entry| {
            let label = entry.label.to_lowercase();
            let rank = if query.is_empty() || label.starts_with(&query) {
                0
            } else if label
                .split(|character: char| !character.is_alphanumeric())
                .any(|word| word.starts_with(&query))
            {
                1
            } else if label.contains(&query) || entry.keywords.to_lowercase().contains(&query) {
                2
            } else {
                return None;
            };
            Some((rank, entry))
        })
        .collect();
    // Stable sorting keeps snapshot order for equally relevant entries.
    matches.sort_by_key(|(rank, _)| *rank);
    matches
        .into_iter()
        .take(RESULT_LIMIT)
        .map(|(_, entry)| entry)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str, keywords: &str) -> SearchEntry {
        SearchEntry {
            label: label.into(),
            detail: String::new(),
            action: NavigationAction::Settings,
            keywords: keywords.into(),
        }
    }

    #[test]
    fn prefix_then_word_then_substring_with_stable_ties() {
        let entries = vec![
            entry("Megafix", ""),
            entry("Apply fix", ""),
            entry("Fix first", ""),
            entry("Fix second", ""),
        ];
        let labels: Vec<_> = search(&entries, " FIX ")
            .into_iter()
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(labels, ["Fix first", "Fix second", "Apply fix", "Megafix"]);
    }

    #[test]
    fn searches_unicode_ids_and_harness_metadata_without_losing_text() {
        let entries = vec![entry("Überprüfung 東京", "task_9 Claude")];
        for query in ["ÜBER", "東京", "TASK_9", "claude"] {
            assert_eq!(search(&entries, query).len(), 1, "{query}");
        }
        assert!(search(&entries, "unmatched").is_empty());
    }

    #[test]
    fn empty_query_preserves_source_order_and_caps_results() {
        let entries: Vec<_> = (0..20)
            .map(|index| entry(&format!("Task {index}"), ""))
            .collect();
        let results = search(&entries, "  ");
        assert_eq!(results.len(), RESULT_LIMIT);
        assert_eq!(results[0].label, "Task 0");
        assert_eq!(results[7].label, "Task 7");
        assert!(search(&[], "").is_empty());
    }
}
