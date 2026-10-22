#![forbid(unsafe_code)]
//! Workspace: the active folder's branch, changes, branches, and worktrees,
//! as `vibemuxctl workspace status` reports them from cached refs.

use eframe::egui::Ui;

use crate::dashboard::Workspace;

use super::{
    super::{C, section_page},
    ConsoleActions, ConsoleView, WorkspaceLoad, notice, toolbar,
};

pub(super) fn render(ui: &mut Ui, c: &C, view: &ConsoleView<'_>, actions: &mut ConsoleActions) {
    if view.folder.is_none() {
        notice(ui, c, "Open a folder to see its Git workspace.");
        return;
    }
    let loading = matches!(view.data.workspace, WorkspaceLoad::Loading);
    let status = if loading {
        "Reading the Git inventory…"
    } else {
        "From cached refs; nothing is fetched"
    };
    if toolbar(ui, c, status, "Refresh", !loading) {
        actions.refresh = true;
    }
    match &view.data.workspace {
        WorkspaceLoad::NotLoaded | WorkspaceLoad::Loading => {
            ui.spinner();
        }
        WorkspaceLoad::Failed(code) => notice(ui, c, &failure_text(code)),
        WorkspaceLoad::Loaded(workspace) => render_inventory(ui, c, workspace),
    }
}

fn render_inventory(ui: &mut Ui, c: &C, workspace: &Workspace) {
    section_page::value_card(ui, c, &summary_rows(workspace));
    ui.add_space(20.0);
    section_page::sub_heading(ui, c, &format!("Branches ({})", workspace.branches.len()));
    if workspace.branches.is_empty() {
        notice(ui, c, "No local branches.");
    } else {
        section_page::card(ui, c, |ui| {
            for (index, branch) in workspace.branches.iter().enumerate() {
                if index > 0 {
                    section_page::separator(ui, c);
                }
                section_page::value_row(
                    ui,
                    c,
                    &branch.name,
                    &format!("{} · {} · {}", branch.role, branch.commit, branch.upstream),
                );
            }
        });
    }
    ui.add_space(20.0);
    section_page::sub_heading(ui, c, &format!("Worktrees ({})", workspace.worktrees.len()));
    if workspace.worktrees.is_empty() {
        notice(ui, c, "No worktrees.");
    } else {
        section_page::card(ui, c, |ui| {
            for (index, worktree) in workspace.worktrees.iter().enumerate() {
                if index > 0 {
                    section_page::separator(ui, c);
                }
                ui.add_space(10.0);
                section_page::footnote(ui, c, worktree);
                ui.add_space(10.0);
            }
        });
    }
}

/// The summary card's rows.
pub(super) fn summary_rows(workspace: &Workspace) -> Vec<(&'static str, String)> {
    vec![
        ("Branch", workspace.current_branch.clone()),
        ("HEAD", workspace.head.clone()),
        (
            "Against baseline",
            format!(
                "{} ahead, {} behind {}",
                workspace.ahead, workspace.behind, workspace.baseline
            ),
        ),
        ("Changes", changes_text(workspace)),
    ]
}

pub(super) fn changes_text(workspace: &Workspace) -> String {
    if workspace.staged + workspace.unstaged + workspace.untracked == 0 {
        return "Clean".to_string();
    }
    format!(
        "{} staged, {} unstaged, {} untracked",
        workspace.staged, workspace.unstaged, workspace.untracked
    )
}

pub(super) fn failure_text(code: &str) -> String {
    let hint = match code {
        "workspace_git_query_failed" | "workspace_unavailable" => {
            "Is this folder a Git repository?"
        }
        "vibemuxctl_unavailable" => {
            "vibemuxctl was not found next to the GUI; build it with cargo build -p vibemux_cli --bin vibemuxctl."
        }
        _ => "Run vibemuxctl workspace status in this folder for details.",
    };
    format!("Could not read the Git inventory ({code}). {hint}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> Workspace {
        Workspace {
            current_branch: "feat/console".into(),
            head: "b68f381f".into(),
            baseline: "origin/main".into(),
            ahead: 2,
            behind: 0,
            ..Workspace::default()
        }
    }

    #[test]
    fn the_summary_names_the_branch_baseline_and_changes() {
        let mut inventory = workspace();
        let rows = summary_rows(&inventory);
        assert_eq!(rows[0], ("Branch", "feat/console".to_string()));
        assert_eq!(
            rows[2],
            (
                "Against baseline",
                "2 ahead, 0 behind origin/main".to_string()
            )
        );
        assert_eq!(rows[3].1, "Clean");
        inventory.unstaged = 3;
        inventory.untracked = 1;
        assert_eq!(
            changes_text(&inventory),
            "0 staged, 3 unstaged, 1 untracked"
        );
    }

    #[test]
    fn failures_keep_the_code_and_say_what_to_check() {
        let text = failure_text("workspace_git_query_failed");
        assert!(text.contains("workspace_git_query_failed"));
        assert!(text.contains("Git repository"));
        assert!(failure_text("vibemuxctl_timeout").contains("vibemuxctl_timeout"));
    }
}
