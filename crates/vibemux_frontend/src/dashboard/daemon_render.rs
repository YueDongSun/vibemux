//! Renderers of the four daemon views. They draw `DaemonView` data only, and
//! a missing read says why instead of showing an empty list.
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
};

use super::{
    State, Tab,
    daemon_model::{DaemonView, DecisionRow, Section, TaskDetailView, TaskRow},
    filter::matches,
};
use crate::tui::theme::{Theme, muted_style, title_style};

/// Narrowest view that puts the task detail beside the task list.
const TASK_SIDE_BY_SIDE_WIDTH: u16 = 100;

/// The tasks a filter leaves, in daemon order.
#[must_use]
pub fn visible_tasks<'a>(daemon: &'a DaemonView, query: &str) -> Vec<&'a TaskRow> {
    match &daemon.tasks {
        Section::Loaded(list) => list
            .rows
            .iter()
            .filter(|task| matches(query, &[&task.title, &task.status, &task.task_id]))
            .collect(),
        _ => Vec::new(),
    }
}

/// The ids of `visible_tasks`, which `Enter` indexes into.
#[must_use]
pub fn visible_task_ids(daemon: &DaemonView, query: &str) -> Vec<String> {
    visible_tasks(daemon, query)
        .into_iter()
        .map(|task| task.task_id.clone())
        .collect()
}

