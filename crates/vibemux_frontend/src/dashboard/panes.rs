//! Console layouts: which views share the terminal and which pane has focus.
//! Pure presentation state; nothing here reads the daemon or the terminal.
use super::Tab;

/// Narrowest terminal that shows two panes side by side.
pub const SPLIT_MIN_WIDTH: u16 = 90;
/// Smallest terminal that shows the two-by-two grid.
pub const GRID_MIN_WIDTH: u16 = 120;
pub const GRID_MIN_HEIGHT: u16 = 32;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LayoutMode {
    #[default]
    Single,
    Split,
    Grid,
}

impl LayoutMode {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Single => Self::Split,
            Self::Split => Self::Grid,
            Self::Grid => Self::Single,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Split => "split",
            Self::Grid => "grid",
        }
    }
}

/// The layout the terminal can actually show: a grid that does not fit
/// falls back to a split, and a split that does not fit to one view.
#[must_use]
pub const fn effective_layout(mode: LayoutMode, width: u16, height: u16) -> LayoutMode {
    match mode {
        LayoutMode::Grid if width >= GRID_MIN_WIDTH && height >= GRID_MIN_HEIGHT => {
            LayoutMode::Grid
        }
        LayoutMode::Grid | LayoutMode::Split if width >= SPLIT_MIN_WIDTH => LayoutMode::Split,
        _ => LayoutMode::Single,
    }
}

/// Panes drawn by `mode`.
#[must_use]
pub const fn pane_count(mode: LayoutMode) -> usize {
    match mode {
        LayoutMode::Single => 1,
        LayoutMode::Split => 2,
        LayoutMode::Grid => 4,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneState {
    /// The layout the operator chose; the terminal may show less.
    pub mode: LayoutMode,
    /// The view of each pane; a split uses the first two.
    pub views: [Tab; 4],
    pub focus: usize,
    /// Shows the focused pane alone, in the full body.
    pub zoomed: bool,
}

impl Default for PaneState {
    fn default() -> Self {
        Self {
            mode: LayoutMode::Single,
            views: [Tab::Overview, Tab::Tasks, Tab::Daemon, Tab::Sessions],
            focus: 0,
            zoomed: false,
        }
    }
}

impl PaneState {
    /// The focused pane's view under `effective`, never past the panes that
    /// layout draws.
    #[must_use]
    pub fn focused_view(&self, effective: LayoutMode) -> Tab {
        self.views[self.focus.min(pane_count(effective) - 1)]
    }

    pub(crate) fn move_focus(&mut self, effective: LayoutMode, forward: bool) {
        let count = pane_count(effective);
        let current = self.focus.min(count - 1);
        self.focus = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
    }

    pub(crate) fn set_focused_view(&mut self, effective: LayoutMode, view: Tab) {
        self.focus = self.focus.min(pane_count(effective) - 1);
        self.views[self.focus] = view;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_falls_back_by_terminal_size() {
        assert_eq!(
            effective_layout(LayoutMode::Grid, 120, 32),
            LayoutMode::Grid
        );
        assert_eq!(
            effective_layout(LayoutMode::Grid, 119, 40),
            LayoutMode::Split
        );
        assert_eq!(
            effective_layout(LayoutMode::Grid, 100, 24),
            LayoutMode::Split
        );
        assert_eq!(
            effective_layout(LayoutMode::Split, 89, 40),
            LayoutMode::Single
        );
        assert_eq!(
            effective_layout(LayoutMode::Grid, 80, 24),
            LayoutMode::Single
        );
        assert_eq!(
            effective_layout(LayoutMode::Single, 200, 60),
            LayoutMode::Single
        );
    }

    #[test]
    fn focus_wraps_within_the_drawn_panes() {
        let mut panes = PaneState {
            focus: 3,
            ..PaneState::default()
        };
        // A grid that fell back to a split draws two panes.
        assert_eq!(panes.focused_view(LayoutMode::Split), Tab::Tasks);
        panes.move_focus(LayoutMode::Split, true);
        assert_eq!(panes.focus, 0);
        panes.move_focus(LayoutMode::Grid, false);
        assert_eq!(panes.focus, 3);
        panes.set_focused_view(LayoutMode::Split, Tab::Decisions);
        assert_eq!(panes.focus, 1);
        assert_eq!(panes.views[1], Tab::Decisions);
    }
}
