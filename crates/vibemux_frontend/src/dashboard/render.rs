use std::ops::Range;

use super::{
    AgentView, Context, DaemonView, DashboardModel, ListCounts, NativeTuiSlot, NativeTuiSlotState,
    State, Tab, Workspace,
    daemon_render::{render_daemon_view, visible_decisions, visible_tasks},
    filter::matches,
    model::{safe_text, slot_state_label},
    panes::{LayoutMode, effective_layout, pane_count},
};
use crate::tui::theme::{Theme, muted_style, title_style};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Tabs, Wrap},
};
use vibemux_harness::dispatch::AdapterSupport;

const WIDE_LAYOUT: u16 = 110;
const HARNESS_COLUMNS_WIDTH: u16 = 72;
/// Narrowest agent table with every probe column.
const AGENT_FULL_WIDTH: u16 = 72;
/// Narrowest compact agent table whose route column still fits
/// `local_gateway` beside the agent, launcher, and adapter columns.
const AGENT_ROUTE_WIDTH: u16 = 50;
const WORKTREE_PANEL_HEIGHT: u16 = 16;
/// Agent table rows that hold no agent: the top border, the header, and the
/// bottom border.
const AGENT_TABLE_CHROME_ROWS: u16 = 3;
/// The stacked overview's smallest agent table: its chrome plus two agents.
const AGENT_TABLE_MIN_HEIGHT: u16 = AGENT_TABLE_CHROME_ROWS + 2;

/// Narrowest terminal whose tab bar spells out every view's name with
/// padded ` | ` dividers (100 cells).
const PADDED_TAB_BAR_WIDTH: u16 = 100;
/// Narrowest terminal whose tab bar still spells out every view's name, with
/// bare `|` dividers (74 cells).
const FULL_TAB_LABELS_WIDTH: u16 = 76;
/// Narrowest terminal whose footer also names the zoom key.
const FULL_FOOTER_WIDTH: u16 = 100;

pub fn render_dashboard(frame: &mut Frame<'_>, model: &DashboardModel) {
    render_with_state(frame, model, &State::default(), &Context::default());
}

/// Rows each selectable list draws under the current filters. The runtime
/// hands these to `State::handle_key`, so a selection never points past a
/// filtered list.
#[must_use]
pub fn list_counts(model: &DashboardModel, context: &Context, state: &State) -> ListCounts {
    let daemon = context.daemon.as_deref();
    ListCounts {
        agents: model
            .filtered_agents(state.filter.query(Tab::Overview))
            .len(),
        branches: context.workspace.as_ref().map_or(0, |workspace| {
            filtered_workspace(workspace, state.filter.query(Tab::Workspace))
                .branches
                .len()
        }),
        tasks: daemon.map_or(0, |daemon| {
            visible_tasks(daemon, state.filter.query(Tab::Tasks)).len()
        }),
        decisions: daemon.map_or(0, |daemon| {
            visible_decisions(daemon, state.filter.query(Tab::Decisions)).len()
        }),
    }
}

fn filtered_model(model: &DashboardModel, query: &str) -> DashboardModel {
    DashboardModel {
        agents: model.filtered_agents(query).into_iter().cloned().collect(),
        ..model.clone()
    }
}

fn filtered_workspace(workspace: &Workspace, query: &str) -> Workspace {
    Workspace {
        branches: workspace
            .branches
            .iter()
            .filter(|branch| matches(query, &[&branch.name, &branch.role, &branch.upstream]))
            .cloned()
            .collect(),
        ..workspace.clone()
    }
}

/// Everything a view renderer reads, after filtering.
struct ViewInputs<'a> {
    model: &'a DashboardModel,
    workspace: Option<&'a Workspace>,
    daemon: Option<&'a DaemonView>,
    state: &'a State,
    theme: Theme,
}

