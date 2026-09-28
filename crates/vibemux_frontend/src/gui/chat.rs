#![forbid(unsafe_code)]
//! Conversation-first task activity and a capability-aware composer.
use super::{
    C,
    design::{self, Icon},
    supervisor_state::{SupervisorUiState, TaskDetailTab, UiActionQueue},
};
use crate::supervisor_model::{SnapshotMode, SupervisorAction, SupervisorSnapshot, TaskView};
use egui::{self, RichText, Ui, ViewportCommand, ViewportId};

pub fn render_conversation(
    ui: &mut Ui,
    ctx: &egui::Context,
    c: &C,
    snapshot: &SupervisorSnapshot,
    state: &mut SupervisorUiState,
    actions: &UiActionQueue,
) {
    egui::ScrollArea::vertical().id_salt("coordinator_conversation").auto_shrink([false,false]).show(ui,|ui| {
        content_column(ui,|ui| {
            ui.add_space(28.0);
            ui.horizontal(|ui| {
                design::spark(ui, c.accent, 26.0, 0.0);ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Workspace updates").size(15.0).strong().color(c.txt));
                    ui.label(RichText::new("Task activity · reported by the daemon").size(12.0).color(c.muted));
                });
                if snapshot.mode==SnapshotMode::Demo {demo_badge(ui,c);}
            });
            ui.add_space(22.0);
            if snapshot.tasks.is_empty() {
                ui.label(RichText::new("A clear place to coordinate your work.").size(25.0).color(c.txt));
                ui.add_space(12.0);
                wrapped_label(ui,RichText::new("Assigned tasks, worker updates, and results will appear in this conversation. Your draft stays here while the coordinator connection is unavailable.").size(15.0).color(c.muted));
                ui.add_space(32.0);
                ui.horizontal(|ui| {design::icon(ui,Icon::Activity,c.muted,18.0);ui.label(RichText::new("No recorded tasks yet").size(13.0).color(c.muted));});
            } else {
                ui.label(RichText::new(format!("{} tasks in this workspace",snapshot.tasks.len())).size(22.0).color(c.txt));
                ui.add_space(7.0);
                ui.label(RichText::new("Open a task to follow its execution. The conversation stays here.").size(13.0).color(c.muted));
                ui.add_space(20.0);
                egui::Frame::new().fill(c.raised).stroke(egui::Stroke::new(1.0,c.border)).corner_radius(12).inner_margin(egui::Margin::symmetric(20,4)).show(ui,|ui| {
                    ui.set_min_width(ui.available_width());
                    for (index,task) in snapshot.tasks.iter().enumerate() {
                        if index>0 {ui.separator();}
                        task_row(ui,ctx,c,snapshot,task,state,actions);
                    }
                });
            }
            if let Some(cursor)=snapshot.next_cursor.as_ref() {
                ui.add_space(12.0);
                if design::quiet_button(ui,c,"Load more tasks").clicked() {actions.enqueue(SupervisorAction::LoadMoreTasks {cursor:cursor.clone()});}
            }
            ui.add_space(30.0);
        });
    });
}

