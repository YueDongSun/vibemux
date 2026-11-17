#![forbid(unsafe_code)]
//! Read-only collaboration flow view for the egui shell.
//!
//! [`layout`] turns a [`SupervisorSnapshot`] into a deterministic
//! left-to-right graph:
//!
//! ```text
//! Task ──spawned──▶ Run ──executed──▶ Harness ──blocked──▶ Status
//! ```
//!
//! A task fans out to its runs, each run names the harness that executed it,
//! and a run that needs the operator (blocked, failed, needs input, stale)
//! gets a status node. Nothing here writes canonical state: selecting a node
//! only routes through the existing task-detail selection, which enqueues the
//! same read the task list uses.

use eframe::egui::{
    self, Color32, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, vec2,
};

use crate::supervisor_model::{SupervisorSnapshot, TaskView};

use super::{
    C, design,
    task_labels::{nonempty, task_title},
};

/// Fixed geometry of the pure layout, in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutOptions {
    pub node_width: f32,
    pub node_height: f32,
    pub column_gap: f32,
    pub row_gap: f32,
    pub margin: f32,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            node_width: 208.0,
            node_height: 58.0,
            column_gap: 82.0,
            row_gap: 22.0,
            margin: 24.0,
        }
    }
}

impl LayoutOptions {
    /// The horizontal pitch of one column plus its gap.
    fn column_pitch(self) -> f32 {
        self.node_width + self.column_gap
    }

    /// The vertical pitch of one node row.
    fn row_pitch(self) -> f32 {
        self.node_height + self.row_gap
    }

    /// Left edge of `column`, counting the margin.
    #[must_use]
    pub fn column_x(self, column: usize) -> f32 {
        self.margin + column as f32 * self.column_pitch()
    }

    /// Top edge of `row`, counting the margin.
    #[must_use]
    pub fn row_y(self, row: usize) -> f32 {
        self.margin + row as f32 * self.row_pitch()
    }
}

/// Which stage of the flow a node belongs to. Its index is the column.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum NodeKind {
    Task,
    Run,
    Harness,
    Status,
}

impl NodeKind {
    pub const ALL: [Self; 4] = [Self::Task, Self::Run, Self::Harness, Self::Status];

    #[must_use]
    pub const fn column(self) -> usize {
        match self {
            Self::Task => 0,
            Self::Run => 1,
            Self::Harness => 2,
            Self::Status => 3,
        }
    }

    #[must_use]
    pub const fn caption(self) -> &'static str {
        match self {
            Self::Task => "Task",
            Self::Run => "Run",
            Self::Harness => "Harness",
            Self::Status => "Status",
        }
    }
}

/// A plain world-space rectangle. Kept independent of egui so the layout is a
/// pure function that unit tests can check without a GUI context.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NodeRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[cfg(test)]
impl NodeRect {
    #[must_use]
    pub fn right(self) -> f32 {
        self.x + self.width
    }

    #[must_use]
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    /// Strict overlap: touching edges do not count, so a gap of zero is still
    /// a clean layout.
    #[must_use]
    pub fn overlaps(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

/// One laid-out node. `task_id` is what a click routes to the existing detail.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub state: String,
    pub needs_you: bool,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub rect: NodeRect,
}

/// The three relationships the flow view draws.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdgeKind {
    /// A task spawned one of its runs.
    Spawned,
    /// A run was executed by a harness.
    Executed,
    /// A needs-you run reached a status that needs the operator.
    Blocked,
}

/// One edge, referring to [`GraphLayout::nodes`] by index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
}

/// The complete pure layout of one snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphLayout {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub width: f32,
    pub height: f32,
}