pub fn render_with_state(
    frame: &mut Frame<'_>,
    model: &DashboardModel,
    state: &State,
    context: &Context,
) {
    let area = frame.area();
    if area.width < 30 || area.height < 12 {
        frame.render_widget(
            Paragraph::new("VibeMux: resize terminal").wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let theme = state.theme;
    let effective = effective_layout(state.panes.mode, area.width, area.height);
    let single = effective == LayoutMode::Single || state.panes.zoomed;
    let view = state.view(area.width, area.height);
    // The native slot panel describes the probe views; daemon views and
    // panes use the rows instead.
    let slots_height = match (single && !view.is_daemon(), area.width < 60) {
        (false, _) => 0,
        (true, true) => 7,
        (true, false) => 4,
    };
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(slots_height),
            Constraint::Length(3),
        ])
        .split(area);
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            safe_text(&model.title),
            title_style(theme).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  [{}]  READ ONLY / {}",
            safe_text(&model.platform),
            safe_text(&context.source)
        )),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, areas[0]);
    let tabs = Tabs::new(tab_titles(view, area.width))
        .select(view.index())
        .highlight_style(title_style(theme).add_modifier(Modifier::BOLD));
    let tabs = if area.width >= PADDED_TAB_BAR_WIDTH {
        tabs.divider(" | ")
    } else {
        tabs.padding("", "").divider("|")
    };
    frame.render_widget(tabs, areas[1]);

    let filtered = filtered_model(model, state.filter.query(Tab::Overview));
    let workspace = context
        .workspace
        .as_ref()
        .map(|workspace| filtered_workspace(workspace, state.filter.query(Tab::Workspace)));
    let inputs = ViewInputs {
        model: &filtered,
        workspace: workspace.as_ref(),
        daemon: context.daemon.as_deref(),
        state,
        theme,
    };
    if single {
        draw_view(frame, areas[2], view, &inputs);
    } else {
        let focus = state.panes.focus.min(pane_count(effective) - 1);
        for (index, rect) in pane_rects(areas[2], effective).into_iter().enumerate() {
            draw_pane(
                frame,
                rect,
                state.panes.views[index],
                index == focus,
                &inputs,
            );
        }
    }
    if slots_height > 0 {
        frame.render_widget(
            Paragraph::new(slot_lines(&model.native_tui_slots))
                .wrap(Wrap { trim: true })
                .style(muted_style(theme))
                .block(
                    Block::default()
                        .title("Native CLI TUI surfaces")
                        .borders(Borders::ALL),
                ),
            areas[3],
        );
    }
    render_footer(frame, areas[4], state, view, effective, theme);
    if state.help_visible {
        let overlay = Rect {
            x: area.x.saturating_add(2),
            y: area.y.saturating_add(2),
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(4),
        };
        frame.render_widget(Clear, overlay);
        frame.render_widget(
            Paragraph::new(HELP_TEXT)
                .wrap(Wrap { trim: true })
                .block(Block::default().title("Help").borders(Borders::ALL)),
            overlay,
        );
    }
}

const HELP_TEXT: &str = "READ-ONLY CONSOLE\n\n1-7: views (Overview, Harnesses, Workspace, Tasks, Daemon, Sessions, Decisions)\nTab: next view, or next pane in split and grid\nL: cycle layouts (single, split, grid)\nz: zoom the focused pane\nj/k or arrows: move the selection\nEnter: open the selected task or harness details\n/: filter the focused list (Enter keeps, Esc clears)\nr: refresh the daemon views now\nt: theme\n?: close this help\nq/Esc: quit\n\nNative TUI attach and tool execution are disabled. Daemon views read content-free state; no prompt or transcript is shown.\nLauncher, authentication, inference and task completion are separate evidence.\nWorkspace information uses cached Git refs; no fetch or cleanup occurs.";

/// Tab bar labels: every name down to `FULL_TAB_LABELS_WIDTH`, otherwise
/// digits plus the current view's name.
fn tab_titles(view: Tab, width: u16) -> Vec<String> {
    Tab::ALL
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            if width >= FULL_TAB_LABELS_WIDTH || *tab == view {
                format!("{} {}", index + 1, tab.label())
            } else {
                (index + 1).to_string()
            }
        })
        .collect()
}

fn pane_rects(body: Rect, effective: LayoutMode) -> Vec<Rect> {
    let halves = |area: Rect, direction: Direction| {
        Layout::default()
            .direction(direction)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area)
    };
    match effective {
        LayoutMode::Single => vec![body],
        LayoutMode::Split => halves(body, Direction::Horizontal).to_vec(),
        LayoutMode::Grid => {
            let rows = halves(body, Direction::Vertical);
            let top = halves(rows[0], Direction::Horizontal);
            let bottom = halves(rows[1], Direction::Horizontal);
            vec![top[0], top[1], bottom[0], bottom[1]]
        }
    }
}

