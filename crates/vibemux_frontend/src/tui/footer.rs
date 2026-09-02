#![forbid(unsafe_code)]
//! Bottom-of-screen TUI footer: action keys + the active theme.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::theme::ThemeId;
use crate::view_model::ViewModel;

pub fn render_footer(frame: &mut Frame<'_>, _model: &ViewModel, theme_id: ThemeId) {
    let area = footer_area(frame);
    let palette = crate::theme::palette_for(theme_id);
    let bg = hex_color(&palette.surface, Color::Black);
    let border_color = hex_color(&palette.border, Color::DarkGray);
    let text_primary = hex_color(&palette.text_primary, Color::White);
    let text_muted = hex_color(&palette.text_muted, Color::Gray);
    let accent = hex_color(&palette.accent, Color::Cyan);

    let line = Line::from(vec![
        Span::styled(
            "q/Esc",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(": quit  ", Style::default().fg(text_muted)),
        Span::styled(
            "c",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(": copy snapshot  ", Style::default().fg(text_muted)),
        Span::styled(
            "t",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(": cycle theme (", Style::default().fg(text_muted)),
        Span::styled(theme_id.as_str(), Style::default().fg(text_primary)),
        Span::styled(")", Style::default().fg(text_muted)),
    ]);
    let paragraph = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(bg).fg(text_primary)),
    );
    frame.render_widget(paragraph, area);
}

fn footer_area(frame: &mut Frame<'_>) -> Rect {
    let full = frame.area();
    Rect {
        x: full.x,
        y: full.y + full.height.saturating_sub(3),
        width: full.width,
        height: 3,
    }
}

fn hex_color(hex: &str, fallback: Color) -> Color {
    match crate::theme::parse_hex_rgb(hex) {
        Some((r, g, b)) => Color::Rgb(r, g, b),
        None => fallback,
    }
}
