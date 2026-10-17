#![forbid(unsafe_code)]
//! Rendering for the DSH terminal surface.
//!
//! The renderer is pure: it takes a [`DshTuiState`] and a [`Theme`] and draws.
//! Two rules keep it honest:
//!
//! - **No invented completion.** The status line prints the session summary
//!   the state machine produced. A `session.status: idle` record is rendered
//!   as diagnostics, because idle is not completion (AGENTS.md §2 invariant 5).
//! - **Bounded display.** A runtime record is reduced to one truncated line;
//!   the byte-exact form stays in the driver's buffer, reachable through the
//!   poll operation rather than through the renderer.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde_json::Value;
use vibemux_harness::{
    dsh::{DshRecordKind, DshTurnOutcome},
    terminal_text::display_character,
};

use crate::tui::Theme;
use crate::tui::theme::{muted_style, title_style};

use super::{DshTuiState, MAX_LINE_BYTES, TranscriptLine};

/// Reduces one byte-exact runtime record to a single bounded display line.
///
/// Only discriminants this adapter has verified are read; everything else is
/// shown as an opaque marker plus a bounded prefix of the raw line, so an
/// unrecognized event is visible without being interpreted.
#[must_use]
pub fn display_text(kind: DshRecordKind, raw_json: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(raw_json).ok();
    let value = parsed.as_ref();
    let text = match kind {
        DshRecordKind::Handshake => value
            .and_then(|value| value.pointer("/result/serverInfo"))
            .map(|info| {
                format!(
                    "handshake ok: {} {}",
                    string_at(info, "/name"),
                    string_at(info, "/version")
                )
            })
            .unwrap_or_else(|| "handshake ok".to_string()),
        DshRecordKind::TurnStart => format!("turn {} started", turn_of(value)),
        DshRecordKind::TurnEnd => format!("turn {} ended: {}", turn_of(value), reason_of(value)),
        DshRecordKind::Text => value
            .and_then(|value| value.pointer("/params/event/data/chunk/text"))
            .and_then(Value::as_str)
            .map_or_else(|| "assistant text".to_string(), str::to_string),
        DshRecordKind::Tool => value
            .and_then(|value| value.pointer("/params/event/data/name"))
            .and_then(Value::as_str)
            .map_or_else(
                || "tool activity".to_string(),
                |name| format!("tool {name}"),
            ),
        DshRecordKind::Approval => {
            let tool = value
                .and_then(|value| value.pointer("/params/event/data/toolName"))
                .and_then(Value::as_str)
                .unwrap_or("(unnamed)");
            format!("approval asked: {tool}")
        }
        // Rendered as diagnostics only; never as completion.
        DshRecordKind::Status => format!(
            "status {} (diagnostic)",
            value
                .and_then(|value| value.pointer("/params/status"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
        DshRecordKind::Subagent => "subagent lifecycle".to_string(),
        DshRecordKind::Response => "response".to_string(),
        DshRecordKind::Error => {
            let message = value
                .and_then(|value| value.pointer("/error/message"))
                .and_then(Value::as_str)
                .unwrap_or("error");
            format!("error: {message}")
        }
        DshRecordKind::Other => "unrecognized frame".to_string(),
    };
    truncate(&one_line(&text), MAX_LINE_BYTES)
}

/// Runtime text as one display line: a line break, an ANSI escape, or a
/// bidirectional override becomes a space instead of reaching the terminal.
fn one_line(text: &str) -> String {
    text.chars().map(display_character).collect()
}

fn string_at(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string()
}

fn turn_of(value: Option<&Value>) -> String {
    value
        .and_then(|value| value.pointer("/params/event/data/turn"))
        .and_then(Value::as_u64)
        .map_or_else(|| "?".to_string(), |turn| turn.to_string())
}

fn reason_of(value: Option<&Value>) -> String {
    value
        .and_then(|value| value.pointer("/params/event/data/reason/kind"))
        .and_then(Value::as_str)
        .unwrap_or("unrecognized")
        .to_string()
}

/// Truncates on a character boundary, appending a marker when shortened.
#[must_use]
pub fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit.saturating_sub(3);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// Style for one transcript line. Emphasis comes from the audited theme, so
/// the surface cannot drift from the shared palette (ADR 026).
#[must_use]
pub fn line_style(theme: Theme, kind: Option<DshRecordKind>) -> Style {
    match kind {
        Some(DshRecordKind::TurnEnd | DshRecordKind::Handshake | DshRecordKind::Error) => {
            title_style(theme)
        }
        // Idle and other progress records are deliberately muted: they are
        // diagnostics, not outcomes.
        Some(DshRecordKind::Status | DshRecordKind::Other | DshRecordKind::Subagent) => {
            muted_style(theme)
        }
        None => muted_style(theme),
        _ => Style::default(),
    }
}

/// Renders the whole surface.
pub fn render_dsh(frame: &mut Frame<'_>, state: &DshTuiState, theme: Theme) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    render_status(frame, rows[0], state, theme);
    render_transcript(frame, rows[1], state, theme);
    render_input(frame, rows[2], state, theme);
    render_hint(frame, rows[3], state, theme);
}

fn render_status(frame: &mut Frame<'_>, area: Rect, state: &DshTuiState, theme: Theme) {
    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        " DeepSeek Harness session ",
        title_style(theme),
    ));
    // The handshake bounds the runtime's version string but not its characters.
    let version = state
        .server_version
        .as_deref()
        .map_or_else(|| "(no handshake yet)".to_string(), one_line);
    let outcome = state.last_outcome.map_or("none", DshTurnOutcome::as_str);
    let lines = vec![
        Line::from(format!(
            "phase {}   provider {}   model {}",
            state.phase.as_str(),
            state.provider,
            state.model
        )),
        Line::from(format!(
            "server {version}   turns {} (ok {} / failed {} / cancelled {} / unverified {})",
            state.turns_started,
            state.completed_turns,
            state.failed_turns,
            state.cancelled_turns,
            state.unverified_turns
        )),
        Line::from(Span::styled(
            format!("records {}   last outcome {outcome}", state.record_count),
            muted_style(theme),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, state: &DshTuiState, theme: Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" runtime records ", title_style(theme)));
    let inner_height = area.height.saturating_sub(2) as usize;
    let total = state.transcript.len();
    let scroll = usize::from(state.scroll).min(total.saturating_sub(1));
    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(inner_height.max(1));

    let lines: Vec<Line<'_>> = state.transcript[start..end]
        .iter()
        .map(|line| {
            let style = line_style(theme, line.kind());
            let prefix = match line {
                TranscriptLine::Runtime { sequence, .. } => format!("{sequence:>5} "),
                TranscriptLine::Local { .. } => "      ".to_string(),
            };
            Line::from(vec![
                Span::styled(prefix, muted_style(theme)),
                Span::styled(line.text().to_string(), style),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_input(frame: &mut Frame<'_>, area: Rect, state: &DshTuiState, theme: Theme) {
    let title = if state.accepts_prompt() {
        " prompt "
    } else {
        " prompt (session not ready) "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title, title_style(theme)));
    let mut spans = vec![Span::raw(state.input.clone())];
    if state.accepts_prompt() {
        spans.push(Span::styled(
            "_",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

fn render_hint(frame: &mut Frame<'_>, area: Rect, state: &DshTuiState, theme: Theme) {
    let hint = if state.scroll > 0 {
        format!("{}   [scrolled back {} lines]", state.status, state.scroll)
    } else {
        state.status.clone()
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, muted_style(theme)))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use vibemux_harness::dsh::DshPhase;

    fn render_to_string(state: &DshTuiState, theme: Theme, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dsh(frame, state, theme))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    fn state() -> DshTuiState {
        let mut state = DshTuiState::new(
            "deepseek-official".to_string(),
            "deepseek-v4-flash".to_string(),
        );
        state.phase = DshPhase::Ready;
        state.server_version = Some("0.0.1".to_string());
        state
    }

    #[test]
    fn a_turn_end_is_summarised_from_its_structured_reason() {
        let raw = r#"{"jsonrpc":"2.0","method":"session.event","params":{"sessionId":"s","event":{"type":"turn/end","seq":4,"time":3,"data":{"turn":7,"reason":{"kind":"completed"}}}}}"#;
        assert_eq!(
            display_text(DshRecordKind::TurnEnd, raw),
            "turn 7 ended: completed"
        );
        let failed = raw.replace("completed", "max-tokens");
        assert_eq!(
            display_text(DshRecordKind::TurnEnd, &failed),
            "turn 7 ended: max-tokens"
        );
        // An unrecognized reason is visible but never claimed as an outcome.
        let unknown = raw.replace("completed", "future-reason");
        assert_eq!(
            display_text(DshRecordKind::TurnEnd, &unknown),
            "turn 7 ended: future-reason"
        );
    }

    #[test]
    fn an_idle_status_is_labelled_as_a_diagnostic() {
        let raw = r#"{"jsonrpc":"2.0","method":"session.status","params":{"sessionId":"s","status":"idle"}}"#;
        let text = display_text(DshRecordKind::Status, raw);
        assert!(text.contains("idle"), "{text}");
        assert!(text.contains("diagnostic"), "{text}");
    }

    #[test]
    fn unrecognized_records_stay_visible_without_being_interpreted() {
        assert_eq!(
            display_text(DshRecordKind::Other, r#"{"method":"future/thing"}"#),
            "unrecognized frame"
        );
        // Broken JSON never panics and never invents a field.
        assert_eq!(
            display_text(DshRecordKind::TurnEnd, "not json"),
            "turn ? ended: unrecognized"
        );
        assert_eq!(
            display_text(DshRecordKind::Error, r#"{"error":{"message":"boom"}}"#),
            "error: boom"
        );
    }

    /// A runtime record or version can carry a line break, an ANSI escape, or
    /// a bidirectional override; none of them reaches the terminal raw.
    #[test]
    fn runtime_text_is_one_line_without_terminal_controls() {
        let raw = serde_json::json!({"params": {"event": {"data": {"chunk": {"text": "first\nsecond\u{1b}[2J\u{202e}gnp.exe"}}}}})
            .to_string();
        assert_eq!(
            display_text(DshRecordKind::Text, &raw),
            "first second [2J gnp.exe"
        );
        let error = serde_json::json!({"error": {"message": "bad\r\ntool\u{2066}"}}).to_string();
        assert_eq!(
            display_text(DshRecordKind::Error, &error),
            "error: bad  tool "
        );

        let mut state = state();
        state.server_version = Some("0.0.1\u{1b}]0;title\u{7}\u{202e}".to_string());
        let rendered = render_to_string(&state, Theme::Classic, 100, 20);
        assert!(rendered.contains("server 0.0.1 ]0;title"), "{rendered}");
        assert!(!rendered.contains(['\u{1b}', '\u{7}', '\u{202e}']));
    }

    #[test]
    fn display_lines_are_bounded_on_character_boundaries() {
        let raw = serde_json::json!({"params": {"event": {"data": {"chunk": {"text": "é".repeat(4000)}}}}})
            .to_string();
        let text = display_text(DshRecordKind::Text, &raw);
        assert!(text.len() <= MAX_LINE_BYTES);
        assert!(text.ends_with("..."));
        // Truncation never splits a multi-byte character.
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        assert_eq!(truncate("short", 20), "short");
    }

    #[test]
    fn the_dashboard_renders_every_phase_without_panicking() {
        for phase in [
            DshPhase::Starting,
            DshPhase::Initializing,
            DshPhase::Ready,
            DshPhase::Prompting,
            DshPhase::Closing,
            DshPhase::Closed,
            DshPhase::Failed,
        ] {
            for theme in Theme::ALL {
                let mut state = state();
                state.phase = phase;
                state.note("a local note");
                state.push(TranscriptLine::Runtime {
                    sequence: 1,
                    kind: DshRecordKind::Text,
                    text: "assistant text".to_string(),
                });
                let rendered = render_to_string(&state, theme, 100, 24);
                assert!(
                    rendered.contains(phase.as_str()),
                    "{phase:?} missing from the status line"
                );
                assert!(rendered.contains("DeepSeek Harness session"));
            }
        }
    }

    #[test]
    fn a_tiny_terminal_does_not_panic() {
        let mut state = state();
        for index in 0..40 {
            state.note(format!("line {index}"));
        }
        for (width, height) in [(20_u16, 6_u16), (1, 1), (200, 60)] {
            let _ = render_to_string(&state, Theme::Classic, width, height);
        }
    }

    #[test]
    fn the_transcript_follows_the_tail_until_scrolled_back() {
        let mut state = state();
        for index in 0..60 {
            state.note(format!("line {index}"));
        }
        let tail = render_to_string(&state, Theme::Classic, 80, 20);
        assert!(tail.contains("line 59"), "the tail must be visible");
        assert!(!tail.contains("line 0 "), "old lines scroll out of view");

        state.scroll = 40;
        let scrolled = render_to_string(&state, Theme::Classic, 80, 20);
        assert!(scrolled.contains("scrolled back 40 lines"));
        assert!(!scrolled.contains("line 59"));
    }
}