/// The decisions a filter leaves, newest first.
#[must_use]
pub fn visible_decisions<'a>(daemon: &'a DaemonView, query: &str) -> Vec<&'a DecisionRow> {
    match &daemon.decisions {
        Section::Loaded(rows) => rows
            .iter()
            .filter(|row| {
                matches(
                    query,
                    &[&row.decision_id, &row.status, &row.chosen, &row.source],
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn render_daemon_view(
    frame: &mut Frame<'_>,
    area: Rect,
    view: Tab,
    daemon: Option<&DaemonView>,
    state: &State,
    theme: Theme,
) {
    let Some(daemon) = daemon else {
        frame.render_widget(
            Paragraph::new(
                "Daemon views need the interactive console: run vibemuxctl ui in a terminal.",
            )
            .wrap(Wrap { trim: true })
            .block(Block::default().title(view.label()).borders(Borders::ALL)),
            area,
        );
        return;
    };
    match view {
        Tab::Tasks => tasks_view(frame, area, daemon, state, theme),
        Tab::Daemon => paragraph(frame, area, "Daemon", daemon_lines(daemon), theme),
        Tab::Sessions => paragraph(frame, area, "Sessions", session_lines(daemon), theme),
        Tab::Decisions => decisions_view(frame, area, daemon, state, theme),
        Tab::Overview | Tab::Harnesses | Tab::Workspace => {}
    }
}

/// One line for a section that has no data.
fn missing_line<T>(label: &str, section: &Section<T>) -> Option<String> {
    match section {
        Section::Waiting => Some(format!("{label}: waiting for the daemon")),
        Section::Unavailable(reason) => Some(format!(
            "{label}: unavailable ({}) - {}",
            reason.code, reason.hint
        )),
        Section::Loaded(_) => None,
    }
}

fn paragraph(frame: &mut Frame<'_>, area: Rect, title: &str, lines: Vec<String>, theme: Theme) {
    frame.render_widget(
        Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>())
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(Line::styled(title.to_string(), title_style(theme)))
                    .borders(Borders::ALL),
            ),
        area,
    );
}

fn filter_suffix(state: &State, view: Tab) -> String {
    let query = state.filter.query(view);
    let editing = state
        .filter
        .editing
        .is_some_and(|list| Some(list) == super::filter::FilterList::for_view(view));
    match (query.is_empty(), editing) {
        (true, false) => String::new(),
        (_, true) => format!(" / filter: {query}_"),
        (false, false) => format!(" / filter: {query}"),
    }
}

fn tasks_view(frame: &mut Frame<'_>, area: Rect, daemon: &DaemonView, state: &State, theme: Theme) {
    let parts = if area.width >= TASK_SIDE_BY_SIDE_WIDTH {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area)
    };
    let suffix = filter_suffix(state, Tab::Tasks);
    if let Some(line) = missing_line("Tasks", &daemon.tasks) {
        paragraph(
            frame,
            parts[0],
            &format!("Tasks{suffix}"),
            vec![line],
            theme,
        );
    } else {
        let tasks = visible_tasks(daemon, state.filter.query(Tab::Tasks));
        let more = matches!(&daemon.tasks, Section::Loaded(list) if list.more);
        let title = format!(
            "Tasks {}{}{suffix}",
            tasks.len(),
            if more { " (first page)" } else { "" }
        );
        let rows = tasks.iter().map(|task| {
            Row::new([
                Cell::from(task.status.clone()),
                Cell::from(task.title.clone()),
            ])
        });
        let table = Table::new(rows, [Constraint::Length(12), Constraint::Min(10)])
            .header(
                Row::new(["Status", "Title"]).style(Style::default().add_modifier(Modifier::BOLD)),
            )
            .row_highlight_style(title_style(theme))
            .highlight_symbol("> ")
            .block(
                Block::default()
                    .title(Line::styled(title, title_style(theme)))
                    .borders(Borders::ALL),
            );
        let mut selection = TableState::default()
            .with_selected((!tasks.is_empty()).then(|| state.selected_task.min(tasks.len() - 1)));
        frame.render_stateful_widget(table, parts[0], &mut selection);
    }
    let detail = match &daemon.task_detail {
        None => vec!["Enter opens the selected task's runs, events, and artifacts.".to_string()],
        Some(section) => match missing_line("Task detail", section) {
            Some(line) => vec![line],
            None => match section {
                Section::Loaded(detail) => detail_lines(detail),
                _ => Vec::new(),
            },
        },
    };
    paragraph(frame, parts[1], "Task detail", detail, theme);
}

fn detail_lines(detail: &TaskDetailView) -> Vec<String> {
    let mut lines = vec![format!("{} ({})", detail.title, detail.task_id)];
    for (label, items) in [
        ("Runs", &detail.runs),
        ("Events", &detail.events),
        ("Artifacts", &detail.artifacts),
    ] {
        lines.push(format!("{label}:"));
        if items.is_empty() {
            lines.push("  none".to_string());
        }
        lines.extend(items.iter().map(|item| format!("  {item}")));
    }
    if detail.truncated {
        lines.push("Older entries truncated by the daemon.".to_string());
    }
    lines
}

fn daemon_lines(daemon: &DaemonView) -> Vec<String> {
    let mut lines = vec![format!("Connection: {}", daemon.connection)];
    if let Some(observed) = &daemon.observed_at {
        lines.push(format!("Observed: {observed}"));
    }
    match &daemon.health {
        Section::Loaded(health) => lines.push(format!(
            "Health: {} | store schema {} | writer queue {}",
            health.status, health.schema, health.queue
        )),
        section => lines.extend(missing_line("Health", section)),
    }
    match &daemon.plugins {
        Section::Loaded(plugins) if plugins.is_empty() => {
            lines.push("Plugins: none configured".to_string());
        }
        Section::Loaded(plugins) => {
            lines.push("Plugins:".to_string());
            lines.extend(plugins.iter().map(|plugin| {
                format!(
                    "  {} {} restarts {} last error {}",
                    plugin.plugin_id, plugin.state, plugin.restarts, plugin.last_error
                )
            }));
        }
        section => lines.extend(missing_line("Plugins", section)),
    }
    match &daemon.routes {
        Section::Loaded(routes) if routes.is_empty() => {
            lines.push("Dispatch routes: none".to_string());
        }
        Section::Loaded(routes) => {
            lines.push("Dispatch routes:".to_string());
            lines.extend(
                routes
                    .iter()
                    .map(|route| format!("  {} {} {}", route.harness, route.protocol, route.mode)),
            );
        }
        section => lines.extend(missing_line("Dispatch routes", section)),
    }
    lines
}

fn session_lines(daemon: &DaemonView) -> Vec<String> {
    let mut lines = Vec::new();
    match &daemon.dsh {
        Section::Loaded(dsh) => {
            lines.push(format!(
                "DSH session: {} | {} / {} | transport {}",
                dsh.phase,
                dsh.provider,
                dsh.model,
                if dsh.transport_open { "open" } else { "closed" }
            ));
            lines.push(format!("  turns: {}", dsh.turns));
            lines.push("  A session phase is not task completion.".to_string());
        }
        section => lines.extend(missing_line("DSH session", section)),
    }
    match &daemon.slots {
        Section::Loaded(slots) => {
            lines.push(format!(
                "Workflow slots ({} evidence):",
                slots.evidence_class
            ));
            if slots.rows.is_empty() {
                lines.push("  none".to_string());
            }
            lines.extend(slots.rows.iter().map(|row| format!("  {row}")));
        }
        section => lines.extend(missing_line("Workflow slots", section)),
    }
    lines
}

fn decisions_view(
    frame: &mut Frame<'_>,
    area: Rect,
    daemon: &DaemonView,
    state: &State,
    theme: Theme,
) {
    let suffix = filter_suffix(state, Tab::Decisions);
    if let Some(line) = missing_line("Decisions", &daemon.decisions) {
        paragraph(
            frame,
            area,
            &format!("Decisions{suffix}"),
            vec![line],
            theme,
        );
        return;
    }
    let decisions = visible_decisions(daemon, state.filter.query(Tab::Decisions));
    let rows = decisions.iter().map(|row| {
        Row::new([
            Cell::from(row.decision_id.clone()),
            Cell::from(row.status.clone()),
            Cell::from(row.chosen.clone()),
            Cell::from(row.source.clone()),
            Cell::from(if row.consumed { "consumed" } else { "-" }),
            Cell::from(row.created.clone()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(9),
            Constraint::Min(10),
        ],
    )
    .header(
        Row::new(["Id", "Status", "Chosen", "Source", "Used", "Created"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(title_style(theme))
    .highlight_symbol("> ")
    .block(
        Block::default()
            .title(Line::styled(
                format!("Decisions {} (advisory){suffix}", decisions.len()),
                title_style(theme),
            ))
            .title_bottom(Line::styled(
                "A decision never grants execution",
                muted_style(theme),
            ))
            .borders(Borders::ALL),
    );
    let mut selection = TableState::default().with_selected(
        (!decisions.is_empty()).then(|| state.selected_decision.min(decisions.len() - 1)),
    );
    frame.render_stateful_widget(table, area, &mut selection);
}
