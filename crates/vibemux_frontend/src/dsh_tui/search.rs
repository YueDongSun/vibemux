//! Transcript search for the DSH surface: pure state over the bounded
//! in-memory transcript the screen already shows. Nothing is written to disk.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{DshTuiState, TranscriptLine};

/// Longest query, in bytes.
pub const MAX_QUERY_BYTES: usize = 128;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TranscriptSearch {
    pub open: bool,
    pub query: String,
    /// The transcript index of the highlighted match.
    pub current: Option<usize>,
}

pub(crate) fn line_matches(line: &TranscriptLine, query: &str) -> bool {
    !query.is_empty() && line.text().to_lowercase().contains(&query.to_lowercase())
}

/// The newest match strictly before `before`.
#[must_use]
pub fn find_older(lines: &[TranscriptLine], query: &str, before: usize) -> Option<usize> {
    lines[..before.min(lines.len())]
        .iter()
        .rposition(|line| line_matches(line, query))
}

/// The oldest match strictly after `after`.
#[must_use]
pub fn find_newer(lines: &[TranscriptLine], query: &str, after: usize) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .skip(after.saturating_add(1))
        .find(|(_, line)| line_matches(line, query))
        .map(|(index, _)| index)
}

#[must_use]
pub fn match_count(lines: &[TranscriptLine], query: &str) -> usize {
    lines
        .iter()
        .filter(|line| line_matches(line, query))
        .count()
}

/// Scrolls so that transcript line `index` is the bottom line in view.
fn scroll_to(state: &mut DshTuiState, index: usize) {
    let from_tail = state.transcript.len().saturating_sub(index + 1);
    state.scroll = u16::try_from(from_tail).unwrap_or(u16::MAX);
}

fn select(state: &mut DshTuiState, found: Option<usize>) {
    if let Some(index) = found {
        state.search.current = Some(index);
        scroll_to(state, index);
    }
}

/// Applies search and jump keys. Returns true when the key was consumed;
/// otherwise the prompt editor handles it. While search is open every
/// editing key is consumed, so `Enter` never sends a prompt and `Esc` never
/// cancels a turn; `Ctrl-C` and `Ctrl-Q` still quit.
pub fn handle_search_key(state: &mut DshTuiState, key: KeyEvent) -> bool {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    if control && key.code == KeyCode::Char('f') {
        state.search.open = true;
        let newest = find_older(
            &state.transcript,
            &state.search.query,
            state.transcript.len(),
        );
        select(state, newest);
        return true;
    }
    if !state.search.open {
        match key.code {
            KeyCode::Home => scroll_to(state, 0),
            KeyCode::End => state.scroll = 0,
            _ => return false,
        }
        return true;
    }
    if control || key.modifiers.contains(KeyModifiers::ALT) {
        return false;
    }
    let lines = &state.transcript;
    match key.code {
        KeyCode::Esc => {
            state.search.open = false;
            state.search.current = None;
        }
        KeyCode::Enter | KeyCode::Down => {
            let before = state.search.current.unwrap_or(lines.len());
            let found = find_older(lines, &state.search.query, before);
            select(state, found);
        }
        KeyCode::Up => {
            let found = state
                .search
                .current
                .and_then(|after| find_newer(lines, &state.search.query, after));
            select(state, found);
        }
        KeyCode::Backspace => {
            state.search.query.pop();
            state.search.current = None;
            let newest = find_older(
                &state.transcript,
                &state.search.query,
                state.transcript.len(),
            );
            select(state, newest);
        }
        KeyCode::Char(character) if !character.is_control() => {
            if state.search.query.len() + character.len_utf8() <= MAX_QUERY_BYTES {
                state.search.query.push(character);
            }
            state.search.current = None;
            let newest = find_older(
                &state.transcript,
                &state.search.query,
                state.transcript.len(),
            );
            select(state, newest);
        }
        KeyCode::Home => scroll_to(state, 0),
        KeyCode::End => state.scroll = 0,
        // Page keys keep scrolling the transcript.
        KeyCode::PageUp | KeyCode::PageDown => return false,
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsh_tui::{DshTuiState, TranscriptLine};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn state_with(lines: &[&str]) -> DshTuiState {
        let mut state = DshTuiState::new("provider".into(), "model".into());
        state.transcript.clear();
        for (index, text) in lines.iter().enumerate() {
            state.push(TranscriptLine::Runtime {
                sequence: index as u64,
                kind: vibemux_harness::dsh::DshRecordKind::Text,
                text: (*text).to_string(),
            });
        }
        state
    }

    fn key(state: &mut DshTuiState, code: KeyCode) -> bool {
        handle_search_key(state, code.into())
    }

    fn ctrl_f(state: &mut DshTuiState) -> bool {
        handle_search_key(
            state,
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
        )
    }

    #[test]
    fn search_moves_between_matches_and_scrolls() {
        let mut state = state_with(&["Alpha one", "beta", "alpha two", "gamma"]);
        assert!(ctrl_f(&mut state));
        assert!(state.search.open);
        for character in "alpha".chars() {
            assert!(key(&mut state, KeyCode::Char(character)));
        }
        // Typing jumps to the newest match.
        assert_eq!(state.search.current, Some(2));
        assert_eq!(state.scroll, 1);
        assert!(key(&mut state, KeyCode::Enter));
        assert_eq!(state.search.current, Some(0));
        assert_eq!(state.scroll, 3);
        // No older match: the current one stays.
        assert!(key(&mut state, KeyCode::Down));
        assert_eq!(state.search.current, Some(0));
        assert!(key(&mut state, KeyCode::Up));
        assert_eq!(state.search.current, Some(2));
        assert_eq!(match_count(&state.transcript, &state.search.query), 2);
        assert_eq!(state.input, "", "search keys never reach the prompt");
    }

    #[test]
    fn esc_in_search_closes_search_without_consuming_turn_cancel() {
        let mut state = state_with(&["alpha"]);
        assert!(
            !key(&mut state, KeyCode::Esc),
            "closed: Esc cancels the turn"
        );
        ctrl_f(&mut state);
        assert!(key(&mut state, KeyCode::Esc));
        assert!(!state.search.open);
        assert!(!key(&mut state, KeyCode::Esc));
        // Ctrl-C still quits while searching.
        ctrl_f(&mut state);
        assert!(!handle_search_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
    }

    #[test]
    fn home_and_end_jump() {
        let mut state = state_with(&["a", "b", "c", "d"]);
        assert!(key(&mut state, KeyCode::Home));
        assert_eq!(state.scroll, 3);
        assert!(key(&mut state, KeyCode::End));
        assert_eq!(state.scroll, 0);
        // Typing other keys outside search is the prompt's business.
        assert!(!key(&mut state, KeyCode::Char('x')));
        assert!(!key(&mut state, KeyCode::Enter));
    }

    #[test]
    fn the_query_is_bounded_and_matching_ignores_case() {
        let mut state = state_with(&["ALPHA"]);
        ctrl_f(&mut state);
        for _ in 0..MAX_QUERY_BYTES + 10 {
            key(&mut state, KeyCode::Char('a'));
        }
        assert_eq!(state.search.query.len(), MAX_QUERY_BYTES);
        assert_eq!(find_older(&state.transcript, "alpha", 1), Some(0));
        assert_eq!(find_newer(&state.transcript, "alpha", 0), None);
        assert_eq!(find_older(&state.transcript, "", 1), None);
    }
}
