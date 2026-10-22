#![forbid(unsafe_code)]
//! Reusable overview/activity/artifact/terminal task detail view.

use eframe::egui::{self, RichText, Ui};
use vibemux_harness::terminal_text::display_line;

use crate::supervisor_model::{
    RunView, SupervisorAction, SupervisorSnapshot, TaskView, TerminalCandidate,
};

use super::{
    C,
    chat::{demo_badge, wrapped_label},
    design,
    supervisor_state::{TaskDetailSelection, TaskDetailTab, UiActionQueue},
    task_labels::{executor_label, nonempty, run_label, single_run_label, task_state, task_title},
};

/// Whether the detail view draws the task title itself (detached window) or
/// leaves it to the drawer header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TitleDisplay {
    Shown,
    InHeader,
}

const DETAIL_LABEL_WIDTH: f32 = 76.0;

pub fn render(
    ui: &mut Ui,
    colors: &C,
    snapshot: &SupervisorSnapshot,
    task: &TaskView,
    selection: &mut TaskDetailSelection,
    actions: &UiActionQueue,
    title: TitleDisplay,
) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if snapshot.mode == crate::supervisor_model::SnapshotMode::Demo {
                demo_badge(ui, colors);
                ui.add_space(8.0);
            }
            if title == TitleDisplay::Shown {
                wrapped_label(
                    ui,
                    RichText::new(task_title(&task.title))
                        .font(super::typography::display_font(
                            ui.ctx(),
                            super::typography::TITLE_SIZE,
                        ))
                        .color(colors.txt),
                );
                ui.add_space(3.0);
            }
            ui.horizontal_wrapped(|ui| {
                design::status(ui, colors, task_state(&task.state));
                ui.label(
                    RichText::new(executor_label(task))
                        .size(13.0)
                        .color(colors.muted),
                );
            });
            ui.add_space(12.0);
            select_run(ui, colors, task, selection);
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                for (tab, label) in [
                    (TaskDetailTab::Overview, "Overview"),
                    (TaskDetailTab::Activity, "Activity"),
                    (TaskDetailTab::Artifacts, "Artifacts"),
                ] {
                    let selected = selection.tab == tab;
                    let response = ui.add(
                        egui::Button::new(RichText::new(label).size(13.0).color(if selected {
                            colors.accent_text
                        } else {
                            colors.muted
                        }))
                        .frame(false),
                    );
                    if selected {
                        ui.painter().hline(
                            response.rect.x_range(),
                            response.rect.bottom() + 3.0,
                            egui::Stroke::new(2.0, colors.accent),
                        );
                    }
                    if response.clicked() {
                        selection.tab = tab;
                    }
                }
            });
            ui.separator();
            ui.add_space(8.0);

            let run = selection.selected_run(task);
            match selection.tab {
                TaskDetailTab::Overview => render_overview(ui, colors, task, run, actions),
                TaskDetailTab::Activity => render_activity(ui, colors, task, run),
                TaskDetailTab::Artifacts => render_artifacts(ui, colors, task, run),
            }
        });
}

fn select_run(ui: &mut Ui, colors: &C, task: &TaskView, selection: &mut TaskDetailSelection) {
    match task.runs.as_slice() {
        [] => {
            ui.label(
                RichText::new("No Run is recorded for this task.")
                    .size(12.0)
                    .color(colors.muted),
            );
        }
        [run] => {
            wrapped_label(
                ui,
                RichText::new(single_run_label(run))
                    .size(12.0)
                    .color(colors.muted),
            );
            selection.selected_run_id = Some(run.run_id.clone());
        }
        runs => {
            let selected_id = selection
                .selected_run_id
                .as_deref()
                .filter(|id| runs.iter().any(|run| run.run_id == *id))
                .or_else(|| runs.first().map(|run| run.run_id.as_str()))
                .unwrap_or_default()
                .to_owned();
            let selected_label = runs
                .iter()
                .find(|run| run.run_id == selected_id)
                .map_or_else(|| "Select Run".to_string(), run_label);
            egui::ComboBox::from_id_salt(("task_run", task.task_id.as_str()))
                .selected_text(selected_label)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for run in runs {
                        if ui
                            .selectable_label(run.run_id == selected_id, run_label(run))
                            .clicked()
                        {
                            selection.selected_run_id = Some(run.run_id.clone());
                        }
                    }
                });
        }
    }
}

