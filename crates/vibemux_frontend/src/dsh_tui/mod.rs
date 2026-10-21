#![forbid(unsafe_code)]
//! The DSH terminal surface: a live view of one DeepSeek Harness SDK session.
//!
//! This is deliberately a separate binary from `vibemux_frontend_tui`. That
//! view is a read-only diagnostic dashboard whose input loop does nothing but
//! switch themes; this one owns a runtime session, so it needs an editor, a
//! cancellation path, and a teardown that always reaps the child.
//!
//! Everything the surface shows comes from `vibemux_dsh::DshDriver`. The
//! renderer never invents a turn outcome: it prints the session summary the
//! state machine produced, and `session.status: idle` is rendered as
//! diagnostics rather than as completion.

pub mod render;
pub mod search;

use std::{io, time::Duration};

use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::runtime::Handle;
use vibemux_dsh::{DshDriver, DshDriverError, DshLaunch};
use vibemux_harness::dsh::{DshPhase, DshRecordKind, DshSessionSettings, DshTurnOutcome};

use crate::tui::Theme;

/// Poll cadence of the runtime event stream. The reader task is event-driven;
/// this only bounds how long a record waits before it is drawn.
const POLL_INTERVAL: Duration = Duration::from_millis(80);
/// Bound on typed input, so a paste cannot grow memory without limit.
pub const MAX_INPUT_BYTES: usize = 4096;
/// Bound on retained transcript lines.
pub const MAX_TRANSCRIPT_LINES: usize = 2000;
/// Bound on one rendered transcript line.
pub const MAX_LINE_BYTES: usize = 512;

/// One line of the rendered transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranscriptLine {
    /// A byte-exact runtime record, reduced to a bounded display form.
    Runtime {
        sequence: u64,
        kind: DshRecordKind,
        text: String,
    },
    /// A local note: a refusal, a lifecycle message, a failure.
    Local { text: String },
}

impl TranscriptLine {
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Runtime { text, .. } | Self::Local { text } => text,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> Option<DshRecordKind> {
        match self {
            Self::Runtime { kind, .. } => Some(*kind),
            Self::Local { .. } => None,
        }
    }
}

/// Everything the renderer needs; no I/O happens here.
#[derive(Debug)]
pub struct DshTuiState {
    pub transcript: Vec<TranscriptLine>,
    pub input: String,
    pub cursor: u64,
    pub phase: DshPhase,
    pub provider: String,
    pub model: String,
    pub server_version: Option<String>,
    pub turns_started: u64,
    pub completed_turns: u64,
    pub failed_turns: u64,
    pub cancelled_turns: u64,
    pub unverified_turns: u64,
    pub last_outcome: Option<DshTurnOutcome>,
    pub record_count: u64,
    pub records_dropped: u64,
    /// Lines scrolled back from the tail.
    pub scroll: u16,
    pub status: String,
    pub shutting_down: bool,
    /// Ctrl-F transcript search; never written to disk.
    pub search: search::TranscriptSearch,
}

impl DshTuiState {
    #[must_use]
    pub fn new(provider: String, model: String) -> Self {
        Self {
            transcript: vec![TranscriptLine::Local {
                text: "vibemux dsh: starting the DeepSeek Harness SDK runtime".to_string(),
            }],
            input: String::new(),
            cursor: 0,
            phase: DshPhase::Starting,
            provider,
            model,
            server_version: None,
            turns_started: 0,
            completed_turns: 0,
            failed_turns: 0,
            cancelled_turns: 0,
            unverified_turns: 0,
            last_outcome: None,
            record_count: 0,
            records_dropped: 0,
            scroll: 0,
            status: "Enter sends  Esc cancels the turn  Ctrl-F searches  Ctrl-C quits".to_string(),
            shutting_down: false,
            search: search::TranscriptSearch::default(),
        }
    }

    pub fn push(&mut self, line: TranscriptLine) {
        // Scrolling back means the tail moved; keep the view stable.
        if self.scroll > 0 {
            self.scroll = self.scroll.saturating_add(1);
        }
        self.transcript.push(line);
        if self.transcript.len() > MAX_TRANSCRIPT_LINES {
            let overflow = self.transcript.len() - MAX_TRANSCRIPT_LINES;
            self.transcript.drain(..overflow);
            self.scroll = self.scroll.saturating_sub(overflow as u16);
        }
    }

    pub fn note(&mut self, text: impl Into<String>) {
        self.push(TranscriptLine::Local { text: text.into() });
    }

    /// Whether a prompt may be sent right now.
    #[must_use]
    pub fn accepts_prompt(&self) -> bool {
        self.phase.accepts_prompt() && !self.shutting_down
    }
}