#[cfg(test)]
impl GraphLayout {
    pub fn nodes_of_kind(&self, kind: NodeKind) -> impl Iterator<Item = &GraphNode> {
        self.nodes.iter().filter(move |node| node.kind == kind)
    }

    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

/// States that hand work back to the operator. `attention` and `needs_input`
/// are accepted alongside the canonical tokens the store emits.
#[must_use]
pub fn needs_you(state: &str) -> bool {
    matches!(
        state,
        "blocked" | "input_required" | "failed" | "stale" | "attention" | "needs_input"
    )
}

fn task_needs_you(task: &TaskView) -> bool {
    needs_you(&task.state) || task.runs.iter().any(|run| needs_you(&run.state))
}

/// Lay out `snapshot` deterministically. Tasks keep their snapshot order,
/// runs keep their task order, and every run owns one row shared by its Run,
/// Harness, and (when needed) Status node.
#[must_use]
pub fn layout(snapshot: &SupervisorSnapshot, options: LayoutOptions) -> GraphLayout {
    let mut layout = GraphLayout::default();
    if snapshot.tasks.is_empty() {
        layout.width = options.margin * 2.0;
        layout.height = options.margin * 2.0;
        return layout;
    }

    let mut row = 0usize;
    for task in &snapshot.tasks {
        // A task without runs still occupies one row, so it never overlaps a
        // sibling and remains visible.
        let rows = task.runs.len().max(1);
        let first_row = row;
        let last_row = row + rows - 1;
        // Centre the task node over the rows its runs occupy.
        let center_y =
            (options.row_y(first_row) + options.row_y(last_row) + options.node_height) / 2.0;
        let task_index = layout.nodes.len();
        layout.nodes.push(GraphNode {
            id: format!("task:{}", task.task_id),
            kind: NodeKind::Task,
            label: task_title(&task.title).to_string(),
            state: task.state.clone(),
            needs_you: task_needs_you(task),
            task_id: Some(task.task_id.clone()),
            run_id: None,
            rect: NodeRect {
                x: options.column_x(NodeKind::Task.column()),
                y: center_y - options.node_height / 2.0,
                width: options.node_width,
                height: options.node_height,
            },
        });

        for (offset, run) in task.runs.iter().enumerate() {
            let y = options.row_y(first_row + offset);
            let rect = |kind: NodeKind| NodeRect {
                x: options.column_x(kind.column()),
                y,
                width: options.node_width,
                height: options.node_height,
            };

            let run_index = layout.nodes.len();
            layout.nodes.push(GraphNode {
                id: format!("run:{}:{}", task.task_id, run.run_id),
                kind: NodeKind::Run,
                label: nonempty(&run.harness, "Unassigned").to_string(),
                state: run.state.clone(),
                needs_you: needs_you(&run.state),
                task_id: Some(task.task_id.clone()),
                run_id: Some(run.run_id.clone()),
                rect: rect(NodeKind::Run),
            });
            layout.edges.push(GraphEdge {
                from: task_index,
                to: run_index,
                kind: EdgeKind::Spawned,
            });

            let harness_index = layout.nodes.len();
            layout.nodes.push(GraphNode {
                id: format!("harness:{}:{}", task.task_id, run.run_id),
                kind: NodeKind::Harness,
                label: nonempty(&run.harness, "Unassigned").to_string(),
                state: run.state.clone(),
                needs_you: needs_you(&run.state),
                task_id: Some(task.task_id.clone()),
                run_id: Some(run.run_id.clone()),
                rect: rect(NodeKind::Harness),
            });
            layout.edges.push(GraphEdge {
                from: run_index,
                to: harness_index,
                kind: EdgeKind::Executed,
            });

            if needs_you(&run.state) {
                let status_index = layout.nodes.len();
                layout.nodes.push(GraphNode {
                    id: format!("status:{}:{}", task.task_id, run.run_id),
                    kind: NodeKind::Status,
                    label: design::state_text(&run.state),
                    state: run.state.clone(),
                    needs_you: true,
                    task_id: Some(task.task_id.clone()),
                    run_id: Some(run.run_id.clone()),
                    rect: rect(NodeKind::Status),
                });
                // Routed from the harness stage so the edge stays left-to-right
                // and never crosses a node.
                layout.edges.push(GraphEdge {
                    from: harness_index,
                    to: status_index,
                    kind: EdgeKind::Blocked,
                });
            }
        }

        row += rows;
    }

    let columns = NodeKind::ALL.len();
    layout.width = options.margin * 2.0
        + columns as f32 * options.node_width
        + (columns - 1) as f32 * options.column_gap;
    layout.height = options.margin * 2.0
        + row as f32 * options.node_height
        + (row - 1) as f32 * options.row_gap;
    layout
}

/// Read-only data the page needs. The page never mutates the snapshot.
pub struct CollaborationView<'a> {
    pub snapshot: &'a SupervisorSnapshot,
    pub selected_task_id: Option<&'a str>,
}

/// What the page asks the shell to do. Only existing task-detail selection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CollaborationActions {
    pub selected_task_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ViewMode {
    #[default]
    Graph,
    List,
}