fn render_overview(
    ui: &mut Ui,
    colors: &C,
    task: &TaskView,
    run: Option<&RunView>,
    actions: &UiActionQueue,
) {
    section_label(ui, colors, "LATEST UPDATE");
    wrapped_label(
        ui,
        RichText::new(nonempty(
            &task.latest_update,
            "No event summary is available yet.",
        ))
        .size(13.0)
        .color(colors.txt),
    );
    ui.add_space(16.0);
    section_label(ui, colors, "RUN DETAILS");
    if let Some(run) = run {
        detail_grid(
            ui,
            colors,
            "run_details_grid",
            &[
                ("Harness", nonempty(&run.harness, "Unknown").to_string()),
                ("Role", nonempty(&run.role, "Not recorded").to_string()),
                ("Run state", design::state_text(&run.state)),
                ("Branch", nonempty(&run.branch, "Not recorded").to_string()),
                (
                    "Worktree",
                    nonempty(&run.worktree, "Not recorded").to_string(),
                ),
            ],
        );
    } else {
        wrapped_label(
            ui,
            RichText::new("Run details will appear when a Run is recorded.")
                .size(12.0)
                .color(colors.muted),
        );
    }
    if let Some(run) = run {
        ui.add_space(16.0);
        render_linked_terminal(ui, colors, task, run, actions);
    }
    ui.add_space(16.0);
    egui::CollapsingHeader::new("Identity & references").show(ui, |ui| {
        let mut rows = vec![("Task", task.task_id.clone())];
        if let Some(run) = run {
            rows.push(("Run", run.run_id.clone()));
            if let Some(binding_id) = run.terminal.binding_id.as_deref() {
                rows.push(("Terminal binding", binding_id.to_string()));
            }
        }
        detail_grid(ui, colors, "identity_grid", &rows);
    });
    ui.add_space(24.0);
    render_activity(ui, colors, task, run);
    if task.has_more_runs {
        ui.add_space(12.0);
        wrapped_label(
            ui,
            RichText::new("Showing the 12 most recent Runs.")
                .size(12.0)
                .color(colors.faint),
        );
    }
}

fn render_activity(ui: &mut Ui, colors: &C, task: &TaskView, run: Option<&RunView>) {
    section_label(ui, colors, "RECENT ACTIVITY");
    if task.recent_events.is_empty() {
        ui.label(
            RichText::new("No activity events are available.")
                .size(12.0)
                .color(colors.muted),
        );
        return;
    }
    for event in task.recent_events.iter().filter(|event| {
        run.is_none_or(|run| event.run_id.as_deref().is_none_or(|id| id == run.run_id))
    }) {
        egui::Frame::new()
            .fill(colors.raised)
            .stroke(egui::Stroke::NONE)
            .corner_radius(design::CARD_RADIUS)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if !event.occurred_at.trim().is_empty() {
                        ui.label(
                            RichText::new(event.occurred_at.as_str())
                                .size(12.0)
                                .color(colors.faint),
                        );
                    }
                    if !event.kind.trim().is_empty() {
                        ui.label(
                            RichText::new(event.kind.as_str())
                                .size(12.0)
                                .color(colors.muted),
                        );
                    }
                });
                ui.add_space(4.0);
                wrapped_label(
                    ui,
                    RichText::new(nonempty(&event.summary, "No event summary was supplied."))
                        .size(13.0)
                        .color(colors.txt),
                );
            });
        ui.add_space(7.0);
    }
    if task.has_more_events {
        ui.add_space(6.0);
        wrapped_label(
            ui,
            RichText::new("Showing the 24 most recent events.")
                .size(12.0)
                .color(colors.faint),
        );
    }
}