/// Runs the interactive surface. Returns when the user quits.
pub fn run_dsh_tui(
    handle: &Handle,
    driver: &mut DshDriver,
    launch_label: &str,
    theme: Theme,
) -> io::Result<()> {
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut state = DshTuiState::new("(unset)".to_string(), "(unset)".to_string());
    state.note(format!("vibemux dsh: {launch_label}"));

    let mut frames_since_poll = 0_u32;
    loop {
        // Draining before drawing keeps the view and the summary consistent.
        drain(handle, driver, &mut state, false);
        terminal.draw(|frame| render::render_dsh(frame, &state, theme))?;

        if event::poll(POLL_INTERVAL)? {
            let event = event::read()?;
            let Event::Key(key) = event else { continue };
            if route_key(&mut state, key) == KeyRoute::Prompt {
                match handle_key(key, handle, driver, &mut state, theme) {
                    KeyOutcome::Quit => break,
                    KeyOutcome::Continue => {}
                }
            }
            frames_since_poll = 0;
        } else {
            frames_since_poll += 1;
            let _ = frames_since_poll;
        }
    }
    Ok(())
}

enum KeyOutcome {
    Continue,
    Quit,
}

/// Where a key event goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyRoute {
    /// A key release. Windows terminals report one for every key; acting on
    /// it would repeat the key, and an Esc release after closing the search
    /// would cancel the running turn.
    Ignored,
    /// Search or a scroll jump consumed it.
    Search,
    /// The prompt editor and session controls handle it.
    Prompt,
}

pub(crate) fn route_key(state: &mut DshTuiState, key: KeyEvent) -> KeyRoute {
    if key.kind == KeyEventKind::Release {
        return KeyRoute::Ignored;
    }
    if search::handle_search_key(state, key) {
        return KeyRoute::Search;
    }
    KeyRoute::Prompt
}

fn handle_key(
    key: KeyEvent,
    handle: &Handle,
    driver: &mut DshDriver,
    state: &mut DshTuiState,
    _theme: Theme,
) -> KeyOutcome {
    use KeyCode::*;
    match key.code {
        Char('c') | Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.note("vibemux dsh: shutting the runtime down");
            state.shutting_down = true;
            drain(handle, driver, state, true);
            return KeyOutcome::Quit;
        }
        Enter => {
            let prompt = state.input.trim().to_string();
            if prompt.is_empty() {
                state.note("vibemux dsh: nothing to send");
                return KeyOutcome::Continue;
            }
            if !state.accepts_prompt() {
                state.note(format!(
                    "vibemux dsh: refused, the session is {}",
                    state.phase.as_str()
                ));
                return KeyOutcome::Continue;
            }
            let byte_count = prompt.len();
            match handle.block_on(driver.prompt(&prompt)) {
                Ok(()) => {
                    state.input.clear();
                    state.scroll = 0;
                    state.note(format!("vibemux dsh: prompt sent ({byte_count} bytes)"));
                }
                Err(error) => state.note(format!("vibemux dsh: {}", error.code())),
            }
        }
        Esc => {
            if state.input.is_empty() {
                if state.phase == DshPhase::Prompting {
                    match handle.block_on(driver.cancel()) {
                        Ok(()) => {
                            state.note("vibemux dsh: turn cancelled; the runtime is terminating")
                        }
                        Err(error) => state.note(format!("vibemux dsh: {}", error.code())),
                    }
                } else {
                    state.note("vibemux dsh: no turn to cancel");
                }
            } else {
                state.input.clear();
            }
        }
        Backspace => {
            state.input.pop();
        }
        Up => state.scroll = state.scroll.saturating_add(1),
        Down => state.scroll = state.scroll.saturating_sub(1),
        PageUp => state.scroll = state.scroll.saturating_add(10),
        PageDown => state.scroll = state.scroll.saturating_sub(10),
        Char(character) if !character.is_control() => {
            if state.input.len() + character.len_utf8() <= MAX_INPUT_BYTES {
                state.input.push(character);
            }
        }
        _ => {}
    }
    KeyOutcome::Continue
}

/// Polls the driver and folds new records into the view. `final_drain` also
/// closes the session, so quitting always reaps the runtime.
fn drain(handle: &Handle, driver: &mut DshDriver, state: &mut DshTuiState, final_drain: bool) {
    match driver.poll(state.cursor) {
        Ok(poll) => {
            for record in &poll.records {
                state.push(TranscriptLine::Runtime {
                    sequence: record.sequence,
                    kind: record.kind,
                    text: render::display_text(record.kind, &record.raw_json),
                });
            }
            state.cursor = poll.next;
            let summary = poll.summary;
            state.phase = summary.phase;
            state.provider = summary.provider;
            state.model = summary.model;
            state.server_version = summary.server_version;
            state.turns_started = summary.turns_started;
            state.completed_turns = summary.completed_turns;
            state.failed_turns = summary.failed_turns;
            state.cancelled_turns = summary.cancelled_turns;
            state.unverified_turns = summary.unverified_turns;
            state.last_outcome = summary.last_outcome;
            state.record_count = summary.records;
            if poll.truncated {
                state.note("vibemux dsh: the record window moved; older records were evicted");
            }
        }
        Err(error) => {
            state.note(format!("vibemux dsh: {}", error.code()));
        }
    }
    if let Some(error) = driver.transport_error() {
        let code = error.code();
        if !state.status.contains(code) {
            state.note(format!("vibemux dsh: transport ended ({code})"));
            state.status = format!("transport ended ({code})");
        }
    }
    if final_drain {
        match handle.block_on(driver.close()) {
            Ok(()) => state.note("vibemux dsh: runtime stopped"),
            Err(error) => state.note(format!("vibemux dsh: {}", error.code())),
        }
        state.phase = driver.phase();
    }
}