/// One pane: a header line naming the view (marked when focused), then the
/// view itself.
fn draw_pane(frame: &mut Frame<'_>, rect: Rect, view: Tab, focused: bool, inputs: &ViewInputs<'_>) {
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(rect);
    let query = inputs.state.filter.query(view);
    let header = format!(
        "{}{} {}{}",
        if focused { "> " } else { "  " },
        view.index() + 1,
        view.label(),
        if query.is_empty() {
            String::new()
        } else {
            format!(" / filter: {query}")
        }
    );
    let style = if focused {
        title_style(inputs.theme).add_modifier(Modifier::BOLD)
    } else {
        muted_style(inputs.theme)
    };
    frame.render_widget(Paragraph::new(Line::styled(header, style)), parts[0]);
    draw_view(frame, parts[1], view, inputs);
}

fn draw_view(frame: &mut Frame<'_>, area: Rect, view: Tab, inputs: &ViewInputs<'_>) {
    let (model, state, theme) = (inputs.model, inputs.state, inputs.theme);
    match view {
        Tab::Overview => overview(frame, area, model, state, theme),
        Tab::Harnesses => harnesses(frame, area, model, state, theme),
        Tab::Workspace => workspace_view(frame, area, inputs.workspace, state, theme),
        Tab::Tasks | Tab::Daemon | Tab::Sessions | Tab::Decisions => {
            render_daemon_view(frame, area, view, inputs.daemon, state, theme);
        }
    }
}

fn render_footer(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &State,
    view: Tab,
    effective: LayoutMode,
    theme: Theme,
) {
    let query = state.filter.query(view);
    let keys = if state.filter.editing.is_some() {
        format!("filter: {query}_ | Enter keep | Esc clear")
    } else {
        let keys = if area.width >= FULL_FOOTER_WIDTH {
            "q quit | Tab/1-7 view | L layout | z zoom | / filter | r refresh | t theme | ? help"
        } else if area.width >= 80 {
            "q quit | Tab/1-7 view | L layout | / filter | r refresh | t theme | ? help"
        } else {
            "q quit | Tab view | L layout | ? help"
        };
        if query.is_empty() {
            keys.to_string()
        } else {
            format!("filter: {query} | {keys}")
        }
    };
    let mut status = format!("layout {}", effective.label());
    if state.panes.zoomed && effective != LayoutMode::Single {
        status.push_str(" zoomed");
    }
    if effective != state.panes.mode {
        status.push_str(&format!(
            " ({} needs a larger terminal)",
            state.panes.mode.label()
        ));
    }
    status.push_str(&format!(" | theme {}", theme.name()));
    frame.render_widget(
        Paragraph::new(keys).block(
            Block::default()
                .title(Line::styled(format!(" {status} "), muted_style(theme)).right_aligned())
                .borders(Borders::ALL),
        ),
        area,
    );
}

/// One line per slot state, in first-seen order, naming its harnesses. One
/// `name [state]` label per harness overflowed the panel at 80 columns and
/// hid the last harnesses; grouping keeps every name in two lines.
fn slot_lines(slots: &[NativeTuiSlot]) -> Vec<Line<'static>> {
    let mut states: Vec<NativeTuiSlotState> = Vec::new();
    for slot in slots {
        if !states.contains(&slot.state) {
            states.push(slot.state);
        }
    }
    states
        .into_iter()
        .map(|state| {
            let names = slots
                .iter()
                .filter(|slot| slot.state == state)
                .map(|slot| slot.agent.display_name())
                .collect::<Vec<_>>();
            Line::from(format!("{}: {}", slot_state_label(state), names.join(", ")))
        })
        .collect()
}

/// The details pane's slot line for one harness. A reserved slot is not a
/// running session; an unavailable one has no verified launcher.
const fn slot_detail(state: NativeTuiSlotState) -> &'static str {
    match state {
        NativeTuiSlotState::Reserved => "reserved, not running",
        NativeTuiSlotState::Unavailable => "unavailable, no verified launcher",
    }
}

