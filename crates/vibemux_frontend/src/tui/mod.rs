#![forbid(unsafe_code)]
//! Read-only TUI debug view. Per-harness details and "native-TUI
//! reservation" copy moved to the GUI; this module renders the audited
//! theme system and the per-harness agent table.

mod footer;
pub mod render;
pub mod reprobe;
pub mod snapshot;
pub mod state;
pub mod theme;

use std::{io, time::Duration};

use crossterm::{
    cursor::Show,
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::view_model::ViewModel;

pub use render::render_debug_dashboard;
pub use snapshot::debug_snapshot;
pub use theme::Theme;

use reprobe::{ProbeConfig, Reprobe};
use state::{DebugCommand, DebugState};

/// Run the TUI debug view. Returns when the user quits (`q`/`Esc`).
/// `c` writes the ASCII snapshot to stdout, `t` cycles the theme, `j`/`k`
/// select a harness, and `r` re-runs the read-only probe with `config` on a
/// worker thread (ignored without a config or while a re-probe runs).
///
/// The TUI keeps its own audited `Theme` system (classic /
/// high-contrast / mono / light / nord / gruvbox) and deliberately does
/// not mirror the GUI's persisted hex palettes: the debug view is a
/// terminal surface governed by terminal color semantics, and the GUI
/// remains the human-persistent interface.
pub fn run_tui(
    mut view_model: ViewModel,
    theme: Theme,
    config: Option<ProbeConfig>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut state = DebugState {
        theme,
        ..DebugState::default()
    };
    let mut pending: Option<Reprobe> = None;

    loop {
        if let Some(report) = pending.as_mut().and_then(Reprobe::try_take) {
            view_model = ViewModel::from_report(&report);
            state.probing = false;
            pending = None;
        }
        terminal.draw(|frame| {
            render::render_debug_dashboard(frame, &view_model, state.theme, Some(state.selected));
            footer::render_footer(
                frame,
                &view_model,
                state.theme,
                Some(state.selected),
                state.probing,
            );
        })?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        match state.handle_key(key, view_model.agents.len()) {
            DebugCommand::Quit => break,
            DebugCommand::Continue => {}
            DebugCommand::Reprobe => match config.clone().map(Reprobe::start) {
                Some(Ok(reprobe)) => pending = Some(reprobe),
                // No config, or the worker could not start: nothing runs.
                Some(Err(_)) | None => state.probing = false,
            },
            DebugCommand::Snapshot => {
                let snap = snapshot::debug_snapshot(&view_model);
                // We deliberately write to stdout (not the alternate screen)
                // by leaving the alternate screen temporarily.
                execute!(io::stdout(), LeaveAlternateScreen)?;
                println!("{snap}");
                execute!(io::stdout(), EnterAlternateScreen)?;
            }
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
