#![forbid(unsafe_code)]
//! Reusable overview/activity/artifact/terminal task detail view.

use eframe::egui::{self, RichText, Ui};

use crate::supervisor_model::{
    RunView, SupervisorAction, SupervisorSnapshot, TaskView, TerminalCandidate,
};

use super::{
    C,
    chat::{demo_badge, wrapped_label},
    design,
    supervisor_state::{TaskDetailSelection, TaskDetailTab, UiActionQueue},
};

pub fn render(
    ui: &mut Ui,
    colors: &C,
    snapshot: &SupervisorSnapshot,
    task: &TaskView,
    selection: &mut TaskDetailSelection,
    actions: &UiActionQueue,
) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if snapshot.mode == crate::supervisor_model::SnapshotMode::Demo {
                demo_badge(ui, colors);
                ui.add_space(8.0);
            }
            wrapped_label(
                ui,
                RichText::new(task_title(task))
                    .size(24.0)
                    .strong()
                    .color(colors.txt),
            );
            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                design::status(ui, colors, task_state(task));
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
                    (TaskDetailTab::Terminal, "Terminal"),
                ] {
                    if ui
                        .add(
                            egui::Button::new(RichText::new(label).size(13.0).color(
                                if selection.tab == tab {
                                    colors.accent
                                } else {
                                    colors.muted
                                },
                            ))
                            .fill(if selection.tab == tab {
                                colors.accent_bg
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(7),
                        )
                        .clicked()
                    {
                        selection.tab = tab;
                    }
                }
            });
            ui.separator();
            ui.add_space(8.0);

            let run = selection.selected_run(task);
            match selection.tab {
                TaskDetailTab::Overview => render_overview(ui, colors, task, run),
                TaskDetailTab::Activity => render_activity(ui, colors, task, run),
                TaskDetailTab::Artifacts => render_artifacts(ui, colors, task, run),
                TaskDetailTab::Terminal => render_terminal(ui, colors, task, run, actions),
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
                RichText::new(format!("Run · {} · {}", run.harness, run.run_id))
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

fn render_overview(ui: &mut Ui, colors: &C, task: &TaskView, run: Option<&RunView>) {
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
        detail_row(ui, colors, "Harness", nonempty(&run.harness, "Unknown"));
        detail_row(ui, colors, "Role", nonempty(&run.role, "Not recorded"));
        detail_row(ui, colors, "Run state", &design::state_text(&run.state));
        detail_row(ui, colors, "Branch", nonempty(&run.branch, "Not recorded"));
        detail_row(
            ui,
            colors,
            "Worktree",
            nonempty(&run.worktree, "Not recorded"),
        );
    } else {
        wrapped_label(
            ui,
            RichText::new("Run details will appear when a Run is recorded.")
                .size(12.0)
                .color(colors.muted),
        );
    }
    ui.add_space(16.0);
    egui::CollapsingHeader::new("Identity & references").show(ui, |ui| {
        detail_row(ui, colors, "Task", &task.task_id);
        if let Some(run) = run {
            detail_row(ui, colors, "Run", &run.run_id);
        }
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
            .corner_radius(egui::CornerRadius::same(6))
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
            .corner_radius(egui::CornerRadius::same(6))
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

fn render_terminal(
    ui: &mut Ui,
    colors: &C,
    task: &TaskView,
    run: Option<&RunView>,
    actions: &UiActionQueue,
) {
    section_label(ui, colors, "NATIVE TERMINAL BINDING");
    let Some(run) = run else {
        ui.label(
            RichText::new("Select a Run before inspecting a terminal.")
                .size(12.0)
                .color(colors.muted),
        );
        return;
    };
    let terminal = &run.terminal;
    detail_row(
        ui,
        colors,
        "Status",
        nonempty(&terminal.status, "Not inspected"),
    );
    if let Some(binding_id) = terminal.binding_id.as_deref() {
        detail_row(ui, colors, "Binding", binding_id);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Focus terminal").clicked() {
                actions.enqueue(SupervisorAction::FocusTerminal {
                    binding_id: binding_id.to_string(),
                });
            }
            if ui.button("Unlink terminal").clicked() {
                actions.enqueue(SupervisorAction::UnlinkTerminal {
                    binding_id: binding_id.to_string(),
                });
            }
        });
    }
    if let Some(checked_at) = terminal.checked_at.as_deref() {
        detail_row(ui, colors, "Checked", checked_at);
    }
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Inspect terminal").clicked() {
            actions.enqueue(SupervisorAction::InspectTerminal {
                task_id: task.task_id.clone(),
                run_id: run.run_id.clone(),
            });
        }
    });
    ui.add_space(12.0);
    section_label(ui, colors, "AVAILABLE TERMINALS");
    if terminal.candidates.is_empty() {
        ui.label(
            RichText::new("No terminal candidates are reported.")
                .size(12.0)
                .color(colors.muted),
        );
    } else {
        for candidate in &terminal.candidates {
            terminal_candidate(ui, colors, &task.task_id, &run.run_id, candidate, actions);
        }
    }
    ui.add_space(8.0);
    wrapped_label(
        ui,
        RichText::new("Terminal contents stay in the native terminal surface; this view shows binding metadata only.")
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
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            wrapped_label(
                ui,
                RichText::new(nonempty(&candidate.display_name, "Terminal candidate"))
                    .size(13.0)
                    .strong()
                    .color(colors.txt),
            );
            detail_row(ui, colors, "Plugin", &candidate.plugin_id);
            detail_row(ui, colors, "Pane", &candidate.pane_id);
            detail_row(ui, colors, "Instance", &candidate.instance_id);
            let can_link = !candidate.plugin_id.is_empty()
                && !candidate.pane_id.is_empty()
                && !candidate.instance_id.is_empty();
            let link = ui
                .add_enabled(can_link, egui::Button::new("Link terminal"))
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

fn section_label(ui: &mut Ui, colors: &C, label: &str) {
    ui.label(RichText::new(label).size(11.0).strong().color(colors.faint));
    ui.add_space(5.0);
}

fn task_title(task: &TaskView) -> &str {
    nonempty(&task.title, "Untitled task")
}

fn task_state(task: &TaskView) -> &str {
    nonempty(&task.state, "Unknown state")
}

fn executor_label(task: &TaskView) -> &str {
    if !task.executor.is_empty() {
        return &task.executor;
    }
    task.runs
        .first()
        .map(|run| run.harness.as_str())
        .filter(|label| !label.is_empty())
        .unwrap_or("Unassigned")
}

fn run_label(run: &RunView) -> String {
    format!(
        "{} · {} · {}",
        nonempty(&run.harness, "Harness"),
        nonempty(&run.role, "Run"),
        run.run_id
    )
}

fn nonempty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}