fn overview(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardModel,
    state: &State,
    theme: Theme,
) {
    let diagnostics = [
        format!("Gateway: {}", safe_text(&model.gateway_summary)),
        format!("A2A: {}", safe_text(&model.a2a_summary)),
        format!("Telemetry: {}", safe_text(&model.telemetry_summary)),
    ];
    let body = if area.width >= WIDE_LAYOUT {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(area)
    } else {
        // A fixed eight-row table cut the telemetry line at 80x24 and left
        // taller terminals showing five agents above blank diagnostics rows.
        // Diagnostics get the rows their wrapped lines need, the table keeps
        // a header and two agents at the least, takes the rest up to its full
        // height, and any leftover goes back to diagnostics.
        let interior = usize::from(area.width.saturating_sub(2));
        let diagnostics_height = diagnostics
            .iter()
            .map(|line| wrapped_rows(line, interior))
            .sum::<usize>()
            .saturating_add(2);
        let table_full = u16::try_from(model.agents.len())
            .unwrap_or(u16::MAX)
            .saturating_add(AGENT_TABLE_CHROME_ROWS);
        let table_height = area
            .height
            .saturating_sub(u16::try_from(diagnostics_height).unwrap_or(u16::MAX))
            .max(AGENT_TABLE_MIN_HEIGHT)
            .min(table_full);
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(table_height), Constraint::Min(0)])
            .split(area)
    };
    agent_table(frame, body[0], model, state.selected_agent, theme);
    frame.render_widget(
        Paragraph::new(diagnostics.map(Line::from).to_vec())
            .wrap(Wrap { trim: true })
            .block(Block::default().title("Diagnostics").borders(Borders::ALL)),
        body[1],
    );
}

/// Rows `text` takes when greedily word-wrapped to `width` cells, the way
/// `Wrap { trim: true }` lays out a line; a word wider than a row is split
/// across whole rows.
fn wrapped_rows(text: &str, width: usize) -> usize {
    if width == 0 {
        return 0;
    }
    let mut rows = 1;
    let mut used = 0;
    for word in text.split_whitespace() {
        let length = word.chars().count();
        if used > 0 && used + 1 + length <= width {
            used += 1 + length;
            continue;
        }
        if used > 0 {
            rows += 1;
        }
        let extra_rows = length.saturating_sub(1) / width;
        rows += extra_rows;
        used = length - extra_rows * width;
    }
    rows
}

/// The agent rows a bordered table of `height` rows shows around the
/// selected agent, as an index range. The range starts at zero until the
/// selection would leave it, then keeps the selection on the last row.
fn agent_window(total: usize, selected: usize, height: u16) -> Range<usize> {
    let visible = usize::from(height.saturating_sub(AGENT_TABLE_CHROME_ROWS)).min(total);
    if visible == 0 {
        return 0..0;
    }
    let selected = selected.min(total - 1);
    let first = (selected + 1).saturating_sub(visible);
    first..first + visible
}

fn agent_table(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardModel,
    selected: usize,
    theme: Theme,
) {
    let total = model.agents.len();
    let window = agent_window(total, selected, area.height);
    // A short table scrolls with the selection; the title names the rows in
    // view so the harnesses below the fold are never silently absent.
    let title = if window.is_empty() || window.len() == total {
        "Agent probes".to_string()
    } else {
        format!("Agent probes {}-{}/{total}", window.start + 1, window.end)
    };
    let layout = AgentColumns::for_width(area.width);
    let rows = model.agents.iter().map(|agent| {
        Row::new(
            layout
                .values(agent)
                .into_iter()
                .map(|value| Cell::from(safe_text(value))),
        )
    });
    let (columns, header) = layout.columns();
    let table = Table::new(rows, columns)
        .header(Row::new(header).style(Style::default().add_modifier(Modifier::BOLD)))
        .row_highlight_style(title_style(theme))
        .highlight_symbol("> ")
        .block(Block::default().title(title).borders(Borders::ALL));
    let mut selection = TableState::default()
        .with_offset(window.start)
        .with_selected((total > 0).then(|| selected.min(total - 1)));
    frame.render_stateful_widget(table, area, &mut selection);
}

/// Agent table column sets, widest first. Adapter support is a build fact
/// that no observation replaces, so every set keeps it. Narrower sets drop
/// observation detail that the selected-harness pane still shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentColumns {
    Full,
    Compact,
    Minimal,
}

impl AgentColumns {
    const fn for_width(width: u16) -> Self {
        if width >= AGENT_FULL_WIDTH {
            Self::Full
        } else if width >= AGENT_ROUTE_WIDTH {
            Self::Compact
        } else {
            Self::Minimal
        }
    }

