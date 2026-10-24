//! Presentation state only; key presses cannot perform agent or Git operations.
//! The only requests a key can make are daemon reads (`Refresh`, `OpenTask`).
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::filter::FilterState;
use super::panes::{LayoutMode, PaneState, effective_layout};
use crate::tui::Theme;

/// One console view. The first three read the probe report and the Git
/// inventory; the rest read the daemon.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tab {
    #[default]
    Overview,
    Harnesses,
    Workspace,
    Tasks,
    Daemon,
    Sessions,
    Decisions,
}

impl Tab {
    pub const ALL: [Tab; 7] = [
        Self::Overview,
        Self::Harnesses,
        Self::Workspace,
        Self::Tasks,
        Self::Daemon,
        Self::Sessions,
        Self::Decisions,
    ];

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|tab| *tab == self)
            .unwrap_or_default()
    }

    fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }

    /// The view a digit key selects: `1` is the first view.
    fn from_digit(character: char) -> Option<Self> {
        let digit = character.to_digit(10)? as usize;
        (1..=Self::ALL.len())
            .contains(&digit)
            .then(|| Self::ALL[digit - 1])
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Harnesses => "Harnesses",
            Self::Workspace => "Workspace",
            Self::Tasks => "Tasks",
            Self::Daemon => "Daemon",
            Self::Sessions => "Sessions",
            Self::Decisions => "Decisions",
        }
    }

    /// Whether the view reads the daemon rather than the probe report.
    #[must_use]
    pub const fn is_daemon(self) -> bool {
        matches!(
            self,
            Self::Tasks | Self::Daemon | Self::Sessions | Self::Decisions
        )
    }
}

/// What a key asks the runtime to do. Only reads; nothing here starts or
/// changes a daemon, harness, or session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Continue,
    Quit,
    /// Poll the daemon now.
    Refresh,
    /// Read the detail of the task at this index of the visible task list.
    OpenTask(usize),
    CycleTheme,
}

/// Rows in each selectable list as currently drawn (after filtering).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ListCounts {
    pub agents: usize,
    pub branches: usize,
    pub tasks: usize,
    pub decisions: usize,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    /// The view shown in the single layout.
    pub tab: Tab,
    pub panes: PaneState,
    pub selected_agent: usize,
    pub selected_branch: usize,
    pub selected_task: usize,
    pub selected_decision: usize,
    pub help_visible: bool,
    pub filter: FilterState,
    pub theme: Theme,
}

impl State {
    /// The view that keys act on: the single tab, or the focused pane.
    #[must_use]
    pub fn view(&self, width: u16, height: u16) -> Tab {
        match effective_layout(self.panes.mode, width, height) {
            LayoutMode::Single => self.tab,
            effective => self.panes.focused_view(effective),
        }
    }

    /// Maps a key to presentation changes plus at most one read request.
    /// `size` is the terminal size, which decides the layout keys act on.
    pub fn handle_key(&mut self, key: KeyEvent, counts: ListCounts, size: (u16, u16)) -> Command {
        if key.kind == KeyEventKind::Release {
            return Command::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Command::Quit;
        }
        let effective = effective_layout(self.panes.mode, size.0, size.1);
        let view = self.view(size.0, size.1);
        // A filter can shrink a list under its selection between two keys.
        self.clamp_selections(counts);
        if self.filter.handle_editing_key(key) {
            return Command::Continue;
        }
        match key.code {
            KeyCode::Char('q') => return Command::Quit,
            KeyCode::Esc if self.help_visible => self.help_visible = false,
            KeyCode::Esc => {
                if !self.filter.clear(view) {
                    return Command::Quit;
                }
            }
            KeyCode::Char('/') => self.filter.start(view),
            KeyCode::Char('?') => self.help_visible = !self.help_visible,
            KeyCode::Char('r') => return Command::Refresh,
            KeyCode::Char('t') => return Command::CycleTheme,
            KeyCode::Char('L') => {
                self.panes.mode = self.panes.mode.next();
                self.panes.zoomed = false;
            }
            KeyCode::Char('z') if effective != LayoutMode::Single => {
                self.panes.zoomed = !self.panes.zoomed;
            }
            KeyCode::Tab | KeyCode::Right => self.cycle(effective, true),
            KeyCode::BackTab | KeyCode::Left => self.cycle(effective, false),
            KeyCode::Char(character) if Tab::from_digit(character).is_some() => {
                if let Some(tab) = Tab::from_digit(character) {
                    self.show(effective, tab);
                }
            }
            KeyCode::Enter => match view {
                Tab::Tasks if counts.tasks > 0 => {
                    return Command::OpenTask(self.selected_task.min(counts.tasks - 1));
                }
                Tab::Overview | Tab::Harnesses => self.show(effective, Tab::Harnesses),
                _ => {}
            },
            KeyCode::Down | KeyCode::Char('j') => self.select(view, true, counts),
            KeyCode::Up | KeyCode::Char('k') => self.select(view, false, counts),
            _ => {}
        }
        Command::Continue
    }