const VIEW_MODE_ID: &str = "collaboration_view_mode";
const CANVAS_STATE_ID: &str = "collaboration_canvas_state";
const MIN_ZOOM: f32 = 0.35;
const MAX_ZOOM: f32 = 2.5;
const NODE_RADIUS: u8 = design::BUTTON_RADIUS;

/// A pan/zoom transform stored in egui's per-context temp data.
#[derive(Clone, Copy, Debug)]
struct CanvasTransform {
    pan: Vec2,
    zoom: f32,
}

impl Default for CanvasTransform {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

/// Render the page: header, graph canvas (or its list fallback), and the
/// empty state when the snapshot holds no tasks.
pub fn render_page(ui: &mut Ui, colors: &C, view: &CollaborationView<'_>) -> CollaborationActions {
    let mut actions = CollaborationActions::default();
    render_header(ui, colors, view);
    ui.add_space(8.0);
    if view.snapshot.tasks.is_empty() {
        render_empty_state(ui, colors);
        return actions;
    }
    match view_mode(ui) {
        ViewMode::Graph => render_graph(ui, colors, view, &mut actions),
        ViewMode::List => render_list(ui, colors, view, &mut actions),
    }
    actions
}

fn view_mode(ui: &Ui) -> ViewMode {
    ui.ctx()
        .data(|data| data.get_temp::<ViewMode>(egui::Id::new(VIEW_MODE_ID)))
        .unwrap_or_default()
}

fn set_view_mode(ui: &Ui, mode: ViewMode) {
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new(VIEW_MODE_ID), mode));
}

fn render_header(ui: &mut Ui, colors: &C, view: &CollaborationView<'_>) {
    let task_count = view.snapshot.tasks.len();
    let run_count: usize = view.snapshot.tasks.iter().map(|task| task.runs.len()).sum();
    let needs_you = view
        .snapshot
        .tasks
        .iter()
        .filter(|task| task_needs_you(task))
        .count();
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new("Collaboration flow")
                    .size(20.0)
                    .strong()
                    .color(colors.txt),
            );
            ui.label(
                RichText::new(format!(
                    "{task_count} tasks · {run_count} runs · {needs_you} need you"
                ))
                .size(12.0)
                .color(if needs_you > 0 {
                    colors.warn
                } else {
                    colors.muted
                }),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Added right-to-left: List then Graph reads Graph | List.
            for (mode, label) in [(ViewMode::List, "List"), (ViewMode::Graph, "Graph")] {
                let selected = view_mode(ui) == mode;
                if ui
                    .selectable_label(
                        selected,
                        RichText::new(label).size(13.0).color(if selected {
                            colors.accent_text
                        } else {
                            colors.muted
                        }),
                    )
                    .clicked()
                {
                    set_view_mode(ui, mode);
                }
            }
            ui.label(RichText::new("View").size(12.0).color(colors.muted));
        });
    });
    ui.label(
        RichText::new("Read-only: a task, its runs, the harness that executed each, and any status that needs you.")
            .size(12.0)
            .color(colors.muted),
    );
}

fn render_empty_state(ui: &mut Ui, colors: &C) {
    ui.vertical_centered(|ui| {
        ui.add_space(64.0);
        design::icon(ui, design::Icon::Share, colors.muted, 30.0);
        ui.add_space(10.0);
        ui.label(
            RichText::new("No tasks yet")
                .size(17.0)
                .strong()
                .color(colors.txt),
        );
        ui.label(
            RichText::new(
                "Start a task to see how work flows from the task to its runs and harnesses.",
            )
            .size(13.0)
            .color(colors.muted),
        );
    });
}

