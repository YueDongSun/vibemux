#![forbid(unsafe_code)]
//! Unified read-only dashboard model and Ratatui renderer.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};
use vibemux_probe::{
    A2aSelfTestProbe, AgentKind, AgentProbe, GatewayProbe, GatewayTelemetry, ProbeReport,
    ProbeState, RouteKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTuiSlotState {
    Reserved,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTuiSlot {
    pub agent: AgentKind,
    pub title: String,
    pub state: NativeTuiSlotState,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentView {
    pub name: String,
    pub launcher_state: ProbeState,
    pub authentication_state: ProbeState,
    pub inference_state: ProbeState,
    pub version: String,
    pub route: RouteKind,
    pub code: String,
}

/// Aggregate health computed from every probe dimension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Health {
    Ok,
    Warning,
    Failure,
}

/// Selectable visual palette for the dashboard renderer.
///
/// `Classic` targets 8-color terminals, `HighContrast` uses light variants for
/// bright environments and low-vision users, and `Mono` drops foreground
/// colors entirely (emphasis via bold/dim only) for colorless terminals,
/// log capture, and color-blind users.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Theme {
    #[default]
    Classic,
    HighContrast,
    Mono,
    Light,
}

impl Theme {
    /// Every theme in CLI listing order.
    pub const ALL: [Theme; 4] = [Self::Classic, Self::HighContrast, Self::Mono, Self::Light];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::HighContrast => "high-contrast",
            Self::Mono => "mono",
            Self::Light => "light",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "classic" => Some(Self::Classic),
            "high-contrast" => Some(Self::HighContrast),
            "mono" => Some(Self::Mono),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    /// The next theme in CLI listing order, wrapping around.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Classic => Self::HighContrast,
            Self::HighContrast => Self::Mono,
            Self::Mono => Self::Light,
            Self::Light => Self::Classic,
        }
    }

    /// Resolve the theme from `VIBEMUX_FRONTEND_THEME`, defaulting to `Classic`.
    #[must_use]
    pub fn from_environment() -> Self {
        std::env::var("VIBEMUX_FRONTEND_THEME")
            .ok()
            .as_deref()
            .and_then(Self::from_name)
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardModel {
    pub title: String,
    pub platform: String,
    pub observed_at: String,
    pub health: Health,
    pub overall_status: String,
    pub agents: Vec<AgentView>,
    pub gateway_summary: String,
    pub gateway_state: ProbeState,
    pub telemetry_summary: String,
    pub telemetry_failures: u64,
    pub telemetry_available: bool,
    pub a2a_summary: String,
    pub a2a_state: ProbeState,
    pub native_tui_slots: Vec<NativeTuiSlot>,
}

impl DashboardModel {
    #[must_use]
    pub fn from_report(report: &ProbeReport) -> Self {
        let agents: Vec<AgentView> = report.agents.iter().map(Self::build_agent_view).collect();
        let native_tui_slots = Self::build_native_tui_slots(report);
        let counts = StateCounts::from_agents(&report.agents);
        let health = compute_health(&counts, report.gateway.state, report.a2a.state);
        Self {
            title: "VibeMux Unified Frontend".to_string(),
            platform: report.platform.clone(),
            observed_at: format_epoch_seconds(report.observed_at_epoch_seconds),
            health,
            overall_status: build_overall_status(&counts, &report.gateway, &report.a2a),
            agents,
            gateway_summary: Self::build_gateway_summary(&report.gateway),
            gateway_state: report.gateway.state,
            telemetry_summary: Self::build_telemetry_summary(&report.gateway.telemetry),
            telemetry_failures: report
                .gateway
                .telemetry
                .iter()
                .map(|item| item.failures)
                .sum(),
            telemetry_available: !report.gateway.telemetry.is_empty(),
            a2a_summary: Self::build_a2a_summary(&report.a2a),
            a2a_state: report.a2a.state,
            native_tui_slots,
        }
    }

    fn build_agent_view(probe: &AgentProbe) -> AgentView {
        AgentView {
            name: probe.agent.display_name().to_string(),
            launcher_state: probe.launcher_state,
            authentication_state: probe.authentication_state,
            inference_state: probe.inference_state,
            version: probe.version.clone().unwrap_or_else(|| "-".to_string()),
            route: probe.route,
            code: probe.code.clone(),
        }
    }

    fn build_native_tui_slots(report: &ProbeReport) -> Vec<NativeTuiSlot> {
        AgentKind::all()
            .into_iter()
            .map(|agent| {
                let available = report
                    .agents
                    .iter()
                    .find(|probe| probe.agent == agent)
                    .is_some_and(|probe| probe.launcher_state == ProbeState::Verified);
                NativeTuiSlot {
                    agent,
                    title: format!("{} native TUI", agent.display_name()),
                    state: if available {
                        NativeTuiSlotState::Reserved
                    } else {
                        NativeTuiSlotState::Unavailable
                    },
                    reason: if available {
                        "reserved for supervised PTY/ConPTY attach".to_string()
                    } else {
                        "launcher unavailable or version probe failed".to_string()
                    },
                }
            })
            .collect()
    }

    fn build_gateway_summary(gateway: &GatewayProbe) -> String {
        format!(
            "{}:{} {} health={} code={}",
            gateway.host,
            gateway.port,
            state_label(gateway.state),
            gateway
                .health_status
                .map_or_else(|| "-".to_string(), |status| status.to_string()),
            gateway.code,
        )
    }

    fn build_a2a_summary(a2a: &A2aSelfTestProbe) -> String {
        format!(
            "{} correlation={} listener_closed={} code={}",
            state_label(a2a.state),
            a2a.correlation_preserved,
            a2a.listener_closed,
            a2a.code,
        )
    }

    fn build_telemetry_summary(telemetry: &[GatewayTelemetry]) -> String {
        if telemetry.is_empty() {
            "telemetry unavailable".to_string()
        } else {
            telemetry
                .iter()
                .map(|item| {
                    format!(
                        "{}:{} req/{} fail",
                        item.app_type, item.requests, item.failures
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ")
        }
    }

    #[must_use]
    pub fn plain_snapshot(&self) -> String {
        let mut lines = vec![
            self.title.clone(),
            format!(
                "platform: {} observed_at: {} health={:?}",
                self.platform, self.observed_at, self.health
            ),
            format!("overall: {}", self.overall_status),
            format!("gateway: {}", self.gateway_summary),
            format!("telemetry: {}", self.telemetry_summary),
            format!("a2a: {}", self.a2a_summary),
            "agents:".to_string(),
        ];
        lines.extend(self.agents.iter().map(|agent| {
            format!(
                "- {} launcher={} auth={} inference={} version={} route={} code={}",
                agent.name,
                state_label(agent.launcher_state),
                state_label(agent.authentication_state),
                state_label(agent.inference_state),
                agent.version,
                route_label(agent.route),
                agent.code,
            )
        }));
        lines.push("native_tui_slots:".to_string());
        lines.extend(self.native_tui_slots.iter().map(|slot| {
            format!(
                "- {} state={} reason={}",
                slot.title,
                slot_state_label(slot.state),
                slot.reason,
            )
        }));
        lines.join("\n")
    }
}

#[derive(Default)]
struct StateCounts {
    verified: usize,
    failed: usize,
    unavailable: usize,
    not_run: usize,
}

impl StateCounts {
    fn from_agents(agents: &[AgentProbe]) -> Self {
        let mut counts = Self::default();
        for agent in agents {
            match agent.launcher_state {
                ProbeState::Verified => counts.verified += 1,
                ProbeState::Failed => counts.failed += 1,
                ProbeState::Unavailable => counts.unavailable += 1,
                ProbeState::NotRun => counts.not_run += 1,
            }
        }
        counts
    }
}

fn compute_health(counts: &StateCounts, gateway: ProbeState, a2a: ProbeState) -> Health {
    if counts.failed > 0 || gateway == ProbeState::Failed || a2a == ProbeState::Failed {
        Health::Failure
    } else if counts.unavailable > 0
        || gateway == ProbeState::Unavailable
        || a2a == ProbeState::Unavailable
    {
        Health::Warning
    } else {
        Health::Ok
    }
}

fn build_overall_status(
    counts: &StateCounts,
    gateway: &GatewayProbe,
    a2a: &A2aSelfTestProbe,
) -> String {
    let agent_summary = if counts.failed == 0 && counts.unavailable == 0 && counts.not_run == 0 {
        format!("{} verified", counts.verified)
    } else {
        format!(
            "{} verified, {} failed, {} unavailable, {} not_run",
            counts.verified, counts.failed, counts.unavailable, counts.not_run
        )
    };
    format!(
        "agents: {} | gateway: {} | a2a: {}",
        agent_summary,
        state_label(gateway.state),
        state_label(a2a.state),
    )
}

pub fn render_dashboard(frame: &mut Frame<'_>, model: &DashboardModel) {
    render_dashboard_with_theme(frame, model, Theme::Classic);
}

pub fn render_dashboard_with_theme(frame: &mut Frame<'_>, model: &DashboardModel, theme: Theme) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        // Priority under height pressure: agent table, then diagnostics,
        // then slots (the footer health summary covers their absence). The
        // stacked body needs 13 rows for ten agent rows + header + borders
        // plus 5 for diagnostics, so 18 guarantees none of the ten rows is
        // cut; slots start shrinking below that. Slots use Length, not Min:
        // a Min floor would make ratatui steal rows back from the table
        // whenever the terminal is shorter than the full layout, and would
        // absorb all slack (blank space) when it is taller.
        .constraints([
            Constraint::Length(3),
            Constraint::Min(18),
            Constraint::Length(12),
            Constraint::Length(3),
        ])
        .split(frame.area());
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            &model.title,
            title_style(theme).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  [{}]  {}  ", model.platform, model.observed_at)),
        Span::styled(
            &model.overall_status,
            health_style(theme, model.health).add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, areas[0]);

    // Side-by-side only when the table keeps enough width for the longest
    // real version strings; narrower terminals stack vertically so the table
    // spans the full width and no state text is clipped.
    let body = if areas[1].width >= 160 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(64), Constraint::Percentage(36)])
            .split(areas[1])
    } else {
        // Ten agent rows plus the header and two border lines need at least
        // 13 rows; Min(12) clips the tenth agent (Kimi). Diagnostics needs
        // exactly 5 (three wrapped lines + borders) before any row is cut.
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(13), Constraint::Min(5)])
            .split(areas[1])
    };
    // Fixed columns (agent + three state columns + route) take 55 cells and
    // the six columns consume five 1-cell gaps inside the bordered table
    // interior; whatever remains belongs to Version. Ellipsize against that
    // real budget so narrow terminals show a clean ellipsis instead of a
    // hard mid-token cut, while wide tables keep full version strings.
    let version_budget = body[0].width.saturating_sub(2 + 55 + 5).max(10) as usize;
    let rows = model.agents.iter().map(|agent| {
        // A failed launcher jumps out when the whole row is bold; per-cell
        // state colors still apply on top for the state columns.
        let row_emphasis = if agent.launcher_state == ProbeState::Failed {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Row::new(vec![
            Cell::from(agent.name.clone()),
            Cell::from(state_label(agent.launcher_state).to_string())
                .style(state_style(theme, agent.launcher_state)),
            Cell::from(state_label(agent.authentication_state).to_string())
                .style(state_style(theme, agent.authentication_state)),
            Cell::from(state_label(agent.inference_state).to_string())
                .style(state_style(theme, agent.inference_state)),
            Cell::from(ellipsize(&agent.version, version_budget)),
            Cell::from(route_label(agent.route).to_string()).style(route_style(theme, agent.route)),
        ])
        .style(row_emphasis)
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Min(14),
            Constraint::Length(13),
        ],
    )
    .header(
        Row::new(["Agent", "Launcher", "Auth", "Infer", "Version", "Route"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().title("Agent probes").borders(Borders::ALL));
    frame.render_widget(table, body[0]);

    let diagnostics = Paragraph::new(vec![
        Line::from(vec![
            Span::raw("Gateway:   "),
            Span::styled(
                model.gateway_summary.clone(),
                state_style(theme, model.gateway_state),
            ),
        ]),
        Line::from(vec![
            Span::raw("A2A:       "),
            Span::styled(
                model.a2a_summary.clone(),
                state_style(theme, model.a2a_state),
            ),
        ]),
        Line::from(vec![
            Span::raw("Telemetry: "),
            Span::styled(
                model.telemetry_summary.clone(),
                telemetry_style(theme, model.telemetry_available, model.telemetry_failures),
            ),
        ]),
    ])
    .wrap(Wrap { trim: true })
    .block(Block::default().title("Diagnostics").borders(Borders::ALL));
    frame.render_widget(diagnostics, body[1]);

    let slot_entries: Vec<(String, Style)> = model
        .native_tui_slots
        .iter()
        .map(|slot| {
            (
                format!(
                    "{} [{}]",
                    slot.agent.display_name(),
                    slot_state_label(slot.state)
                ),
                slot_state_style(theme, slot.state),
            )
        })
        .collect();
    let slot_block = Block::default()
        .title("Native CLI TUI surfaces")
        .borders(Borders::ALL);
    // Slots render as per-slot styled, wrapped lines in every layout: ten
    // reservations overflow a single Tabs row (which truncated silently),
    // while wrapping keeps every slot visible at any width.
    let visible_rows = areas[2].height.saturating_sub(2) as usize;
    let lines: Vec<Line> = slot_entries
        .iter()
        .map(|(label, style)| Line::from(Span::styled(label.clone(), *style)))
        .collect();
    // Overflowing slots announce the truncation instead of ending silently:
    // the wrapped block shows how many reservations are cut, and the footer
    // health summary still covers every agent.
    let hidden = slot_entries.len().saturating_sub(visible_rows);
    let mut lines = lines;
    if hidden > 0 && visible_rows > 0 {
        lines.truncate(visible_rows);
        lines[visible_rows - 1] = Line::from(Span::styled(
            format!("… +{hidden} more slots"),
            muted_style(theme),
        ));
    }
    let slots = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .block(slot_block);
    frame.render_widget(slots, areas[2]);

    let footer = Paragraph::new(Line::from(vec![
        Span::raw("q/Esc: quit"),
        Span::raw(" | "),
        Span::styled(
            "native TUI launch/attach intentionally disabled",
            muted_style(theme),
        ),
        Span::raw(" | "),
        Span::styled(
            format!("theme: {} (t to cycle)", theme.name()),
            muted_style(theme),
        ),
        Span::raw(" | "),
        Span::styled(
            model.overall_status.clone(),
            health_style(theme, model.health),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, areas[3]);
}

const fn state_label(state: ProbeState) -> &'static str {
    match state {
        ProbeState::Verified => "verified",
        ProbeState::Failed => "failed",
        ProbeState::Unavailable => "unavailable",
        ProbeState::NotRun => "not_run",
    }
}

const fn route_label(route: RouteKind) -> &'static str {
    match route {
        RouteKind::Direct => "direct",
        RouteKind::LocalGateway => "local_gateway",
        RouteKind::Unknown => "unknown",
    }
}

const fn slot_state_label(state: NativeTuiSlotState) -> &'static str {
    match state {
        NativeTuiSlotState::Reserved => "reserved",
        NativeTuiSlotState::Unavailable => "unavailable",
    }
}

/// Truncate `text` to `max_width` visible cells with an ellipsis so a
/// clipped cell announces its own truncation instead of ending mid-token.
fn ellipsize(text: &str, max_width: usize) -> String {
    if text.chars().count() <= max_width {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_width.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

fn state_style(theme: Theme, state: ProbeState) -> Style {
    let (ok, failure, degraded, idle) = match theme {
        // Classic failure uses the bright red: the standard dark red only
        // reaches 3.6:1 against black, below WCAG AA (4.5:1).
        Theme::Classic => (
            Color::Green,
            Color::LightRed,
            Color::Yellow,
            Color::DarkGray,
        ),
        Theme::HighContrast => (
            Color::LightGreen,
            Color::LightRed,
            Color::LightYellow,
            Color::White,
        ),
        Theme::Mono => (Color::Reset, Color::Reset, Color::Reset, Color::Reset),
        // Light: dark variants tuned for white backgrounds; every value
        // clears WCAG AA (4.5:1) against white in the audit test. 16-color
        // yellow/gray fail on white, so degraded and idle use dark amber and
        // a darker gray instead.
        Theme::Light => (
            Color::Rgb(0, 110, 0),
            Color::Red,
            Color::Rgb(150, 75, 0),
            Color::Rgb(96, 96, 96),
        ),
    };
    let style = match state {
        ProbeState::Verified => Style::default().fg(ok),
        ProbeState::Failed => Style::default().fg(failure).add_modifier(Modifier::BOLD),
        ProbeState::Unavailable => Style::default().fg(degraded),
        ProbeState::NotRun => Style::default().fg(idle),
    };
    match theme {
        // Mono keeps a strict emphasis ladder so states stay distinguishable
        // without any color: verified and failure jump out bold, degraded is
        // plain, and idle recedes dim.
        Theme::Mono => match state {
            ProbeState::Verified | ProbeState::Failed => style.add_modifier(Modifier::BOLD),
            ProbeState::Unavailable => style,
            ProbeState::NotRun => style.add_modifier(Modifier::DIM),
        },
        _ => style,
    }
}

fn route_style(theme: Theme, route: RouteKind) -> Style {
    let (direct, gateway, unknown) = match theme {
        Theme::Classic => (Color::Green, Color::Cyan, Color::DarkGray),
        Theme::HighContrast => (Color::LightGreen, Color::LightCyan, Color::White),
        Theme::Mono => (Color::Reset, Color::Reset, Color::Reset),
        Theme::Light => (
            Color::Rgb(0, 110, 0),
            Color::Rgb(0, 110, 110),
            Color::Rgb(96, 96, 96),
        ),
    };
    match route {
        RouteKind::Direct => Style::default().fg(direct),
        RouteKind::LocalGateway => Style::default().fg(gateway),
        RouteKind::Unknown => Style::default().fg(unknown),
    }
}

fn slot_state_style(theme: Theme, state: NativeTuiSlotState) -> Style {
    let (reserved, unavailable) = match theme {
        Theme::Classic => (Color::Cyan, Color::DarkGray),
        Theme::HighContrast => (Color::LightCyan, Color::White),
        Theme::Mono => (Color::Reset, Color::Reset),
        Theme::Light => (Color::Rgb(0, 110, 110), Color::Rgb(96, 96, 96)),
    };
    match state {
        // Mono ladder mirrors the state-column ladder: reservations jump out
        // bold while unavailable slots recede dim.
        NativeTuiSlotState::Reserved if theme == Theme::Mono => {
            Style::default().add_modifier(Modifier::BOLD)
        }
        NativeTuiSlotState::Reserved => Style::default().fg(reserved),
        NativeTuiSlotState::Unavailable if theme == Theme::Mono => {
            Style::default().add_modifier(Modifier::DIM)
        }
        NativeTuiSlotState::Unavailable => Style::default().fg(unavailable),
    }
}

fn health_style(theme: Theme, health: Health) -> Style {
    let (ok, warning, failure) = match theme {
        Theme::Classic => (Color::Green, Color::Yellow, Color::LightRed),
        Theme::HighContrast => (Color::LightGreen, Color::LightYellow, Color::LightRed),
        Theme::Mono => (Color::Reset, Color::Reset, Color::Reset),
        // Terminal Green/Yellow on a white background measure ~1.71:1, far
        // below the WCAG AA 4.5:1 audit the light theme advertises; reuse the
        // dark variants already audited for the light state cells.
        Theme::Light => (Color::Rgb(0, 110, 0), Color::Rgb(150, 75, 0), Color::Red),
    };
    match health {
        Health::Ok => Style::default().fg(ok),
        Health::Warning => Style::default().fg(warning),
        Health::Failure => Style::default().fg(failure).add_modifier(Modifier::BOLD),
    }
}

fn title_style(theme: Theme) -> Style {
    match theme {
        Theme::Classic => Style::default().fg(Color::Cyan),
        Theme::HighContrast => Style::default().fg(Color::White),
        Theme::Mono => Style::default(),
        Theme::Light => Style::default().fg(Color::Rgb(0, 110, 110)),
    }
}

/// Telemetry line semantics: absent data stays muted, zero failures are
/// healthy green, and any failure count turns red regardless of theme.
fn telemetry_style(theme: Theme, available: bool, failures: u64) -> Style {
    if !available {
        return muted_style(theme);
    }
    match theme {
        Theme::Mono => {
            if failures > 0 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            }
        }
        // Light uses the dark red (clears WCAG AA on white); bright red fails
        // there, so the light theme flips the pairing. Green is likewise the
        // dark variant on light backgrounds.
        Theme::Light => {
            let color = if failures > 0 {
                Color::Red
            } else {
                Color::Rgb(0, 110, 0)
            };
            Style::default().fg(color)
        }
        _ => {
            let color = if failures > 0 {
                Color::LightRed
            } else {
                Color::Green
            };
            Style::default().fg(color)
        }
    }
}

fn muted_style(theme: Theme) -> Style {
    match theme {
        Theme::Classic => Style::default().fg(Color::DarkGray),
        Theme::HighContrast => Style::default().fg(Color::White),
        Theme::Mono => Style::default().add_modifier(Modifier::DIM),
        Theme::Light => Style::default().fg(Color::Rgb(96, 96, 96)),
    }
}

/// Render a Unix epoch-seconds value as ISO-8601 UTC (e.g. `2026-09-06T03:34:56Z`).
fn format_epoch_seconds(epoch_seconds: u64) -> String {
    let days = i64::try_from(epoch_seconds / 86_400).unwrap_or(0);
    let secs_today = epoch_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_today / 3600;
    let minute = (secs_today % 3600) / 60;
    let second = secs_today % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's date algorithm: convert days since 1970-01-01 to civil date.
const fn civil_from_days(days_since_1970: i64) -> (i64, u32, u32) {
    let z = days_since_1970 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use ratatui::{Terminal, backend::TestBackend};
    use vibemux_probe::{
        A2aSelfTestProbe, AgentKind, AgentProbe, GatewayProbe, LauncherKind, ProbeReport,
        ProbeState, RouteKind,
    };

    use super::*;

    fn report() -> ProbeReport {
        ProbeReport {
            schema_version: 1,
            // 2026-01-01T00:00:00Z
            observed_at_epoch_seconds: 1_767_225_600,
            platform: "windows".to_string(),
            agents: AgentKind::all()
                .into_iter()
                .map(|agent| AgentProbe {
                    agent,
                    launcher_state: ProbeState::Verified,
                    authentication_state: ProbeState::NotRun,
                    inference_state: ProbeState::NotRun,
                    launcher: LauncherKind::DirectExecutable,
                    version: Some("1.0".to_string()),
                    route: RouteKind::Direct,
                    endpoints: Vec::new(),
                    code: "version_verified".to_string(),
                })
                .collect(),
            gateway: GatewayProbe {
                state: ProbeState::Verified,
                host: "127.0.0.1".to_string(),
                port: 15_721,
                tcp_reachable: true,
                health_status: Some(200),
                telemetry_state: ProbeState::Verified,
                telemetry: Vec::new(),
                code: "gateway_verified".to_string(),
            },
            a2a: A2aSelfTestProbe {
                state: ProbeState::Verified,
                correlation_preserved: true,
                listener_closed: true,
                code: "a2a_self_test_verified".to_string(),
            },
        }
    }

    fn render_text(model: &DashboardModel, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard(frame, model))
            .expect("render dashboard");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn theme_next_cycles_through_all_themes() {
        let mut theme = Theme::Classic;
        for _ in 0..Theme::ALL.len() {
            theme = theme.next();
        }
        assert_eq!(theme, Theme::Classic, "next must cycle back to the start");
        assert_eq!(Theme::Classic.next(), Theme::HighContrast);
        assert_eq!(Theme::Mono.next(), Theme::Light);
    }

    #[test]
    fn ellipsize_bounds_are_exact() {
        // Fits: unchanged.
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("exact", 5), "exact");
        // One over: drop one char and add the ellipsis.
        assert_eq!(ellipsize("toolong", 5), "tool…");
        assert_eq!(ellipsize("2.1.259 (Claude Code)", 10), "2.1.259 (…");
        // Degenerate budgets still keep one visible char plus the marker.
        assert_eq!(ellipsize("abc", 1), "…");
        assert_eq!(ellipsize("abc", 0), "…");
        // Multi-byte characters count as cells, not bytes.
        assert_eq!(ellipsize("版本字符串很长", 4), "版本字…");
        assert_eq!(ellipsize("版本字符串很长", 4).chars().count(), 4);
    }

    #[test]
    fn theme_names_round_trip() {
        for theme in Theme::ALL {
            assert_eq!(Theme::from_name(theme.name()), Some(theme));
            assert_eq!(
                Theme::from_name(theme.name().to_uppercase().as_str()),
                Some(theme)
            );
        }
        assert_eq!(Theme::from_name("neon"), None);
        assert_eq!(Theme::from_name(""), None);
    }

    #[test]
    fn themes_color_verified_state_differently() {
        let model = DashboardModel::from_report(&report());
        let cells: Vec<(String, ratatui::style::Style)> = Theme::ALL
            .into_iter()
            .map(|theme| {
                (
                    theme.name().to_string(),
                    render_verified_style(model.clone(), theme),
                )
            })
            .collect();
        assert_eq!(cells[0].1.fg, Some(Color::Green));
        assert_eq!(cells[1].1.fg, Some(Color::LightGreen));
        assert_eq!(cells[2].1.fg, Some(Color::Reset));
    }

    fn render_verified_style(model: DashboardModel, theme: Theme) -> ratatui::style::Style {
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard_with_theme(frame, &model, theme))
            .expect("render dashboard");
        let cells = terminal.backend().buffer().content().to_vec();
        cells
            .iter()
            .scan(String::new(), |accumulator, cell| {
                accumulator.push_str(cell.symbol());
                if accumulator.ends_with("verified") {
                    Some(Some(cell.style()))
                } else if accumulator.len() > 64 {
                    accumulator.clear();
                    Some(None)
                } else {
                    Some(None)
                }
            })
            .flatten()
            .next()
            .expect("verified cell not found")
    }

    #[test]
    fn mono_theme_ladders_slot_states_too() {
        let model = DashboardModel::from_report(&report());
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard_with_theme(frame, &model, Theme::Mono))
            .expect("render dashboard");
        let cells = terminal.backend().buffer().content().to_vec();
        let width = 120usize;
        let rows: Vec<String> = cells
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect();
        // Slot lines render with per-slot styling at every width.
        let slot_row = rows
            .iter()
            .position(|row| row.contains("Claude [reserved]"))
            .expect("slot row present");
        let style = cells[slot_row * width + 1].style();
        assert!(
            style.add_modifier.contains(Modifier::BOLD),
            "reserved slot must be bold in mono"
        );
        assert!(
            !style.add_modifier.contains(Modifier::DIM),
            "reserved slot must not be dimmed in mono"
        );
    }

    #[test]
    fn mono_theme_ladders_emphasis_without_color() {
        let mut probe = report();
        probe.agents[0].launcher_state = ProbeState::Failed;
        probe.agents[2].launcher_state = ProbeState::Unavailable;
        let model = DashboardModel::from_report(&probe);
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard_with_theme(frame, &model, Theme::Mono))
            .expect("render dashboard");
        let cells = terminal.backend().buffer().content().to_vec();
        let width = 120usize;
        let rows: Vec<String> = cells
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect();
        // The state column starts after the border, 9-char agent column, and
        // one-cell gap; the mono ladder lives on those cells.
        let style_in_row = |needle: &str| {
            let row = rows
                .iter()
                .position(|row| row.contains(needle))
                .expect("row present");
            cells[row * width + 12].style()
        };
        // Verified and failed both jump out bold; unavailable stays plain.
        for needle in ["Claude", "Codex"] {
            let style = style_in_row(needle);
            assert!(
                style.add_modifier.contains(Modifier::BOLD),
                "{needle} state must be bold in mono"
            );
        }
        let unavailable = style_in_row("OpenCode");
        assert!(
            !unavailable.add_modifier.contains(Modifier::BOLD),
            "unavailable stays plain in mono"
        );
        assert!(
            !unavailable.add_modifier.contains(Modifier::DIM),
            "unavailable is not dimmed in mono"
        );
    }

    #[test]
    fn mono_theme_uses_no_foreground_colors() {
        let model = DashboardModel::from_report(&report());
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard_with_theme(frame, &model, Theme::Mono))
            .expect("render dashboard");
        for cell in terminal.backend().buffer().content() {
            if cell.symbol().trim().is_empty() {
                continue;
            }
            assert!(
                matches!(
                    cell.style().fg,
                    None | Some(Color::Reset) | Some(Color::Gray)
                ),
                "mono theme set a colored foreground: {:?} on {:?}",
                cell.style().fg,
                cell.symbol()
            );
        }
    }

    #[test]
    fn every_theme_keeps_real_data_unclipped_at_common_sizes() {
        let model = DashboardModel::from_report(&real_report());
        for theme in Theme::ALL {
            // 80 columns physically cannot fit the full Copilot version next
            // to the state columns; state words and short versions must fit.
            let narrow = render_text_with_theme(&model, theme, 80, 24);
            for expected in ["verified", "not_run", "local_gateway", "2.1.259"] {
                assert!(
                    narrow.contains(expected),
                    "missing {expected} for theme {} at 80x24",
                    theme.name()
                );
            }
            for (width, height) in [(120u16, 32u16), (160, 40)] {
                let rendered = render_text_with_theme(&model, theme, width, height);
                for expected in [
                    "verified",
                    "not_run",
                    "2.1.259 (Claude Code)",
                    "grok 1.0.13 (5e9a58528b76) [stable]",
                    "local_gateway",
                    "reserved",
                ] {
                    assert!(
                        rendered.contains(expected),
                        "missing {expected} for theme {} at {width}x{height}",
                        theme.name()
                    );
                }
            }
        }
    }

    fn render_text_with_theme(
        model: &DashboardModel,
        theme: Theme,
        width: u16,
        height: u16,
    ) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard_with_theme(frame, model, theme))
            .expect("render dashboard");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn dashboard_reserves_every_native_tui() {
        let model = DashboardModel::from_report(&report());
        assert_eq!(model.native_tui_slots.len(), 10);
        assert!(
            model
                .native_tui_slots
                .iter()
                .all(|slot| slot.state == NativeTuiSlotState::Reserved)
        );
    }

    #[test]
    fn ratatui_test_backend_renders_agents_and_reserved_slots() {
        let model = DashboardModel::from_report(&report());
        let rendered = render_text(&model, 120, 32);
        for expected in [
            "Claude",
            "Codex",
            "OpenCode",
            "Copilot",
            "Grok",
            "Native CLI TUI surfaces",
            "reserved",
            "Gateway",
            "A2A",
            "agents: 10 verified",
        ] {
            assert!(rendered.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn narrow_windows_terminal_keeps_all_agents_and_slots_visible() {
        let model = DashboardModel::from_report(&report());
        let rendered = render_text(&model, 80, 24);
        for expected in ["Claude", "Codex", "OpenCode", "Copilot", "Grok", "verified"] {
            assert!(rendered.contains(expected), "missing {expected}");
        }
    }

    fn real_report() -> ProbeReport {
        // Shapes captured from a real Windows probe run: verified launchers,
        // not_run auth/inference, and vendor-length version strings.
        let versions: [(AgentKind, &str, RouteKind); 5] = [
            (
                AgentKind::Claude,
                "2.1.259 (Claude Code)",
                RouteKind::LocalGateway,
            ),
            (AgentKind::Codex, "codex-cli 0.153.0", RouteKind::Unknown),
            (AgentKind::OpenCode, "1.18.21", RouteKind::Direct),
            (
                AgentKind::Copilot,
                "GitHub Copilot CLI 1.0.75.",
                RouteKind::Unknown,
            ),
            (
                AgentKind::Grok,
                "grok 1.0.13 (5e9a58528b76) [stable]",
                RouteKind::LocalGateway,
            ),
        ];
        let mut probe = report();
        probe.agents = versions
            .into_iter()
            .map(|(agent, version, route)| AgentProbe {
                agent,
                launcher_state: ProbeState::Verified,
                authentication_state: ProbeState::NotRun,
                inference_state: ProbeState::NotRun,
                launcher: LauncherKind::DirectExecutable,
                version: Some(version.to_string()),
                route,
                endpoints: Vec::new(),
                code: "version_verified".to_string(),
            })
            .collect();
        // The five domestic agents are not installed on the capture machine:
        // their probes report an unavailable launcher with no version and an
        // unknown route, exactly matching the live --once output.
        let domestic = [
            AgentKind::Qwen,
            AgentKind::Iflow,
            AgentKind::Trae,
            AgentKind::Codebuddy,
            AgentKind::Kimi,
        ];
        for agent in domestic {
            probe.agents.push(AgentProbe {
                agent,
                launcher_state: ProbeState::Unavailable,
                authentication_state: ProbeState::NotRun,
                inference_state: ProbeState::NotRun,
                launcher: LauncherKind::Unavailable,
                version: None,
                route: RouteKind::Unknown,
                endpoints: Vec::new(),
                code: "launcher_unavailable".to_string(),
            });
        }
        probe
    }

    #[test]
    fn real_probe_states_and_versions_are_never_clipped() {
        let model = DashboardModel::from_report(&real_report());
        // Regression: the combined "L:.. A:.. I:.." cell needed 30-31 chars
        // but had a fixed 28-char column, so verified agents rendered a
        // truncated inference state like "I:not_r" on real machines.
        let narrow = render_text(&model, 80, 24);
        for expected in [
            "verified",
            "not_run",
            "local_gateway",
            "2.1.259",
            "codex-cli 0.153.0",
            "GitHub Copilot",
        ] {
            assert!(narrow.contains(expected), "missing {expected} at 80x24");
        }
        for (width, height) in [(120u16, 32u16), (160, 40)] {
            let rendered = render_text(&model, width, height);
            for expected in [
                "verified",
                "not_run",
                "2.1.259 (Claude Code)",
                "codex-cli 0.153.0",
                "GitHub Copilot CLI 1.0.75.",
                "grok 1.0.13 (5e9a58528b76) [stable]",
                "local_gateway",
            ] {
                assert!(
                    rendered.contains(expected),
                    "missing {expected} at {width}x{height}"
                );
            }
        }
    }

    fn golden_fixture_path(theme: Theme, width: u16, height: u16) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden")
            .join(format!(
                "dashboard_{}_{}x{}.txt",
                theme.name(),
                width,
                height
            ))
    }

    fn canonical_render(model: &DashboardModel, theme: Theme, width: u16, height: u16) -> String {
        // Trailing spaces come from the right border padding; stripping them
        // keeps fixtures stable against buffer-width noise.
        render_text_with_theme(model, theme, width, height)
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join(
                "
",
            )
            + "
"
    }

    /// Regenerate the golden fixtures after an intentional visual change:
    /// `cargo test -p vibemux_frontend write_golden_fixtures -- --ignored --nocapture`
    #[test]
    #[ignore = "generator for golden render fixtures"]
    fn write_golden_fixtures() {
        let model = DashboardModel::from_report(&real_report());
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden");
        std::fs::create_dir_all(&directory).expect("create golden fixture directory");
        for theme in Theme::ALL {
            for (width, height) in [(80u16, 24u16), (120, 32)] {
                let path = golden_fixture_path(theme, width, height);
                std::fs::write(&path, canonical_render(&model, theme, width, height))
                    .expect("write golden fixture");
                println!("wrote {}", path.display());
            }
        }
    }

    #[test]
    fn golden_render_snapshots_match_fixtures() {
        let model = DashboardModel::from_report(&real_report());
        for theme in Theme::ALL {
            for (width, height) in [(80u16, 24u16), (120, 32)] {
                let path = golden_fixture_path(theme, width, height);
                let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                    panic!(
                        "missing golden fixture {} (run the ignored write_golden_fixtures test): {error}",
                        path.display()
                    )
                });
                assert_eq!(
                    canonical_render(&model, theme, width, height),
                    expected,
                    "dashboard render drifted for theme {} at {width}x{height}",
                    theme.name()
                );
            }
        }
    }

    #[test]
    fn plain_snapshot_is_deterministic_and_read_only() {
        let snapshot = DashboardModel::from_report(&report()).plain_snapshot();
        assert!(snapshot.contains("Claude native TUI state=reserved"));
        assert!(snapshot.contains("a2a: verified correlation=true listener_closed=true"));
        assert!(snapshot.contains("observed_at: 2026-01-01T00:00:00Z"));
        assert!(
            snapshot.contains("overall: agents: 10 verified | gateway: verified | a2a: verified")
        );
    }

    #[test]
    fn epoch_seconds_renders_as_iso8601_utc() {
        assert_eq!(format_epoch_seconds(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_epoch_seconds(1_767_225_600), "2026-01-01T00:00:00Z");
        assert_eq!(format_epoch_seconds(1_736_208_000), "2025-01-07T00:00:00Z");
        assert_eq!(format_epoch_seconds(86_400 * 365), "1971-01-01T00:00:00Z");
        // Partial day components
        assert_eq!(
            format_epoch_seconds(86_400 + 3 * 3600 + 4 * 60 + 5),
            "1970-01-02T03:04:05Z"
        );
    }

    #[test]
    fn civil_from_days_handles_epoch_and_neighboring_years() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(365), (1971, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(-365), (1969, 1, 1));
    }

    #[test]
    fn health_is_failure_when_any_agent_failed() {
        let mut probe = report();
        probe.agents[0].launcher_state = ProbeState::Failed;
        let model = DashboardModel::from_report(&probe);
        assert_eq!(model.health, Health::Failure);
        assert!(model.overall_status.contains("1 failed"));
        assert!(model.overall_status.contains("9 verified"));
    }

    #[test]
    fn health_is_warning_when_agent_unavailable() {
        let mut probe = report();
        probe.agents[0].launcher_state = ProbeState::Unavailable;
        let model = DashboardModel::from_report(&probe);
        assert_eq!(model.health, Health::Warning);
        assert!(model.overall_status.contains("1 unavailable"));
    }

    #[test]
    fn health_is_warning_when_gateway_unavailable() {
        let mut probe = report();
        probe.gateway.state = ProbeState::Unavailable;
        let model = DashboardModel::from_report(&probe);
        assert_eq!(model.health, Health::Warning);
    }

    #[test]
    fn health_is_failure_when_a2a_failed() {
        let mut probe = report();
        probe.a2a.state = ProbeState::Failed;
        let model = DashboardModel::from_report(&probe);
        assert_eq!(model.health, Health::Failure);
    }

    #[test]
    fn gateway_and_a2a_summaries_include_probe_codes() {
        let model = DashboardModel::from_report(&report());
        assert!(model.gateway_summary.contains("code=gateway_verified"));
        assert!(model.a2a_summary.contains("code=a2a_self_test_verified"));
    }

    #[test]
    fn title_row_displays_timestamp_and_overall_status() {
        let model = DashboardModel::from_report(&report());
        let rendered = render_text(&model, 160, 40);
        assert!(
            rendered.contains("2026-01-01T00:00:00Z"),
            "timestamp missing from title row"
        );
        assert!(
            rendered.contains("agents: 10 verified | gateway: verified | a2a: verified"),
            "overall status missing from title row"
        );
    }

    #[test]
    fn footer_includes_health_summary() {
        let model = DashboardModel::from_report(&report());
        let rendered = render_text(&model, 160, 40);
        assert!(rendered.contains("q/Esc: quit"));
        assert!(rendered.contains("agents: 10 verified"));
    }

    #[test]
    fn diagnostics_lines_color_by_probe_state() {
        let mut probe = report();
        probe.gateway.state = ProbeState::Failed;
        let model = DashboardModel::from_report(&probe);
        let width = 120usize;
        let backend = TestBackend::new(width as u16, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard(frame, &model))
            .expect("render dashboard");
        let cells = terminal.backend().buffer().content().to_vec();
        let rows: Vec<String> = cells
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect();
        let gateway_row = rows
            .iter()
            .position(|row| row.contains("127.0.0.1:15721"))
            .expect("gateway diagnostics row");
        let style = cells[gateway_row * width + 12].style();
        assert_eq!(
            style.fg,
            Some(Color::LightRed),
            "failed gateway must render bright red"
        );
    }

    // xterm-256 standard RGB values for the 16 base colors the themes use.
    fn color_rgb(color: Color) -> (f64, f64, f64) {
        match color {
            Color::Black => (0.0, 0.0, 0.0),
            Color::Red => (205.0, 0.0, 0.0),
            Color::Green => (0.0, 205.0, 0.0),
            Color::Yellow => (205.0, 205.0, 0.0),
            Color::Cyan => (0.0, 205.0, 205.0),
            Color::DarkGray => (128.0, 128.0, 128.0),
            Color::LightRed => (255.0, 0.0, 0.0),
            Color::LightGreen => (0.0, 255.0, 0.0),
            Color::LightYellow => (255.0, 255.0, 0.0),
            Color::LightCyan => (0.0, 255.0, 255.0),
            Color::White => (229.0, 229.0, 229.0),
            Color::Rgb(red, green, blue) => (f64::from(red), f64::from(green), f64::from(blue)),
            _ => (229.0, 229.0, 229.0),
        }
    }

    fn relative_luminance(rgb: (f64, f64, f64)) -> f64 {
        let channel = |value: f64| {
            let normalized = value / 255.0;
            if normalized <= 0.03928 {
                normalized / 12.92
            } else {
                ((normalized + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgb.0) + 0.7152 * channel(rgb.1) + 0.0722 * channel(rgb.2)
    }

    fn contrast_ratio(first: Color, second: Color) -> f64 {
        let lighter =
            relative_luminance(color_rgb(first)).max(relative_luminance(color_rgb(second)));
        let darker =
            relative_luminance(color_rgb(first)).min(relative_luminance(color_rgb(second)));
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn theme_state_colors_meet_wcag_aa_on_dark_backgrounds() {
        // The dashboard targets dark terminals; every state color must keep
        // at least AA (4.5:1) against black so text stays readable.
        let classic_pairs = [
            (ProbeState::Verified, Color::Green),
            (ProbeState::Failed, Color::LightRed),
            (ProbeState::Unavailable, Color::Yellow),
            (ProbeState::NotRun, Color::DarkGray),
        ];
        for (_, color) in classic_pairs {
            let ratio = contrast_ratio(color, Color::Black);
            assert!(ratio >= 4.5, "classic {color:?} contrast {ratio:.2} < 4.5");
        }
        let high_contrast_pairs = [
            (ProbeState::Verified, Color::LightGreen),
            (ProbeState::Failed, Color::LightRed),
            (ProbeState::Unavailable, Color::LightYellow),
            (ProbeState::NotRun, Color::White),
        ];
        for (state_probe, color) in high_contrast_pairs {
            let ratio = contrast_ratio(color, Color::Black);
            assert!(
                ratio >= 4.5,
                "high-contrast {color:?} contrast {ratio:.2} < 4.5"
            );
            let classic_twin = classic_pairs
                .iter()
                .find(|(state, _)| *state == state_probe)
                .map(|(_, color)| contrast_ratio(*color, Color::Black))
                .unwrap_or(0.0);
            assert!(
                ratio >= classic_twin,
                "high-contrast must not be weaker than classic"
            );
        }
        // Light variants must be strictly brighter than their classic twins.
        assert!(
            contrast_ratio(Color::LightGreen, Color::Black)
                > contrast_ratio(Color::Green, Color::Black)
        );
        assert!(
            contrast_ratio(Color::LightRed, Color::Black)
                > contrast_ratio(Color::Red, Color::Black)
        );
        // The light theme inverts the background: its dark colors must clear
        // AA against white, with the red pairing flipped (dark red passes on
        // white, bright red fails there).
        let light_pairs = [
            (ProbeState::Verified, Color::Rgb(0, 110, 0)),
            (ProbeState::Failed, Color::Red),
            (ProbeState::Unavailable, Color::Rgb(150, 75, 0)),
            (ProbeState::NotRun, Color::Rgb(96, 96, 96)),
        ];
        for (_, color) in light_pairs {
            let ratio = contrast_ratio(color, Color::White);
            assert!(
                ratio >= 4.5,
                "light {color:?} on white contrast {ratio:.2} < 4.5"
            );
        }
        assert!(contrast_ratio(Color::Red, Color::White) >= 4.5);
        assert!(contrast_ratio(Color::LightRed, Color::White) < 4.5);
        // The naive light palette would have failed: green-on-white 1.71,
        // yellow-on-white 1.71, gray-on-white 3.95.
        assert!(contrast_ratio(Color::Green, Color::White) < 4.5);
        assert!(contrast_ratio(Color::Yellow, Color::White) < 4.5);
        // Light theme accents against white: title/route cyan and the dark
        // gray muted/route-unknown clear AA too.
        for (label, color) in [
            ("light title dark cyan", Color::Rgb(0, 110, 110)),
            ("light route direct", Color::Rgb(0, 110, 0)),
            ("light route gateway", Color::Rgb(0, 110, 110)),
            ("light muted gray", Color::Rgb(96, 96, 96)),
        ] {
            let ratio = contrast_ratio(color, Color::White);
            assert!(ratio >= 4.5, "{label} contrast {ratio:.2} < 4.5");
        }
        // Every other accent color in the renderer: title cyan, tabs yellow,
        // route colors, and the muted gray must also clear AA on black.
        for (label, color) in [
            ("title cyan", Color::Cyan),
            ("high-contrast title white", Color::White),
            ("high-contrast tabs yellow", Color::LightYellow),
            ("tabs yellow", Color::Yellow),
            ("route direct green", Color::Green),
            ("route gateway cyan", Color::Cyan),
            ("muted dark gray", Color::DarkGray),
        ] {
            let ratio = contrast_ratio(color, Color::Black);
            assert!(ratio >= 4.5, "{label} contrast {ratio:.2} < 4.5");
        }
        // The aggregate health label in the title/footer must follow the same
        // audit: dark themes use the classic ramps on black; the light theme
        // uses the WCAG-corrected dark variants on white (plain terminal
        // Green/Yellow on white measure ~1.71:1 and would fail).
        for (label, color) in [
            ("classic health ok", Color::Green),
            ("classic health warning", Color::Yellow),
            ("classic health failure", Color::LightRed),
            ("high-contrast health ok", Color::LightGreen),
            ("high-contrast health warning", Color::LightYellow),
            ("high-contrast health failure", Color::LightRed),
        ] {
            let ratio = contrast_ratio(color, Color::Black);
            assert!(ratio >= 4.5, "{label} on black contrast {ratio:.2} < 4.5");
        }
        for (label, color) in [
            ("light health ok", Color::Rgb(0, 110, 0)),
            ("light health warning", Color::Rgb(150, 75, 0)),
            ("light health failure", Color::Red),
        ] {
            let ratio = contrast_ratio(color, Color::White);
            assert!(ratio >= 4.5, "{label} on white contrast {ratio:.2} < 4.5");
        }
    }

    #[test]
    fn stacked_layout_renders_all_ten_agent_rows() {
        // Regression: Min(12) fit only nine data rows (header + borders), so
        // the tenth agent (Kimi) was silently cut. The outer Min(18) keeps
        // the table at 13 + diagnostics at 5 at both golden sizes; smaller
        // terminals shrink slots first instead.
        let model = DashboardModel::from_report(&real_report());
        for (width, height) in [(80u16, 24u16), (120, 32)] {
            let rendered = render_text(&model, width, height);
            for name in [
                "Claude", "Codex", "OpenCode", "Copilot", "Grok", "Qwen", "iFlow", "TRAE",
                "CodeBuddy", "Kimi",
            ] {
                assert!(
                    rendered.contains(name),
                    "missing agent {name} at {width}x{height}"
                );
            }
        }
        // Slots shrink before any agent row: at 80x24 (25-row chrome budget)
        // the slot block collapses to its bordered title only, which the
        // footer health summary covers.
        let narrow = render_text(&model, 80, 24);
        assert!(narrow.contains("Native CLI TUI surfaces"));
        // At 120x32 six slots fit and the overflow announces itself instead
        // of truncating silently; every slot is visible from 36 rows up
        // (covered by dashboard_reserves_every_native_tui and the 160x40
        // theme sweep).
        let wide = render_text(&model, 120, 32);
        assert!(wide.contains("Claude [reserved]"));
        assert!(wide.contains("+4 more slots"), "overflow must be announced");
    }

    #[test]
    fn telemetry_line_reflects_failure_semantics() {
        let mut probe = report();
        probe.gateway.telemetry = vec![GatewayTelemetry {
            app_type: "codex".to_string(),
            requests: 10,
            failures: 3,
            latest_epoch_seconds: None,
        }];
        let model = DashboardModel::from_report(&probe);
        assert_eq!(model.telemetry_failures, 3);
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard(frame, &model))
            .expect("render dashboard");
        let cells = terminal.backend().buffer().content().to_vec();
        let width = 120usize;
        let rows: Vec<String> = cells
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect();
        let telemetry_row = rows
            .iter()
            .position(|row| row.contains("Telemetry: codex"))
            .expect("telemetry row present");
        let style = cells[telemetry_row * width + 12].style();
        assert_eq!(
            style.fg,
            Some(Color::LightRed),
            "failures must render bright red"
        );

        let healthy = DashboardModel::from_report(&report());
        assert_eq!(healthy.telemetry_summary, "telemetry unavailable");
        let mut clean = report();
        clean.gateway.telemetry = vec![GatewayTelemetry {
            app_type: "codex".to_string(),
            requests: 10,
            failures: 0,
            latest_epoch_seconds: None,
        }];
        assert_eq!(
            DashboardModel::from_report(&clean).telemetry_failures,
            0,
            "zero failures aggregate"
        );
    }

    #[test]
    fn failed_agent_row_is_bold_for_scan_visibility() {
        let mut probe = report();
        probe.agents[0].launcher_state = ProbeState::Failed;
        let model = DashboardModel::from_report(&probe);
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard(frame, &model))
            .expect("render dashboard");
        let cells = terminal.backend().buffer().content().to_vec();
        let width = 120usize;
        let rows: Vec<String> = cells
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect();
        let failed_row = rows
            .iter()
            .position(|row| row.contains("Claude") && row.contains("failed"))
            .expect("failed agent row");
        let name_cell = cells[failed_row * width + 2].style();
        assert!(
            name_cell.add_modifier.contains(Modifier::BOLD),
            "failed row must be bold"
        );
        let healthy_row = rows
            .iter()
            .position(|row| row.contains("Codex") && row.contains("verified"))
            .expect("healthy agent row");
        let healthy_style = cells[healthy_row * width + 2].style();
        assert!(
            !healthy_style.add_modifier.contains(Modifier::BOLD),
            "healthy rows must stay regular weight"
        );
    }

    #[test]
    fn wide_layout_places_diagnostics_beside_the_agent_table() {
        let model = DashboardModel::from_report(&real_report());
        let rows_of = |width: u16, height: u16| {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("test terminal");
            terminal
                .draw(|frame| render_dashboard(frame, &model))
                .expect("render dashboard");
            terminal
                .backend()
                .buffer()
                .content()
                .chunks(width as usize)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                .collect::<Vec<_>>()
        };
        // At 160 columns the diagnostics panel shares rows with agent rows
        // ("OpenCode" only ever appears inside the agent table).
        // The table header row and the first diagnostics line share a row
        // only in the side-by-side layout ("Launcher" is table-only).
        let wide = rows_of(160, 40);
        assert!(
            wide.iter()
                .any(|row| row.contains("Launcher") && row.contains("127.0.0.1:15721")),
            "wide layout must render diagnostics beside the agent table"
        );
        // At and below 120 columns the panels stack: no row mixes both.
        for narrow in [rows_of(80, 24), rows_of(120, 32)] {
            assert!(
                !narrow
                    .iter()
                    .any(|row| row.contains("Launcher") && row.contains("127.0.0.1:15721")),
                "narrow layouts must stack the panels on separate rows"
            );
        }
    }

    #[test]
    fn footer_shows_active_theme_name() {
        let model = DashboardModel::from_report(&report());
        for theme in Theme::ALL {
            let rendered = render_text_with_theme(&model, theme, 160, 40);
            assert!(
                rendered.contains(&format!("theme: {} (t to cycle)", theme.name())),
                "footer missing theme label {}",
                theme.name()
            );
        }
    }

    #[test]
    fn failure_state_renders_failed_agent_in_dashboard() {
        let mut probe = report();
        probe.agents[0].launcher_state = ProbeState::Failed;
        let model = DashboardModel::from_report(&probe);
        let rendered = render_text(&model, 120, 32);
        assert!(rendered.contains("failed"));
        assert!(model.overall_status.contains("1 failed"));
    }

    #[test]
    fn telemetry_summary_renders_empty_when_absent() {
        let model = DashboardModel::from_report(&report());
        assert_eq!(model.telemetry_summary, "telemetry unavailable");
    }

    #[test]
    fn agent_view_preserves_probe_state_for_styling() {
        let mut probe = report();
        probe.agents[0].launcher_state = ProbeState::Failed;
        probe.agents[1].launcher_state = ProbeState::Unavailable;
        let model = DashboardModel::from_report(&probe);
        assert_eq!(model.agents[0].launcher_state, ProbeState::Failed);
        assert_eq!(model.agents[1].launcher_state, ProbeState::Unavailable);
        assert_eq!(model.agents[2].launcher_state, ProbeState::Verified);
        assert_eq!(model.agents[2].route, RouteKind::Direct);
    }
}
