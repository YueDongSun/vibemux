//! Presentation state only; key presses cannot perform agent or Git operations.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tab {
    #[default]
    Overview,
    Harnesses,
    Workspace,
}

impl Tab {
    pub fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Harnesses => 1,
            Self::Workspace => 2,
        }
    }
    fn from_index(index: usize) -> Self {
        match index % 3 {
            0 => Self::Overview,
            1 => Self::Harnesses,
            _ => Self::Workspace,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub tab: Tab,
    pub selected_agent: usize,
    pub selected_branch: usize,
    pub help_visible: bool,
}

impl State {
    /// Returns true only to request UI shutdown, never workload cancellation.
    pub fn handle_key(&mut self, key: KeyEvent, agent_count: usize, branch_count: usize) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc if self.help_visible => self.help_visible = false,
            KeyCode::Esc => return true,
            KeyCode::Char('?') => self.help_visible = !self.help_visible,
            KeyCode::Tab | KeyCode::Right => self.tab = Tab::from_index(self.tab.index() + 1),
            KeyCode::BackTab | KeyCode::Left => self.tab = Tab::from_index(self.tab.index() + 2),
            KeyCode::Char('1') => self.tab = Tab::Overview,
            KeyCode::Char('2') => self.tab = Tab::Harnesses,
            KeyCode::Enter if self.tab != Tab::Workspace => self.tab = Tab::Harnesses,
            KeyCode::Char('3') => self.tab = Tab::Workspace,
            KeyCode::Down | KeyCode::Char('j') => self.select(true, agent_count, branch_count),
            KeyCode::Up | KeyCode::Char('k') => self.select(false, agent_count, branch_count),
            _ => {}
        }
        false
    }

    fn select(&mut self, forward: bool, agent_count: usize, branch_count: usize) {
        let (selected, count) = if self.tab == Tab::Workspace {
            (&mut self.selected_branch, branch_count)
        } else {
            (&mut self.selected_agent, agent_count)
        };
        if count == 0 {
            *selected = 0;
            return;
        }
        let current = (*selected).min(count - 1);
        *selected = if forward {
            if current + 1 == count { 0 } else { current + 1 }
        } else if current == 0 {
            count - 1
        } else {
            current - 1
        };
    }
}

#[derive(Clone, Debug)]
pub struct Branch {
    pub name: String,
    pub role: String,
    pub commit: String,
    pub upstream: String,
}

#[derive(Clone, Debug, Default)]
pub struct Workspace {
    pub current_branch: String,
    pub head: String,
    pub baseline: String,
    pub ahead: u64,
    pub behind: u64,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub change_detection: String,
    pub branches: Vec<Branch>,
    pub worktrees: Vec<String>,
}

/// Immutable presentation metadata, independent of optional Git inventory.
#[derive(Clone, Debug)]
pub struct Context {
    pub source: String,
    pub workspace: Option<Workspace>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            source: "snapshot".to_string(),
            workspace: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_is_bounded_and_never_executes_an_action() {
        let mut state = State::default();
        assert!(!state.handle_key(KeyCode::Char('k').into(), 5, 2));
        assert_eq!(state.selected_agent, 4);
        state.handle_key(KeyCode::Tab.into(), 5, 2);
        assert_eq!(state.tab, Tab::Harnesses);
        state.handle_key(KeyCode::Char('3').into(), 5, 2);
        state.handle_key(KeyCode::Down.into(), 5, 2);
        assert_eq!(state.selected_branch, 1);
        state.handle_key(KeyCode::Down.into(), 5, 0);
        assert_eq!(state.selected_branch, 0);
        state.handle_key(KeyCode::Char('?').into(), 5, 2);
        assert!(!state.handle_key(KeyCode::Esc.into(), 5, 2));
        assert!(!state.help_visible);
        assert!(state.handle_key(KeyCode::Char('q').into(), 5, 2));
    }
}