    fn clamp_selections(&mut self, counts: ListCounts) {
        for (selected, count) in [
            (&mut self.selected_agent, counts.agents),
            (&mut self.selected_branch, counts.branches),
            (&mut self.selected_task, counts.tasks),
            (&mut self.selected_decision, counts.decisions),
        ] {
            *selected = (*selected).min(count.saturating_sub(1));
        }
    }

    fn cycle(&mut self, effective: LayoutMode, forward: bool) {
        if effective == LayoutMode::Single {
            let step = if forward { 1 } else { Tab::ALL.len() - 1 };
            self.tab = Tab::from_index(self.tab.index() + step);
        } else {
            self.panes.move_focus(effective, forward);
        }
    }

    fn show(&mut self, effective: LayoutMode, tab: Tab) {
        if effective == LayoutMode::Single {
            self.tab = tab;
        } else {
            self.panes.set_focused_view(effective, tab);
        }
    }

    fn select(&mut self, view: Tab, forward: bool, counts: ListCounts) {
        let (selected, count) = match view {
            Tab::Overview | Tab::Harnesses => (&mut self.selected_agent, counts.agents),
            Tab::Workspace => (&mut self.selected_branch, counts.branches),
            Tab::Tasks => (&mut self.selected_task, counts.tasks),
            Tab::Decisions => (&mut self.selected_decision, counts.decisions),
            Tab::Daemon | Tab::Sessions => return,
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

pub use vibemux_client::workspace_inventory::{Branch, Workspace};

/// Immutable presentation metadata, independent of optional Git inventory.
#[derive(Clone, Debug)]
pub struct Context {
    pub source: String,
    pub workspace: Option<Workspace>,
    /// The newest daemon observation; `None` when no feed runs (`--once`).
    pub daemon: Option<std::sync::Arc<super::DaemonView>>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            source: "snapshot".to_string(),
            workspace: None,
            daemon: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::panes::LayoutMode;

    const WIDE: (u16, u16) = (130, 40);

    fn counts(agents: usize, branches: usize) -> ListCounts {
        ListCounts {
            agents,
            branches,
            ..ListCounts::default()
        }
    }

    fn press(state: &mut State, code: KeyCode, counts: ListCounts) -> Command {
        state.handle_key(code.into(), counts, WIDE)
    }

    #[test]
    fn navigation_is_bounded_and_never_executes_an_action() {
        let mut state = State::default();
        assert_eq!(
            press(&mut state, KeyCode::Char('k'), counts(5, 2)),
            Command::Continue
        );
        assert_eq!(state.selected_agent, 4);
        press(&mut state, KeyCode::Tab, counts(5, 2));
        assert_eq!(state.tab, Tab::Harnesses);
        press(&mut state, KeyCode::Char('3'), counts(5, 2));
        press(&mut state, KeyCode::Down, counts(5, 2));
        assert_eq!(state.selected_branch, 1);
        press(&mut state, KeyCode::Down, counts(5, 0));
        assert_eq!(state.selected_branch, 0);
        press(&mut state, KeyCode::Char('?'), counts(5, 2));
        assert_eq!(
            press(&mut state, KeyCode::Esc, counts(5, 2)),
            Command::Continue
        );
        assert!(!state.help_visible);
        assert_eq!(
            press(&mut state, KeyCode::Char('q'), counts(5, 2)),
            Command::Quit
        );
    }

    #[test]
    fn tab_and_digits_follow_the_layout() {
        let mut state = State::default();
        press(&mut state, KeyCode::Tab, ListCounts::default());
        assert_eq!(state.tab, Tab::Harnesses);
        press(&mut state, KeyCode::Char('4'), ListCounts::default());
        assert_eq!(state.tab, Tab::Tasks);
        assert_eq!(state.view(WIDE.0, WIDE.1), Tab::Tasks);

        press(&mut state, KeyCode::Char('L'), ListCounts::default());
        assert_eq!(state.panes.mode, LayoutMode::Split);
        press(&mut state, KeyCode::Tab, ListCounts::default());
        assert_eq!(state.panes.focus, 1);
        press(&mut state, KeyCode::Char('7'), ListCounts::default());
        assert_eq!(state.panes.views[1], Tab::Decisions);
        assert_eq!(state.tab, Tab::Tasks, "a pane view is not the single tab");
        assert_eq!(state.view(WIDE.0, WIDE.1), Tab::Decisions);

        press(&mut state, KeyCode::Char('z'), ListCounts::default());
        assert!(state.panes.zoomed);
        assert_eq!(state.view(WIDE.0, WIDE.1), Tab::Decisions);
        press(&mut state, KeyCode::Char('z'), ListCounts::default());
        assert!(!state.panes.zoomed);

        // On a terminal too narrow for a split, the keys act on the tab.
        state.handle_key(KeyCode::Char('2').into(), ListCounts::default(), (80, 24));
        assert_eq!(state.tab, Tab::Harnesses);
        assert_eq!(state.view(80, 24), Tab::Harnesses);
    }

    #[test]
    fn filter_clamps_selection_and_enter_on_empty_list_is_noop() {
        let mut state = State {
            tab: Tab::Tasks,
            selected_task: 2,
            ..State::default()
        };
        let none = ListCounts::default();
        press(&mut state, KeyCode::Char('j'), none);
        assert_eq!(state.selected_task, 0);
        assert_eq!(press(&mut state, KeyCode::Enter, none), Command::Continue);
        // A filter that leaves one row clamps a selection past it.
        state.selected_task = 5;
        let one = ListCounts {
            tasks: 1,
            ..ListCounts::default()
        };
        assert_eq!(press(&mut state, KeyCode::Enter, one), Command::OpenTask(0));
        assert_eq!(state.selected_task, 0);
    }

    #[test]
    fn typing_q_while_editing_does_not_quit() {
        let mut state = State {
            tab: Tab::Tasks,
            ..State::default()
        };
        press(&mut state, KeyCode::Char('/'), ListCounts::default());
        for character in ['q', 'r', 't', 'L', '4'] {
            assert_eq!(
                press(&mut state, KeyCode::Char(character), ListCounts::default()),
                Command::Continue
            );
        }
        assert_eq!(state.filter.query(Tab::Tasks), "qrtL4");
        assert_eq!(state.tab, Tab::Tasks);
        press(&mut state, KeyCode::Enter, ListCounts::default());
        assert_eq!(state.filter.editing, None);
        assert_eq!(state.filter.query(Tab::Tasks), "qrtL4");
    }

    #[test]
    fn esc_clears_filter_before_quitting() {
        let mut state = State {
            tab: Tab::Tasks,
            ..State::default()
        };
        press(&mut state, KeyCode::Char('/'), ListCounts::default());
        press(&mut state, KeyCode::Char('a'), ListCounts::default());
        press(&mut state, KeyCode::Enter, ListCounts::default());
        assert_eq!(
            press(&mut state, KeyCode::Esc, ListCounts::default()),
            Command::Continue
        );
        assert_eq!(state.filter.query(Tab::Tasks), "");
        assert_eq!(
            press(&mut state, KeyCode::Esc, ListCounts::default()),
            Command::Quit
        );
    }

    #[test]
    fn keys_only_request_reads() {
        let mut state = State::default();
        assert_eq!(
            press(&mut state, KeyCode::Char('r'), ListCounts::default()),
            Command::Refresh
        );
        assert_eq!(
            press(&mut state, KeyCode::Char('t'), ListCounts::default()),
            Command::CycleTheme
        );
        let three_tasks = ListCounts {
            tasks: 3,
            ..ListCounts::default()
        };
        press(&mut state, KeyCode::Char('4'), three_tasks);
        press(&mut state, KeyCode::Char('k'), three_tasks);
        assert_eq!(state.selected_task, 2);
        assert_eq!(
            press(&mut state, KeyCode::Enter, three_tasks),
            Command::OpenTask(2)
        );
        let two_decisions = ListCounts {
            decisions: 2,
            ..ListCounts::default()
        };
        press(&mut state, KeyCode::Char('7'), two_decisions);
        press(&mut state, KeyCode::Down, two_decisions);
        assert_eq!(state.selected_decision, 1);
        assert_eq!(
            press(&mut state, KeyCode::Enter, two_decisions),
            Command::Continue
        );
        assert_eq!(
            press(&mut state, KeyCode::Char('q'), ListCounts::default()),
            Command::Quit
        );
    }
}