    fn values(self, agent: &AgentView) -> Vec<&str> {
        let adapter = adapter_column_label(agent.adapter.support);
        match self {
            Self::Full => vec![
                agent.name.as_str(),
                agent.launcher_state.as_str(),
                agent.authentication_state.as_str(),
                agent.inference_state.as_str(),
                agent.version.as_str(),
                adapter,
                agent.route.as_str(),
            ],
            Self::Compact => vec![
                agent.name.as_str(),
                agent.launcher_state.as_str(),
                adapter,
                agent.route.as_str(),
            ],
            Self::Minimal => vec![agent.name.as_str(), agent.launcher_state.as_str(), adapter],
        }
    }

    fn columns(self) -> (Vec<Constraint>, Vec<&'static str>) {
        match self {
            Self::Full => (
                vec![
                    Constraint::Length(10),
                    Constraint::Length(11),
                    Constraint::Length(9),
                    Constraint::Length(9),
                    Constraint::Length(11),
                    Constraint::Length(8),
                    Constraint::Min(10),
                ],
                vec![
                    "Agent",
                    "Launcher",
                    "Auth",
                    "Inference",
                    "Version",
                    "Adapter",
                    "Route",
                ],
            ),
            Self::Compact => (
                vec![
                    Constraint::Length(10),
                    Constraint::Length(11),
                    Constraint::Length(8),
                    Constraint::Min(7),
                ],
                vec!["Agent", "Launcher", "Adapter", "Route"],
            ),
            Self::Minimal => (
                vec![
                    Constraint::Length(10),
                    Constraint::Length(11),
                    Constraint::Min(8),
                ],
                vec!["Agent", "Launcher", "Adapter"],
            ),
        }
    }
}

/// Short table label; the details pane and snapshots spell out the full
/// support level and protocol list.
const fn adapter_column_label(support: AdapterSupport) -> &'static str {
    match support {
        AdapterSupport::Execute => "execute",
        AdapterSupport::ProbeOnly => "probe",
        AdapterSupport::TerminalOnly => "terminal",
    }
}

fn harnesses(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardModel,
    state: &State,
    theme: Theme,
) {
    let details_area = if area.width >= HARNESS_COLUMNS_WIDTH {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        agent_table(frame, body[0], model, state.selected_agent, theme);
        body[1]
    } else {
        // Overview lists every agent; this page keeps selected details readable.
        area
    };
    let selected = state
        .selected_agent
        .min(model.agents.len().saturating_sub(1));
    // Below the two-pane width no table is shown, so the title carries the
    // selection's position among every harness.
    let title = if model.agents.is_empty() {
        "Selected harness / j,k".to_string()
    } else {
        format!(
            "Selected harness {}/{} / j,k",
            selected + 1,
            model.agents.len()
        )
    };
    let details = match model.agents.get(selected) {
        Some(agent) => format!(
            "{} / version {}\nLauncher: {}\nAuth: {} | Inference: {}\nAdapter: {}\nRoute: {}\nCode: {}\nNative attach: disabled\nNative TUI slot: {}",
            safe_text(&agent.name),
            safe_text(&agent.version),
            safe_text(&agent.launcher_state),
            safe_text(&agent.authentication_state),
            safe_text(&agent.inference_state),
            agent.adapter.summary(),
            safe_text(&agent.route),
            safe_text(&agent.code),
            model
                .native_tui_slots
                .iter()
                .find(|slot| slot.agent == agent.adapter.harness)
                .map_or("-", |slot| slot_detail(slot.state))
        ),
        None => "No verified agent observation supplied.".to_string(),
    };
    frame.render_widget(
        Paragraph::new(details)
            .wrap(Wrap { trim: true })
            .block(Block::default().title(title).borders(Borders::ALL)),
        details_area,
    );
}