fn render_artifacts(ui: &mut Ui, colors: &C, task: &TaskView, run: Option<&RunView>) {
    section_label(ui, colors, "ARTIFACTS");
    let Some(run) = run else {
        ui.label(
            RichText::new("Select a Run to inspect its artifacts.")
                .size(12.0)
                .color(colors.muted),
        );
        return;
    };
    if run.artifacts.is_empty() {
        ui.label(
            RichText::new("No artifacts are recorded for this Run.")
                .size(12.0)
                .color(colors.muted),
        );
        return;
    }
    for artifact in &run.artifacts {
        egui::Frame::group(ui.style())
            .fill(colors.surf)
            .stroke(egui::Stroke::new(1.0, colors.border))
            .corner_radius(design::CARD_RADIUS)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                wrapped_label(
                    ui,
                    RichText::new(nonempty(&artifact.name, "Unnamed artifact"))
                        .size(13.0)
                        .strong()
                        .color(colors.txt),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(nonempty(&artifact.kind, "Unknown type"))
                            .size(12.0)
                            .color(colors.muted),
                    );
                    ui.label(
                        RichText::new(nonempty(&artifact.state, "Unknown state"))
                            .size(12.0)
                            .color(colors.faint),
                    );
                });
            });
        ui.add_space(7.0);
    }
    if task.has_more_artifacts {
        ui.add_space(6.0);
        wrapped_label(
            ui,
            RichText::new("Task artifact list is limited to 24 references.")
                .size(12.0)
                .color(colors.faint),
        );
    }
}

/// The selected Run's link to a native terminal window, folded into the
/// Overview: its status, Focus and Unlink while linked, a check for
/// terminals, and a Link card for each reported candidate.
fn render_linked_terminal(
    ui: &mut Ui,
    colors: &C,
    task: &TaskView,
    run: &RunView,
    actions: &UiActionQueue,
) {
    section_label(ui, colors, "LINKED TERMINAL");
    let terminal = &run.terminal;
    let linked = terminal.binding_id.as_deref();
    detail_row_prose(
        ui,
        colors,
        "Status",
        nonempty(
            &terminal.status,
            if linked.is_some() {
                "Linked"
            } else {
                "Not linked"
            },
        ),
    );
    if let Some(checked_at) = terminal.checked_at.as_deref() {
        detail_row(ui, colors, "Checked", checked_at);
    }
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if let Some(binding_id) = linked {
            if design::action_button(ui, colors, "Focus", true).clicked() {
                actions.enqueue(SupervisorAction::FocusTerminal {
                    binding_id: binding_id.to_string(),
                });
            }
            if design::action_button(ui, colors, "Unlink", true).clicked() {
                actions.enqueue(SupervisorAction::UnlinkTerminal {
                    binding_id: binding_id.to_string(),
                });
            }
        }
        if design::action_button(ui, colors, "Find terminals", true).clicked() {
            actions.enqueue(SupervisorAction::InspectTerminal {
                task_id: task.task_id.clone(),
                run_id: run.run_id.clone(),
            });
        }
    });
    if !terminal.candidates.is_empty() {
        ui.add_space(8.0);
        for candidate in &terminal.candidates {
            terminal_candidate(ui, colors, &task.task_id, &run.run_id, candidate, actions);
        }
    }
    ui.add_space(6.0);
    wrapped_label(
        ui,
        RichText::new("Terminal contents stay in the terminal window; this shows the link only.")
            .size(12.0)
            .color(colors.faint),
    );
}

fn terminal_candidate(
    ui: &mut Ui,
    colors: &C,
    task_id: &str,
    run_id: &str,
    candidate: &TerminalCandidate,
    actions: &UiActionQueue,
) {
    egui::Frame::group(ui.style())
        .fill(colors.surf)
        .stroke(egui::Stroke::new(1.0, colors.border))
        .corner_radius(design::CARD_RADIUS)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            wrapped_label(
                ui,
                RichText::new(nonempty(&candidate.display_name, "Terminal candidate"))
                    .size(13.0)
                    .strong()
                    .color(colors.txt),
            );
            for (label, value) in candidate_rows(candidate) {
                detail_row(ui, colors, label, &value);
            }
            let link = design::action_button(
                ui,
                colors,
                "Link terminal",
                candidate_is_linkable(candidate),
            )
            .on_hover_text("Link this exact reported terminal candidate to the selected Run.");
            if link.clicked() {
                actions.enqueue(SupervisorAction::LinkTerminal {
                    task_id: task_id.to_string(),
                    run_id: run_id.to_string(),
                    plugin_id: candidate.plugin_id.clone(),
                    pane_id: candidate.pane_id.clone(),
                    instance_id: candidate.instance_id.clone(),
                });
            }
        });
    ui.add_space(7.0);
}