fn render_graph(
    ui: &mut Ui,
    colors: &C,
    view: &CollaborationView<'_>,
    actions: &mut CollaborationActions,
) {
    let layout = layout(view.snapshot, LayoutOptions::default());
    let available = ui.available_size();
    let (canvas_rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
    let transform = update_transform(ui, &response, canvas_rect);
    let origin = canvas_rect.min;
    let painter = ui.painter_at(canvas_rect);

    // Edges first so nodes sit on top.
    for edge in &layout.edges {
        let (Some(from), Some(to)) = (layout.nodes.get(edge.from), layout.nodes.get(edge.to))
        else {
            continue;
        };
        let start = node_screen_rect(from.rect, origin, &transform);
        let end = node_screen_rect(to.rect, origin, &transform);
        let stroke = Stroke::new(
            match edge.kind {
                EdgeKind::Spawned => 1.2,
                EdgeKind::Executed => 1.5,
                EdgeKind::Blocked => 2.0,
            },
            edge_color(colors, edge.kind),
        );
        let start_pos = Pos2::new(start.right(), start.center().y);
        let end_pos = Pos2::new(end.left(), end.center().y);
        let dx = (end_pos.x - start_pos.x).abs().max(1.0);
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [
                start_pos,
                start_pos + vec2(dx * 0.45, 0.0),
                end_pos - vec2(dx * 0.45, 0.0),
                end_pos,
            ],
            false,
            Color32::TRANSPARENT,
            stroke,
        ));
        paint_arrowhead(&painter, end_pos, stroke);
    }

    let mut clicked = None;
    for (index, node) in layout.nodes.iter().enumerate() {
        let rect = node_screen_rect(node.rect, origin, &transform);
        let id = ui
            .id()
            .with(("collaboration_node", index, node.id.as_str()));
        let node_response = ui
            .interact(rect, id, Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        node_response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &node.label));
        let selected = node
            .task_id
            .as_deref()
            .is_some_and(|task_id| Some(task_id) == view.selected_task_id);
        paint_node(
            ui,
            &painter,
            colors,
            node,
            rect,
            selected,
            node_response.hovered(),
            transform.zoom,
        );
        if node_response.clicked() {
            clicked = node.task_id.clone();
        }
    }
    if let Some(task_id) = clicked {
        actions.selected_task_id = Some(task_id);
    }
}

fn update_transform(ui: &Ui, response: &Response, canvas_rect: Rect) -> CanvasTransform {
    let id = egui::Id::new(CANVAS_STATE_ID);
    let mut transform = ui
        .ctx()
        .data_mut(|data| *data.get_temp_mut_or_default::<CanvasTransform>(id));

    // Zoom over the whole canvas, including over a node, so the wheel never
    // feels dead. The node widgets sit on top of the canvas for clicks, so
    // this asks the pointer position instead of the canvas response.
    let pointer = ui
        .input(|input| input.pointer.hover_pos())
        .filter(|pointer| canvas_rect.contains(*pointer));
    if let Some(pointer) = pointer {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let old = transform.zoom;
            let new = (old * (1.0 + scroll * 0.0015)).clamp(MIN_ZOOM, MAX_ZOOM);
            // Keep the world point under the pointer fixed while zooming.
            let local = pointer - canvas_rect.min;
            let world = (local - transform.pan) / old;
            transform.pan = local - world * new;
            transform.zoom = new;
        }
    }
    if response.dragged() {
        transform.pan += response.drag_delta();
    }

    ui.ctx()
        .data_mut(|data| *data.get_temp_mut_or_default::<CanvasTransform>(id) = transform);
    transform
}

fn node_screen_rect(rect: NodeRect, origin: Pos2, transform: &CanvasTransform) -> Rect {
    let min = origin + Vec2::new(rect.x, rect.y) * transform.zoom + transform.pan;
    Rect::from_min_size(min, Vec2::new(rect.width, rect.height) * transform.zoom)
}

fn edge_color(colors: &C, kind: EdgeKind) -> Color32 {
    match kind {
        EdgeKind::Spawned => colors.faint,
        EdgeKind::Executed => colors.accent,
        EdgeKind::Blocked => colors.warn,
    }
}

fn paint_arrowhead(painter: &egui::Painter, end: Pos2, stroke: Stroke) {
    let size = 5.0;
    painter.line_segment([end, end + vec2(-size, -size * 0.7)], stroke);
    painter.line_segment([end, end + vec2(-size, size * 0.7)], stroke);
}

