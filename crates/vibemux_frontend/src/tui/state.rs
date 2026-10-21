//! Diagnostic TUI key state: selection, theme, and the one re-probe in
//! flight. Pure; the run loop performs the I/O a command asks for.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use super::Theme;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebugCommand {
    Continue,
    Quit,
    /// Print the ASCII snapshot.
    Snapshot,
    /// Re-run the read-only probe; asked at most once at a time.
    Reprobe,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DebugState {
    pub selected: usize,
    /// A re-probe is running; `r` is ignored until its report arrives.
    pub probing: bool,
    pub theme: Theme,
}

impl DebugState {
    pub fn handle_key(&mut self, key: KeyEvent, agent_count: usize) -> DebugCommand {
        if key.kind == KeyEventKind::Release {
            return DebugCommand::Continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return DebugCommand::Quit,
            KeyCode::Char('c') => return DebugCommand::Snapshot,
            KeyCode::Char('t') => self.theme = self.theme.next(),
            KeyCode::Char('r') if !self.probing => {
                self.probing = true;
                return DebugCommand::Reprobe;
            }
            KeyCode::Down | KeyCode::Char('j') if agent_count > 0 => {
                self.selected = (self.selected.min(agent_count - 1) + 1) % agent_count;
            }
            KeyCode::Up | KeyCode::Char('k') if agent_count > 0 => {
                self.selected =
                    (self.selected.min(agent_count - 1) + agent_count - 1) % agent_count;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Up | KeyCode::Char('k') => {
                self.selected = 0;
            }
            _ => {}
        }
        DebugCommand::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn r_requests_one_probe_at_a_time() {
        let mut state = DebugState::default();
        assert_eq!(
            state.handle_key(KeyCode::Char('r').into(), 11),
            DebugCommand::Reprobe
        );
        assert!(state.probing);
        assert_eq!(
            state.handle_key(KeyCode::Char('r').into(), 11),
            DebugCommand::Continue
        );
        state.probing = false;
        assert_eq!(
            state.handle_key(KeyCode::Char('r').into(), 11),
            DebugCommand::Reprobe
        );
    }

    #[test]
    fn selection_wraps_and_q_quits() {
        let mut state = DebugState::default();
        state.handle_key(KeyCode::Char('k').into(), 11);
        assert_eq!(state.selected, 10);
        state.handle_key(KeyCode::Down.into(), 11);
        assert_eq!(state.selected, 0);
        state.handle_key(KeyCode::Char('j').into(), 0);
        assert_eq!(state.selected, 0);
        let theme = state.theme;
        assert_eq!(
            state.handle_key(KeyCode::Char('t').into(), 11),
            DebugCommand::Continue
        );
        assert_eq!(state.theme, theme.next());
        assert_eq!(
            state.handle_key(KeyCode::Char('c').into(), 11),
            DebugCommand::Snapshot
        );
        assert_eq!(
            state.handle_key(KeyCode::Char('q').into(), 11),
            DebugCommand::Quit
        );
        assert_eq!(
            state.handle_key(KeyCode::Esc.into(), 11),
            DebugCommand::Quit
        );
    }
}