fn detail_grid(ui: &mut Ui, colors: &C, id: &str, rows: &[(&str, String)]) {
    let value_width = (ui.available_width() - DETAIL_LABEL_WIDTH - 16.0).max(80.0);
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([16.0, 7.0])
        .min_col_width(DETAIL_LABEL_WIDTH)
        .max_col_width(value_width)
        .show(ui, |ui| {
            for (label, value) in rows {
                ui.label(RichText::new(*label).size(12.0).color(colors.muted));
                wrapped_label(
                    ui,
                    RichText::new(value.as_str())
                        .size(12.0)
                        .color(colors.txt)
                        .monospace(),
                );
                ui.end_row();
            }
        });
}

fn detail_row(ui: &mut Ui, colors: &C, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(label).size(12.0).color(colors.muted));
        ui.add_space(8.0);
        wrapped_label(
            ui,
            RichText::new(value)
                .size(12.0)
                .color(colors.txt)
                .monospace(),
        );
    });
    ui.add_space(4.0);
}

/// A label/value row whose value is prose rather than an identifier.
fn detail_row_prose(ui: &mut Ui, colors: &C, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(label).size(12.0).color(colors.muted));
        ui.add_space(8.0);
        wrapped_label(ui, RichText::new(value).size(13.0).color(colors.txt));
    });
    ui.add_space(4.0);
}

fn section_label(ui: &mut Ui, colors: &C, label: &str) {
    ui.label(RichText::new(label).size(11.0).strong().color(colors.faint));
    ui.add_space(5.0);
}

/// The identifier rows of a terminal candidate card, as display lines. A
/// terminal plugin's instance ID only has to pass the protocol's control
/// character check, so the card draws a mapped copy; the link request still
/// sends the identifiers exactly as reported.
fn candidate_rows(candidate: &TerminalCandidate) -> [(&'static str, String); 3] {
    [
        ("Plugin", display_line(&candidate.plugin_id)),
        ("Pane", display_line(&candidate.pane_id)),
        ("Instance", display_line(&candidate.instance_id)),
    ]
}

/// Linking needs the exact reported candidate, so every identifier must be
/// present; one that is empty or only whitespace counts as missing.
fn candidate_is_linkable(candidate: &TerminalCandidate) -> bool {
    [
        &candidate.plugin_id,
        &candidate.pane_id,
        &candidate.instance_id,
    ]
    .iter()
    .all(|id| !id.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_candidate_is_linkable_only_with_every_identifier() {
        let candidate = |plugin_id: &str, pane_id: &str, instance_id: &str| TerminalCandidate {
            plugin_id: plugin_id.to_string(),
            pane_id: pane_id.to_string(),
            instance_id: instance_id.to_string(),
            ..TerminalCandidate::default()
        };
        assert!(candidate_is_linkable(&candidate("wezterm", "3", "gui_1")));
        for missing in [
            candidate("", "3", "gui_1"),
            candidate("wezterm", " ", "gui_1"),
            candidate("wezterm", "3", "\t"),
        ] {
            assert!(!candidate_is_linkable(&missing), "{missing:?}");
        }
    }

    #[test]
    fn candidate_rows_draw_a_display_copy_of_each_identifier() {
        let candidate = TerminalCandidate {
            plugin_id: "wezterm_observer".to_string(),
            pane_id: "3".to_string(),
            instance_id: "gui\u{2028}1\u{202e}fdp.exe".to_string(),
            ..TerminalCandidate::default()
        };
        assert_eq!(
            candidate_rows(&candidate),
            [
                ("Plugin", "wezterm_observer".to_string()),
                ("Pane", "3".to_string()),
                ("Instance", "gui 1 fdp.exe".to_string()),
            ]
        );
        // The candidate a link request copies from is left as reported.
        assert_eq!(candidate.instance_id, "gui\u{2028}1\u{202e}fdp.exe");
    }
}
