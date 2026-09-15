#![forbid(unsafe_code)]
//! Bottom-of-screen TUI footer: action keys, active theme, health.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::theme::{Theme, health_style, muted_style};
use crate::view_model::ViewModel;

pub fn render_footer(frame: &mut Frame<'_>, model: &ViewModel, theme: Theme) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(frame.area());
    let line = Line::from(vec![
        Span::raw("q/Esc: quit"),
        Span::raw(" | "),
        Span::styled("c: copy snapshot", muted_style(theme)),
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
    ]);
    let paragraph = Paragraph::new(line).block(Block::default().borders(Borders::ALL));
    frame.render_widget(paragraph, areas[1]);
}