fn workspace_view(
    frame: &mut Frame<'_>,
    area: Rect,
    workspace: Option<&Workspace>,
    state: &State,
    theme: Theme,
) {
    let Some(workspace) = workspace else {
        frame.render_widget(
            Paragraph::new(
                "Workspace inventory not supplied. Use vibemuxctl ui for repository context.",
            )
            .wrap(Wrap { trim: true })
            .block(Block::default().title("Workspace").borders(Borders::ALL)),
            area,
        );
        return;
    };
    if area.height < 10 {
        frame.render_widget(
            Paragraph::new(
                "Resize terminal for branch details, or use vibemuxctl workspace status.",
            )
            .wrap(Wrap { trim: true })
            .block(Block::default().title("Workspace").borders(Borders::ALL)),
            area,
        );
        return;
    }
    let filters_disabled = workspace.change_detection == "external_filters_disabled";
    let show_worktrees = area.height >= WORKTREE_PANEL_HEIGHT;
    let mut constraints = vec![
        Constraint::Length(if filters_disabled { 6 } else { 5 }),
        Constraint::Min(4),
    ];
    if show_worktrees {
        constraints.push(Constraint::Length(5));
    }
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    let mut summary = format!(
        "Branch: {}   HEAD: {}\nBaseline: {}   ahead={} behind={}\nChanges: staged={} unstaged={} untracked={}",
        safe_text(&workspace.current_branch),
        safe_text(&workspace.head),
        safe_text(&workspace.baseline),
        workspace.ahead,
        workspace.behind,
        workspace.staged,
        workspace.unstaged,
        workspace.untracked
    );
    if filters_disabled {
        summary.push_str("\nExternal filters disabled; counts may differ.");
    }
    frame.render_widget(
        Paragraph::new(summary).block(
            Block::default()
                .title(format!(
                    "Local Git / cached refs / {} worktrees",
                    workspace.worktrees.len()
                ))
                .borders(Borders::ALL),
        ),
        areas[0],
    );
    let rows = workspace.branches.iter().map(|branch| {
        Row::new([
            Cell::from(safe_text(&branch.name)),
            Cell::from(safe_text(&branch.role)),
            Cell::from(safe_text(&branch.commit)),
            Cell::from(safe_text(&branch.upstream)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Min(22),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Min(12),
        ],
    )
    .header(
        Row::new(["Branch", "Role", "Commit", "Upstream"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(title_style(theme))
    .highlight_symbol("> ")
    .block(
        Block::default()
            .title("Branches / j,k to inspect")
            .borders(Borders::ALL),
    );
    let mut selection = TableState::default();
    selection.select((!workspace.branches.is_empty()).then(|| {
        state
            .selected_branch
            .min(workspace.branches.len().saturating_sub(1))
    }));
    frame.render_stateful_widget(table, areas[1], &mut selection);
    if !show_worktrees {
        return;
    }
    let worktrees = workspace
        .worktrees
        .iter()
        .take(3)
        .map(|line| safe_text(line))
        .collect::<Vec<_>>()
        .join("\n");
    frame.render_widget(
        Paragraph::new(worktrees).wrap(Wrap { trim: true }).block(
            Block::default()
                .title("Worktrees / no automatic cleanup")
                .borders(Borders::ALL),
        ),
        areas[2],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_window_keeps_the_selection_in_view() {
        // Eleven agents in an eight-row table: five rows hold agents.
        assert_eq!(agent_window(11, 0, 8), 0..5);
        assert_eq!(agent_window(11, 4, 8), 0..5);
        assert_eq!(agent_window(11, 5, 8), 1..6);
        assert_eq!(agent_window(11, 10, 8), 6..11);
        // An out-of-range selection clamps to the last agent.
        assert_eq!(agent_window(11, 99, 8), 6..11);
        // Everything fits, nothing fits, or there is nothing to show.
        assert_eq!(agent_window(3, 2, 8), 0..3);
        assert_eq!(agent_window(11, 4, 3), 0..0);
        assert_eq!(agent_window(0, 0, 8), 0..0);
    }

    #[test]
    fn wrapped_rows_counts_greedy_word_wrapping() {
        assert_eq!(wrapped_rows("Telemetry: telemetry unavailable", 78), 1);
        // "A2A: verified correlation=true" fills 30 of 43 cells.
        assert_eq!(
            wrapped_rows("A2A: verified correlation=true listener_closed=true", 43),
            2
        );
        assert_eq!(wrapped_rows("exactly ten", 11), 1);
        assert_eq!(wrapped_rows("exactly ten", 10), 2);
        // A word wider than a row fills whole rows: 4 + 4 + 2 cells.
        assert_eq!(wrapped_rows("abcdefghij", 4), 3);
        assert_eq!(wrapped_rows("ab abcdefghij", 4), 4);
        assert_eq!(wrapped_rows("", 10), 1);
        assert_eq!(wrapped_rows("anything", 0), 0);
    }
}
