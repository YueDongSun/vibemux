#![forbid(unsafe_code)]
//! Display labels for a task, shared by the chat task rows, the sidebar recents,
//! the task window, and the detail drawer so they all name one task the same
//! way. Text only: no egui, and nothing here is canonical state.

use vibemux_harness::terminal_text::display_character;

use crate::supervisor_model::{RunView, TaskView};

const UNTITLED_TASK: &str = "Untitled task";
const UNKNOWN_STATE: &str = "Unknown state";
const UNASSIGNED: &str = "Unassigned";
/// The task window's title before the snapshot names its task.
const UNOBSERVED_TASK: &str = "Task observation";
/// The most characters of a task title an operating-system window title shows.
const WINDOW_TITLE_CHARACTERS: usize = 80;

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

/// The operating-system title of a task window, for a task whose title the
/// snapshot may not hold yet. The title bar, taskbar, and window switcher draw
/// it outside egui and honour line breaks and bidirectional overrides, so the
/// task title becomes one line of display characters, cut at
/// `WINDOW_TITLE_CHARACTERS` with an ellipsis.
pub fn task_window_title(title: Option<&str>) -> String {
    let Some(title) = title else {
        return format!("{UNOBSERVED_TASK} · VibeMux");
    };
    let mut line: String = title
        .chars()
        .take(WINDOW_TITLE_CHARACTERS)
        .map(display_character)
        .collect();
    if title.chars().nth(WINDOW_TITLE_CHARACTERS).is_some() {
        line.push('…');
    }
    format!("{} · VibeMux", task_title(&line))
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

    #[test]
    fn a_window_title_is_one_bounded_line_of_display_characters() {
        assert_eq!(
            task_window_title(Some("Fix the parser")),
            "Fix the parser · VibeMux"
        );
        assert_eq!(task_window_title(None), "Task observation · VibeMux");
        // A line break, a tab, and a right-to-left override cannot reach the
        // title bar.
        assert_eq!(
            task_window_title(Some("Fix\nthe\tparser\u{202e}gpj.exe")),
            "Fix the parser gpj.exe · VibeMux"
        );
        // A title made only of whitespace or unsafe characters is blank.
        for blank in ["", "\u{202e}\u{2066}", "\r\n"] {
            assert_eq!(
                task_window_title(Some(blank)),
                "Untitled task · VibeMux",
                "{blank:?}"
            );
        }
        let exact = "a".repeat(WINDOW_TITLE_CHARACTERS);
        assert_eq!(
            task_window_title(Some(&exact)),
            format!("{exact} · VibeMux")
        );
        let long = format!("{exact}bcd");
        assert_eq!(
            task_window_title(Some(&long)),
            format!("{exact}… · VibeMux")
        );
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