#[allow(clippy::too_many_arguments)]
fn paint_node(
    ui: &Ui,
    painter: &egui::Painter,
    colors: &C,
    node: &GraphNode,
    rect: Rect,
    selected: bool,
    hovered: bool,
    zoom: f32,
) {
    let needs_color = if node.state == "failed" {
        colors.danger
    } else {
        colors.warn
    };
    let border = if selected {
        Stroke::new(2.0 * zoom.max(0.6), colors.accent)
    } else if node.needs_you {
        Stroke::new(2.0 * zoom.max(0.6), needs_color)
    } else {
        Stroke::new(1.0 * zoom.max(0.6), colors.border)
    };
    let fill = if selected {
        colors.accent_bg
    } else if hovered {
        colors.bg
    } else {
        colors.raised
    };
    painter.rect(rect, NODE_RADIUS, fill, border, StrokeKind::Inside);

    // A small colour bar names the stage without adding another icon set.
    let bar_color = if node.needs_you {
        needs_color
    } else {
        match node.kind {
            NodeKind::Task => colors.accent,
            NodeKind::Run => colors.accent_text,
            NodeKind::Harness => colors.ok,
            NodeKind::Status => colors.warn,
        }
    };
    let inset = 8.0 * zoom;
    let bar = Rect::from_min_size(
        Pos2::new(rect.left() + inset, rect.top() + inset),
        Vec2::new(3.0 * zoom, (rect.height() - 2.0 * inset).max(0.0)),
    );
    painter.rect_filled(bar, 2, bar_color);

    let padding = 16.0 * zoom;
    let text_rect = rect.shrink2(Vec2::new(padding, 8.0 * zoom));
    let max_width = text_rect.width().max(1.0);
    let title_size = (13.0 * zoom).clamp(6.0, 26.0);
    let caption_size = (10.0 * zoom).clamp(5.0, 22.0);

    let title = egui::WidgetText::from(
        RichText::new(&node.label)
            .size(title_size)
            .strong()
            .color(colors.txt),
    )
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        max_width,
        egui::TextStyle::Body,
    );
    let title_pos = Pos2::new(text_rect.left(), text_rect.top());
    painter.galley(title_pos, title, colors.txt);

    let caption = format!(
        "{} · {}",
        node.kind.caption(),
        design::state_text(&node.state)
    );
    let caption_color = if node.needs_you {
        design::state_text_color(colors, &node.state)
    } else {
        colors.muted
    };
    let caption = egui::WidgetText::from(
        RichText::new(caption)
            .size(caption_size)
            .color(caption_color),
    )
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        max_width,
        egui::TextStyle::Small,
    );
    let caption_pos = Pos2::new(
        text_rect.left(),
        (text_rect.bottom() - caption.size().y).max(text_rect.top()),
    );
    painter.galley(caption_pos, caption, caption_color);

    if node.needs_you {
        let badge_size = (9.0 * zoom).clamp(4.0, 18.0);
        painter.circle_filled(
            Pos2::new(rect.right() - padding * 0.6, rect.top() + 12.0 * zoom),
            badge_size * 0.45,
            needs_color,
        );
    }
}

/// The list fallback: the same read-only chain as rows, usable at any width.
fn render_list(
    ui: &mut Ui,
    colors: &C,
    view: &CollaborationView<'_>,
    actions: &mut CollaborationActions,
) {
    egui::ScrollArea::vertical()
        .id_salt("collaboration_list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for task in &view.snapshot.tasks {
                let selected = view.selected_task_id == Some(task.task_id.as_str());
                egui::Frame::new()
                    .fill(if selected {
                        colors.accent_bg
                    } else {
                        colors.surf
                    })
                    .corner_radius(design::BUTTON_RADIUS)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .selectable_label(
                                    selected,
                                    RichText::new(task_title(&task.title))
                                        .size(14.0)
                                        .color(colors.txt),
                                )
                                .clicked()
                            {
                                actions.selected_task_id = Some(task.task_id.clone());
                            }
                            design::status(ui, colors, &task.state);
                            if task_needs_you(task) {
                                needs_you_badge(ui, colors);
                            }
                            ui.label(
                                RichText::new(format!(
                                    "{} runs · {}",
                                    task.runs.len(),
                                    nonempty(&task.executor, "Unassigned")
                                ))
                                .size(11.0)
                                .color(colors.muted),
                            );
                        });
                        for run in &task.runs {
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {} · {}",
                                        nonempty(&run.harness, "Unassigned"),
                                        nonempty(&run.role, "run"),
                                        design::state_text(&run.state)
                                    ))
                                    .size(11.0)
                                    .color(colors.muted),
                                );
                                if needs_you(&run.state) {
                                    needs_you_badge(ui, colors);
                                }
                            });
                        }
                        if task.runs.is_empty() {
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.label(
                                    RichText::new("No run recorded")
                                        .size(11.0)
                                        .color(colors.muted),
                                );
                            });
                        }
                    });
                ui.add_space(6.0);
            }
        });
}

