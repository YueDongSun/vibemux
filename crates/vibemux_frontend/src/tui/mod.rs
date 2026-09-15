#![forbid(unsafe_code)]
//! Read-only TUI debug view. Per-harness details and "native-TUI
//! reservation" copy moved to the GUI; this module renders the audited
//! theme system and the ten-agent table.

mod footer;
pub mod render;
pub mod snapshot;
pub mod theme;

use std::{io, time::Duration};

use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::view_model::ViewModel;

pub use render::render_debug_dashboard;
pub use snapshot::debug_snapshot;
pub use theme::Theme;

/// Run the TUI debug view. Returns when the user quits (`q`/`Esc`).
/// Pressing `c` writes the ASCII snapshot to stdout; pressing `t`
/// cycles the theme.
///
/// The TUI keeps its own audited `Theme` system (classic /
/// high-contrast / mono / light) and deliberately does not mirror the
/// GUI's persisted hex palettes: the debug view is a terminal surface
/// governed by terminal color semantics, and the GUI remains the
/// human-persistent interface.
pub fn run_tui(view_model: &ViewModel, theme: Theme) -> io::Result<()> {
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut theme = theme;
    let mut snapshot_pending = false;

    loop {
        terminal.draw(|frame| {
            render::render_debug_dashboard(frame, view_model, theme);
            footer::render_footer(frame, view_model, theme);
        })?;
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') => {
                        snapshot_pending = true;
                    }
                    KeyCode::Char('t') => {
                        theme = theme.next();
                    }
                    _ => {}
                }
            }
        }
        if snapshot_pending {
            let snap = snapshot::debug_snapshot(view_model);
            // We deliberately write to stdout (not the alternate screen)
            // by leaving the alternate screen temporarily.
            execute!(io::stdout(), LeaveAlternateScreen)?;
            println!("{snap}");
            execute!(io::stdout(), EnterAlternateScreen)?;
            snapshot_pending = false;
        }
    }
    Ok(())
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
    }
}
