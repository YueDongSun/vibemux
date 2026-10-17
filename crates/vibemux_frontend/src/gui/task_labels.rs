#![forbid(unsafe_code)]
//! Display labels for a task, shared by the chat task rows, the sidebar recents,
//! the task window, and the detail drawer so they all name one task the same
//! way. Text only: no egui, and nothing here is canonical state.

use crate::supervisor_model::{RunView, TaskView};

const UNTITLED_TASK: &str = "Untitled task";
const UNKNOWN_STATE: &str = "Unknown state";
const UNASSIGNED: &str = "Unassigned";

/// `value`, or `fallback` when `value` is empty or only whitespace.
pub fn nonempty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

pub fn task_title(title: &str) -> &str {
    nonempty(title, UNTITLED_TASK)
}

pub fn task_state(state: &str) -> &str {
    nonempty(state, UNKNOWN_STATE)
}

/// The task's executor, else the first Run's harness, else `Unassigned`.
pub fn executor_label(task: &TaskView) -> &str {
    let first_harness = task.runs.first().map_or("", |run| run.harness.as_str());
    nonempty(&task.executor, nonempty(first_harness, UNASSIGNED))
}

/// The caption for a task's only Run.
pub fn single_run_label(run: &RunView) -> String {
    format!(
        "Run · {} · {}",
        nonempty(&run.harness, "Harness"),
        run.run_id
    )
}

/// One entry in the Run picker of a task with several Runs.
pub fn run_label(run: &RunView) -> String {
    format!(
        "{} · {} · {}",
        nonempty(&run.harness, "Harness"),
        nonempty(&run.role, "Run"),
        run.run_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(executor: &str, harnesses: &[&str]) -> TaskView {
        TaskView {
            executor: executor.to_string(),
            runs: harnesses
                .iter()
                .map(|harness| RunView {
                    harness: (*harness).to_string(),
                    ..RunView::default()
                })
                .collect(),
            ..TaskView::default()
        }
    }

    #[test]
    fn blank_text_falls_back_and_real_text_is_kept_verbatim() {
        for blank in ["", " ", "\t\n "] {
            assert_eq!(task_title(blank), "Untitled task");
            assert_eq!(task_state(blank), "Unknown state");
        }
        assert_eq!(task_title(" Fix the parser "), " Fix the parser ");
        assert_eq!(task_state("in_progress"), "in_progress");
        assert_eq!(nonempty("x", "fallback"), "x");
    }

    #[test]
    fn the_executor_falls_back_to_the_first_run_harness_then_unassigned() {
        assert_eq!(
            executor_label(&task("codex · worker", &["claude"])),
            "codex · worker"
        );
        assert_eq!(executor_label(&task("", &["claude", "codex"])), "claude");
        assert_eq!(executor_label(&task("  ", &["claude"])), "claude");
        assert_eq!(executor_label(&task("", &[" ", "codex"])), "Unassigned");
        assert_eq!(executor_label(&task("", &[])), "Unassigned");
    }

    #[test]
    fn run_labels_name_a_blank_harness_or_role_by_its_fallback() {
        let run = |harness: &str, role: &str| RunView {
            run_id: "run_1".to_string(),
            harness: harness.to_string(),
            role: role.to_string(),
            ..RunView::default()
        };
        assert_eq!(
            single_run_label(&run("codex", "worker")),
            "Run · codex · run_1"
        );
        assert_eq!(
            single_run_label(&run(" ", "worker")),
            "Run · Harness · run_1"
        );
        assert_eq!(run_label(&run("codex", "worker")), "codex · worker · run_1");
        assert_eq!(run_label(&run("", "\t")), "Harness · Run · run_1");
    }

    /// The views call these helpers instead of keeping their own copy of a
    /// fallback, which is how the chat task rows and the detail drawer once
    /// labelled the same task differently.
    #[test]
    fn views_do_not_restate_the_fallback_labels() {
        for (view, source) in [
            ("chat.rs", include_str!("chat.rs")),
            ("sidebar.rs", include_str!("sidebar.rs")),
            ("supervisor_app.rs", include_str!("supervisor_app.rs")),
            ("task_detail.rs", include_str!("task_detail.rs")),
        ] {
            for label in [UNTITLED_TASK, UNASSIGNED] {
                assert!(!source.contains(&format!("\"{label}\"")), "{view}: {label}");
            }
        }
    }
}