fn task_row(
    ui: &mut Ui,
    ctx: &egui::Context,
    c: &C,
    snapshot: &SupervisorSnapshot,
    task: &TaskView,
    state: &mut SupervisorUiState,
    actions: &UiActionQueue,
) {
    ui.add_space(14.0);
    ui.horizontal_wrapped(|ui| {
        design::status(ui, c, &task.state);
        ui.label(RichText::new("/").size(12.0).color(c.muted));
        ui.label(
            RichText::new(if task.executor.is_empty() {
                "Unassigned"
            } else {
                &task.executor
            })
            .size(12.0)
            .color(c.muted),
        );
    });
    ui.add_space(3.0);
    let title = if task.title.is_empty() {
        "Untitled task"
    } else {
        &task.title
    };
    if ui
        .add(
            egui::Button::new(RichText::new(title).size(16.0).strong().color(c.txt))
                .frame(false)
                .wrap(),
        )
        .on_hover_text("Open task details")
        .clicked()
    {
        state.select_task(snapshot, &task.task_id);
        actions.enqueue(SupervisorAction::LoadTask {
            task_id: task.task_id.clone(),
        });
    }
    wrapped_label(
        ui,
        RichText::new(&task.latest_update).size(13.0).color(c.muted),
    );
    ui.add_space(5.0);
    ui.horizontal_wrapped(|ui| {
        if design::quiet_button(ui, c, "Details").clicked() {
            state.select_task(snapshot, &task.task_id);
            actions.enqueue(SupervisorAction::LoadTask {
                task_id: task.task_id.clone(),
            });
        }
        ui.separator();
        design::icon(ui, Icon::Arrow, c.muted, 16.0);
        let was_open = state.task_window_is_open(&task.task_id);
        if design::quiet_button(ui, c, "Open window").clicked()
            && state.open_task_window(snapshot, &task.task_id)
        {
            actions.enqueue(SupervisorAction::LoadTask {
                task_id: task.task_id.clone(),
            });
            if was_open {
                ctx.send_viewport_cmd_to(task_viewport_id(&task.task_id), ViewportCommand::Focus);
            }
        }
        ui.separator();
        design::icon(ui, Icon::Terminal, c.muted, 17.0);
        if ui
            .add_enabled(
                !task.runs.is_empty(),
                egui::Button::new(RichText::new("Native terminal").size(13.0).color(c.muted))
                    .frame(false),
            )
            .clicked()
        {
            if let Some(run) = task.runs.first() {
                state.select_task(snapshot, &task.task_id);
                state.details_selection_mut().tab = TaskDetailTab::Terminal;
                actions.enqueue(SupervisorAction::InspectTerminal {
                    task_id: task.task_id.clone(),
                    run_id: run.run_id.clone(),
                });
            }
        }
    });
    ui.add_space(12.0);
}

pub fn render_composer(ui: &mut Ui, c: &C, state: &mut SupervisorUiState) {
    content_column(ui, |ui| {
        design::composer_frame(c).show(ui, |ui| {
            let mut draft = state.composer_draft().to_string();
            let response = ui.add(
                egui::TextEdit::multiline(&mut draft)
                    .id_salt("coordinator_composer")
                    .desired_width(f32::INFINITY)
                    .desired_rows(3)
                    .hint_text("Describe what you want to get done…")
                    .frame(false)
                    .font(egui::TextStyle::Body),
            );
            if state.take_composer_focus_request() {
                response.request_focus();
            }
            if response.changed() {
                state.set_composer_draft(draft);
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                design::icon(ui, Icon::Chat, c.muted, 17.0);
                ui.label(RichText::new("To coordinator").size(12.0).color(c.muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let response = ui
                        .add_enabled(
                            state.send_enabled(),
                            egui::Button::new(RichText::new("Send").size(13.0))
                                .min_size(egui::vec2(64.0, 30.0))
                                .corner_radius(8),
                        )
                        .on_hover_text(state.send_disabled_reason());
                    if response.clicked() {
                        state.try_send();
                    }
                });
            });
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Draft only").size(11.5).color(c.warn));
            ui.label(
                RichText::new("Coordinator chat is not connected. Nothing has been sent.")
                    .size(11.5)
                    .color(c.muted),
            );
        });
    });
}
pub(crate) fn task_viewport_id(task_id: &str) -> ViewportId {
    ViewportId::from_hash_of(("vibemux_task", task_id))
}
pub(crate) fn demo_badge(ui: &mut Ui, c: &C) {
    ui.label(RichText::new("DEMO").size(10.0).strong().color(c.warn));
}
pub fn wrapped_label(ui: &mut Ui, text: RichText) -> egui::Response {
    ui.add(egui::Label::new(text).wrap())
}
pub(crate) fn content_column(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    let width = ui.available_width().min(790.0);
    let inset = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.add_space(inset);
        ui.vertical(|ui| {
            ui.set_width(width);
            body(ui);
        });
    });
}