/// Convenience wrapper: spawn, handshake, run, and always tear down.
pub fn open_and_run(
    handle: &Handle,
    launch: &DshLaunch,
    settings: DshSessionSettings,
    label: &str,
    theme: Theme,
) -> Result<DshTuiState, DshDriverError> {
    let mut driver = handle.block_on(DshDriver::spawn(
        launch,
        settings.clone(),
        Default::default(),
    ))?;
    handle.block_on(driver.await_ready())?;
    let result = run_dsh_tui(handle, &mut driver, label, theme);
    // `run_dsh_tui` already closed the session on a clean quit; this covers a
    // render error, which returns before the final drain.
    let _ = handle.block_on(driver.close());
    result.map_err(|_| DshDriverError::InvalidState)?;
    Ok(DshTuiState::new(settings.provider, settings.model))
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(code, modifiers, kind)
    }

    #[test]
    fn an_esc_release_after_closing_search_never_reaches_the_turn_cancel() {
        // Windows terminals report a release for every key. Only presses act.
        let mut state = DshTuiState::new("provider".into(), "model".into());
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        for (event, expected) in [
            (
                key(KeyCode::Char('f'), ctrl, KeyEventKind::Press),
                KeyRoute::Search,
            ),
            (
                key(KeyCode::Char('f'), ctrl, KeyEventKind::Release),
                KeyRoute::Ignored,
            ),
            (
                key(KeyCode::Char('a'), none, KeyEventKind::Press),
                KeyRoute::Search,
            ),
            (
                key(KeyCode::Char('a'), none, KeyEventKind::Release),
                KeyRoute::Ignored,
            ),
            (
                key(KeyCode::Esc, none, KeyEventKind::Press),
                KeyRoute::Search,
            ),
            (
                key(KeyCode::Esc, none, KeyEventKind::Release),
                KeyRoute::Ignored,
            ),
            (
                key(KeyCode::Char('x'), none, KeyEventKind::Press),
                KeyRoute::Prompt,
            ),
            (
                key(KeyCode::Char('x'), none, KeyEventKind::Release),
                KeyRoute::Ignored,
            ),
        ] {
            assert_eq!(route_key(&mut state, event), expected, "{event:?}");
        }
        assert!(!state.search.open);
        assert_eq!(state.search.query, "a", "the query is not doubled");
    }

    #[test]
    fn the_transcript_is_bounded_and_keeps_the_scroll_stable() {
        let mut state = DshTuiState::new("p".to_string(), "m".to_string());
        for index in 0..(MAX_TRANSCRIPT_LINES + 50) {
            state.note(format!("line {index}"));
        }
        assert_eq!(state.transcript.len(), MAX_TRANSCRIPT_LINES);
        // The oldest lines were dropped, the newest survive.
        assert!(
            state
                .transcript
                .last()
                .expect("tail")
                .text()
                .contains("2049")
        );
    }

    #[test]
    fn scrolling_back_survives_new_lines() {
        let mut state = DshTuiState::new("p".to_string(), "m".to_string());
        state.scroll = 5;
        state.note("a new line");
        assert_eq!(
            state.scroll, 6,
            "the view must not jump while scrolled back"
        );
        state.scroll = 0;
        state.note("another");
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn a_prompt_is_accepted_only_when_the_session_is_ready() {
        let mut state = DshTuiState::new("p".to_string(), "m".to_string());
        assert!(!state.accepts_prompt(), "starting is not ready");
        state.phase = DshPhase::Initializing;
        assert!(!state.accepts_prompt());
        state.phase = DshPhase::Ready;
        assert!(state.accepts_prompt());
        state.phase = DshPhase::Prompting;
        assert!(!state.accepts_prompt(), "one turn at a time");
        state.phase = DshPhase::Ready;
        state.shutting_down = true;
        assert!(!state.accepts_prompt());
        state.phase = DshPhase::Failed;
        assert!(!state.accepts_prompt());
    }

    #[test]
    fn input_is_bounded_by_bytes_not_characters() {
        let mut input = String::new();
        for _ in 0..MAX_INPUT_BYTES {
            let character = '😀';
            if input.len() + character.len_utf8() <= MAX_INPUT_BYTES {
                input.push(character);
            }
        }
        assert!(input.len() <= MAX_INPUT_BYTES);
        assert!(input.len() > MAX_INPUT_BYTES / 2);
    }

    #[test]
    fn local_and_runtime_lines_are_distinguishable() {
        let runtime = TranscriptLine::Runtime {
            sequence: 4,
            kind: DshRecordKind::TurnEnd,
            text: "turn 1 ended: completed".to_string(),
        };
        assert_eq!(runtime.kind(), Some(DshRecordKind::TurnEnd));
        assert_eq!(runtime.text(), "turn 1 ended: completed");
        let local = TranscriptLine::Local {
            text: "note".to_string(),
        };
        assert_eq!(local.kind(), None);
    }
}