fn needs_you_badge(ui: &mut Ui, colors: &C) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, colors.warn))
        .corner_radius(design::PILL_RADIUS)
        .inner_margin(egui::Margin::symmetric(6, 0))
        .show(ui, |ui| {
            ui.label(RichText::new("Needs you").size(10.0).color(colors.warn));
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor_model::{RunView, SupervisorSnapshot};

    fn run(run_id: &str, harness: &str, state: &str) -> RunView {
        RunView {
            run_id: run_id.to_string(),
            harness: harness.to_string(),
            role: "implementer".to_string(),
            state: state.to_string(),
            ..RunView::default()
        }
    }

    fn task(task_id: &str, state: &str, runs: Vec<RunView>) -> TaskView {
        TaskView {
            task_id: task_id.to_string(),
            title: format!("Task {task_id}"),
            state: state.to_string(),
            executor: runs
                .first()
                .map_or_else(|| "Unassigned".to_string(), |run| run.harness.clone()),
            runs,
            ..TaskView::default()
        }
    }

    fn snapshot(tasks: Vec<TaskView>) -> SupervisorSnapshot {
        SupervisorSnapshot {
            tasks,
            ..SupervisorSnapshot::default()
        }
    }

    fn options() -> LayoutOptions {
        LayoutOptions::default()
    }

    #[test]
    fn an_empty_snapshot_lays_out_no_nodes_and_stays_minimal() {
        let layout = layout(&snapshot(Vec::new()), options());
        assert!(layout.nodes.is_empty());
        assert!(layout.edges.is_empty());
        assert_eq!(layout.width, options().margin * 2.0);
        assert_eq!(layout.height, options().margin * 2.0);
    }

    #[test]
    fn layout_is_deterministic_and_follows_snapshot_order() {
        let snapshot = snapshot(vec![
            task("a", "running", vec![run("a1", "Claude", "running")]),
            task(
                "b",
                "blocked",
                vec![run("b1", "Codex", "done"), run("b2", "Codex", "blocked")],
            ),
        ]);
        let first = layout(&snapshot, options());
        let second = layout(&snapshot, options());
        assert_eq!(first, second, "the pure layout must be deterministic");

        let task_labels: Vec<&str> = first
            .nodes_of_kind(NodeKind::Task)
            .map(|node| node.label.as_str())
            .collect();
        assert_eq!(task_labels, ["Task a", "Task b"]);

        let run_labels: Vec<&str> = first
            .nodes_of_kind(NodeKind::Run)
            .map(|node| node.label.as_str())
            .collect();
        assert_eq!(run_labels, ["Claude", "Codex", "Codex"]);

        // Nodes keep the snapshot's top-to-bottom order in every column.
        for kind in NodeKind::ALL {
            let ys: Vec<f32> = first.nodes_of_kind(kind).map(|node| node.rect.y).collect();
            assert!(
                ys.windows(2).all(|pair| pair[0] < pair[1]),
                "{kind:?} rows must increase: {ys:?}"
            );
        }
    }

    #[test]
    fn columns_advance_left_to_right_and_nodes_never_overlap_at_fixed_width() {
        let snapshot = snapshot(vec![
            task(
                "a",
                "running",
                vec![run("a1", "Claude", "running"), run("a2", "Claude", "done")],
            ),
            task("b", "blocked", vec![run("b1", "Codex", "blocked")]),
            task("c", "running", vec![run("c1", "OpenCode", "running")]),
        ]);
        let layout = layout(&snapshot, options());

        for kind in NodeKind::ALL {
            for node in layout.nodes_of_kind(kind) {
                assert_eq!(
                    node.rect.x,
                    options().column_x(kind.column()),
                    "{kind:?} sits in its own column"
                );
            }
        }
        for pair in NodeKind::ALL.windows(2) {
            assert!(
                options().column_x(pair[0].column()) < options().column_x(pair[1].column()),
                "{:?} must sit left of {:?}",
                pair[0],
                pair[1]
            );
        }

        for kind in NodeKind::ALL {
            let nodes: Vec<&GraphNode> = layout.nodes_of_kind(kind).collect();
            for (index, left) in nodes.iter().enumerate() {
                for right in &nodes[index + 1..] {
                    assert!(
                        !left.rect.overlaps(right.rect),
                        "{kind:?} nodes overlap: {:?} vs {:?}",
                        left.id,
                        right.id
                    );
                }
            }
        }
        assert!(layout.height >= options().margin * 2.0);
        assert!(layout.width > options().column_x(NodeKind::Status.column()));
    }

    #[test]
    fn blocked_failed_and_attention_states_need_you() {
        for state in ["blocked", "failed", "input_required", "attention", "stale"] {
            assert!(needs_you(state), "{state}");
        }
        for state in ["running", "done", "succeeded", "open", "cancelled", ""] {
            assert!(!needs_you(state), "{state}");
        }
    }

    #[test]
    fn the_needs_you_flag_rolls_up_from_runs_to_the_task() {
        let quiet = task("quiet", "running", vec![run("q1", "Claude", "done")]);
        assert!(!task_needs_you(&quiet));
        let run_attention = task("run", "running", vec![run("r1", "Claude", "blocked")]);
        assert!(task_needs_you(&run_attention));
        let task_attention = task("task", "failed", vec![run("t1", "Claude", "done")]);
        assert!(task_needs_you(&task_attention));

        let layout = layout(
            &snapshot(vec![quiet.clone(), run_attention.clone(), task_attention]),
            options(),
        );
        let flags: Vec<bool> = layout
            .nodes_of_kind(NodeKind::Task)
            .map(|node| node.needs_you)
            .collect();
        assert_eq!(flags, [false, true, true]);
        let quiet_task = layout.node("task:quiet").expect("task");
        assert!(!quiet_task.needs_you);
        let attention_run = layout.node("run:run:r1").expect("run");
        assert!(attention_run.needs_you);
        assert!(layout.node("harness:run:r1").expect("harness").needs_you);
    }

    #[test]
    fn edges_are_spawned_executed_and_only_blocked_for_needs_you_runs() {
        let snapshot = snapshot(vec![task(
            "a",
            "running",
            vec![
                run("a1", "Claude", "running"),
                run("a2", "Codex", "blocked"),
                run("a3", "Codex", "failed"),
            ],
        )]);
        let layout = layout(&snapshot, options());

        let count = |kind: EdgeKind| layout.edges.iter().filter(|edge| edge.kind == kind).count();
        assert_eq!(count(EdgeKind::Spawned), 3);
        assert_eq!(count(EdgeKind::Executed), 3);
        assert_eq!(count(EdgeKind::Blocked), 2);

        let blocked: Vec<&GraphNode> = layout
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Blocked)
            .map(|edge| &layout.nodes[edge.to])
            .collect();
        assert_eq!(
            blocked
                .iter()
                .map(|node| node.state.as_str())
                .collect::<Vec<_>>(),
            ["blocked", "failed"]
        );
        assert!(blocked.iter().all(|node| node.kind == NodeKind::Status));

        // A run that does not need you gets no status node.
        assert!(layout.node("status:a:a1").is_none());
        assert!(layout.node("status:a:a2").is_some());
        assert!(layout.node("status:a:a3").is_some());

        // Every edge points at a later column, so the graph reads left to right.
        for edge in &layout.edges {
            assert!(
                layout.nodes[edge.from].kind.column() < layout.nodes[edge.to].kind.column(),
                "{edge:?} must move right"
            );
        }
    }

    #[test]
    fn a_task_without_runs_still_occupies_one_row() {
        let layout = layout(&snapshot(vec![task("solo", "open", Vec::new())]), options());
        assert_eq!(layout.nodes_of_kind(NodeKind::Task).count(), 1);
        assert_eq!(layout.nodes.len(), 1);
        assert!(layout.edges.is_empty());
        assert_eq!(
            layout.height,
            options().margin * 2.0 + options().node_height
        );
    }

    #[test]
    fn a_multi_run_task_is_centred_over_its_run_rows() {
        let layout = layout(
            &snapshot(vec![task(
                "a",
                "running",
                vec![
                    run("a1", "Claude", "running"),
                    run("a2", "Codex", "running"),
                    run("a3", "Codex", "running"),
                ],
            )]),
            options(),
        );
        let task = layout.node("task:a").expect("task");
        let runs: Vec<&GraphNode> = layout.nodes_of_kind(NodeKind::Run).collect();
        let run_center = (runs.first().unwrap().rect.y + runs.last().unwrap().rect.bottom()) / 2.0;
        assert!((task.rect.y + task.rect.height / 2.0 - run_center).abs() < 0.01);
    }
}
