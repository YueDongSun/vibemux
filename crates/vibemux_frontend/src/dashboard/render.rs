use super::{
    AgentView, Context, DashboardModel, State, Tab, Workspace,
    model::{safe_text, slot_state_label},
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
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

pub fn render_dashboard(frame: &mut Frame<'_>, model: &DashboardModel) {
    render_with_state(frame, model, &State::default(), &Context::default());
}

pub fn render_with_state(
    frame: &mut Frame<'_>,
    model: &DashboardModel,
    state: &State,
    context: &Context,
) {
    if frame.area().width < 30 || frame.area().height < 12 {
        frame.render_widget(
            Paragraph::new("VibeMux: resize terminal").wrap(Wrap { trim: true }),
            frame.area(),
        );
        return;
    }
    let slots_height = if frame.area().width < 60 { 7 } else { 4 };
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(slots_height),
            Constraint::Length(3),
        ])
        .split(frame.area());
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            safe_text(&model.title),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  [{}]  READ ONLY / {}",
            safe_text(&model.platform),
            safe_text(&context.source)
        )),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, areas[0]);
    frame.render_widget(
        Tabs::new(["1 Overview", "2 Harnesses", "3 Workspace"])
            .select(state.tab.index())
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .divider(" | "),
        areas[1],
    );

    match state.tab {
        Tab::Overview => overview(frame, areas[2], model, state),
        Tab::Harnesses => harnesses(frame, areas[2], model, state),
        Tab::Workspace => workspace_view(frame, areas[2], context.workspace.as_ref(), state),
    }
    let slot_labels = model
        .native_tui_slots
        .iter()
        .map(|slot| {
            format!(
                "{} [{}]",
                slot.agent.display_name(),
                slot_state_label(slot.state)
            )
        })
        .collect::<Vec<_>>();
    let labels = slot_labels.join(if areas[3].width < 60 { "\n" } else { " | " });
    frame.render_widget(
        Paragraph::new(labels)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .title("Native CLI TUI surfaces")
                    .borders(Borders::ALL),
            ),
        areas[3],
    );
    let footer = if frame.area().width >= 80 {
        "q/Esc quit | Tab/1/2/3 section | j/k select | Enter details | ? help"
    } else {
        "q quit | Tab page | j/k select | ? help"
    };
    frame.render_widget(
        Paragraph::new(footer).block(Block::default().borders(Borders::ALL)),
        areas[4],
    );
    if state.help_visible {
        let area = frame.area();
        let overlay = Rect {
            x: area.x.saturating_add(2),
            y: area.y.saturating_add(2),
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(4),
        };
        frame.render_widget(Clear, overlay);
        frame.render_widget(Paragraph::new("READ-ONLY SNAPSHOT\n\nTab or 1/2/3: switch sections\nj/k or arrows: move selection\nEnter: show harness details\n?: close this help\nq/Esc: quit\n\nNative TUI attach and tool execution are disabled.\nLauncher, authentication, inference and task completion are separate evidence.\nWorkspace information uses cached Git refs; no fetch or cleanup occurs.")
            .wrap(Wrap { trim: true }).block(Block::default().title("Help").borders(Borders::ALL)), overlay);
    }
}

fn overview(frame: &mut Frame<'_>, area: Rect, model: &DashboardModel, state: &State) {
    let body = if area.width >= WIDE_LAYOUT {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(8), Constraint::Min(1)])
            .split(area)
    };
    agent_table(frame, body[0], model, state.selected_agent);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Gateway: {}", safe_text(&model.gateway_summary))),
            Line::from(format!("A2A: {}", safe_text(&model.a2a_summary))),
            Line::from(format!(
                "Telemetry: {}",
                safe_text(&model.telemetry_summary)
            )),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::default().title("Diagnostics").borders(Borders::ALL)),
        body[1],
    );
}

fn agent_table(frame: &mut Frame<'_>, area: Rect, model: &DashboardModel, selected: usize) {
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
        .row_highlight_style(Style::default().fg(Color::Cyan))
        .highlight_symbol("> ")
        .block(Block::default().title("Agent probes").borders(Borders::ALL));
    let mut selection = TableState::default();
    selection.select(
        (!model.agents.is_empty()).then(|| selected.min(model.agents.len().saturating_sub(1))),
    );
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

fn harnesses(frame: &mut Frame<'_>, area: Rect, model: &DashboardModel, state: &State) {
    let details_area = if area.width >= HARNESS_COLUMNS_WIDTH {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        agent_table(frame, body[0], model, state.selected_agent);
        body[1]
    } else {
        // Overview lists every agent; this page keeps selected details readable.
        area
    };
    let details = match model.agents.get(
        state
            .selected_agent
            .min(model.agents.len().saturating_sub(1)),
    ) {
        Some(agent) => format!(
            "{} / version {}\nLauncher: {}\nAuth: {} | Inference: {}\nAdapter: {}\nRoute: {}\nCode: {}\nNative attach: disabled\nReserved is not running.",
            safe_text(&agent.name),
            safe_text(&agent.version),
            safe_text(&agent.launcher_state),
            safe_text(&agent.authentication_state),
            safe_text(&agent.inference_state),
            agent.adapter.summary(),
            safe_text(&agent.route),
            safe_text(&agent.code)
        ),
        None => "No verified agent observation supplied.".to_string(),
    };
    frame.render_widget(
        Paragraph::new(details).wrap(Wrap { trim: true }).block(
            Block::default()
                .title("Selected harness / j,k")
                .borders(Borders::ALL),
        ),
        details_area,
    );
}

fn workspace_view(frame: &mut Frame<'_>, area: Rect, workspace: Option<&Workspace>, state: &State) {
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
    .row_highlight_style(Style::default().fg(Color::Cyan))
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
