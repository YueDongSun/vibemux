#![forbid(unsafe_code)]
//! Conversation-first task activity.
use super::{
    C,
    design::{self, Icon},
    supervisor_state::{SupervisorUiState, TaskDetailTab, UiActionQueue},
    typography,
};
use crate::supervisor_model::{SnapshotMode, SupervisorAction, SupervisorSnapshot, TaskView};
use egui::{self, RichText, Ui, ViewportCommand, ViewportId};

pub const CONTENT_WIDTH: f32 = 720.0;
const FADE_HEIGHT: f32 = 28.0;
const BOTTOM_PADDING: f32 = 48.0;

pub fn render_conversation(
    ui: &mut Ui,
    ctx: &egui::Context,
    c: &C,
    snapshot: &SupervisorSnapshot,
    state: &mut SupervisorUiState,
    actions: &UiActionQueue,
) {
    let output = egui::ScrollArea::vertical()
        .id_salt("coordinator_conversation")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            content_column(ui, |ui| {
                ui.add_space(28.0);
                ui.horizontal(|ui| {
                    design::spark(ui, c.accent, 26.0, 0.0);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Workspace updates")
                                .size(15.0)
                                .strong()
                                .color(c.txt),
                        );
                        ui.label(
                            RichText::new("Task activity · reported by the daemon")
                                .size(12.0)
                                .color(c.muted),
                        );
                    });
                    if snapshot.mode == SnapshotMode::Demo {
                        demo_badge(ui, c);
                    }
                });
                ui.add_space(22.0);
                ui.label(
                    RichText::new(format!("{} tasks in this workspace", snapshot.tasks.len()))
                        .font(typography::display_font(ui.ctx(), typography::TITLE_SIZE))
                        .color(c.txt),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Open a task to follow its execution. The conversation stays here.",
                    )
                    .size(13.0)
                    .color(c.muted),
                );
                ui.add_space(14.0);
                for task in &snapshot.tasks {
                    task_row(ui, ctx, c, snapshot, task, state, actions);
                    ui.add_space(4.0);
                }
                if let Some(cursor) = snapshot.next_cursor.as_ref() {
                    ui.add_space(12.0);
                    if design::quiet_button(ui, c, "Load more tasks").clicked() {
                        actions.enqueue(SupervisorAction::LoadMoreTasks {
                            cursor: cursor.clone(),
                        });
                    }
                }
                ui.add_space(BOTTOM_PADDING);
            });
        });
    let viewport = output.inner_rect;
    let more_below = output.state.offset.y + viewport.height() < output.content_size.y - 1.0;
    if more_below {
        let fade = egui::Rect::from_min_max(
            egui::pos2(viewport.left(), viewport.bottom() - FADE_HEIGHT),
            viewport.max,
        );
        design::paint_bottom_fade(ui.painter(), fade, c.bg);
    }
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
    let background = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            task_row_contents(ui, ctx, c, snapshot, task, state, actions);
        });
    let rect = inner.response.rect;
    if ui.rect_contains_pointer(rect) {
        ui.painter().set(
            background,
            egui::epaint::RectShape::filled(rect, design::CARD_RADIUS, c.raised),
        );
    }
}

fn task_row_contents(
    ui: &mut Ui,
    ctx: &egui::Context,
    c: &C,
    snapshot: &SupervisorSnapshot,
    task: &TaskView,
    state: &mut SupervisorUiState,
    actions: &UiActionQueue,
) {
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
            egui::Button::new(
                RichText::new(title)
                    .font(typography::display_font(
                        ui.ctx(),
                        typography::TASK_TITLE_SIZE,
                    ))
                    .color(c.txt),
            )
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
        RichText::new(&task.latest_update)
            .font(typography::display_font(ui.ctx(), typography::PROSE_SIZE))
            .line_height(Some(typography::PROSE_LINE_HEIGHT))
            .color(c.muted),
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
    let width = ui.available_width().min(CONTENT_WIDTH);
    let inset = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.add_space(inset);
        ui.vertical(|ui| {
            ui.set_width(width);
            body(ui);
        });
    });
}
