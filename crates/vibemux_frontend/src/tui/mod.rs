#![forbid(unsafe_code)]
//! Read-only TUI debug view. Per-harness details and "native-TUI
//! reservation" copy moved to the GUI; this module is intentionally
//! minimal and ASCII-only.

mod footer;
mod render;
pub mod snapshot;

use std::{
    io,
    sync::{Arc, atomic::AtomicU8},
    time::Duration,
};

use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::theme::{ThemeId, palette_for};
use crate::view_model::ViewModel;

pub use render::render_debug_dashboard;
pub use snapshot::debug_snapshot;

/// Run the TUI debug view. Returns when the user quits (`q`/`Esc`).
/// Pressing `c` writes the ASCII snapshot to stdout; pressing `t`
/// cycles the theme.
pub fn run_tui(view_model: &ViewModel) -> io::Result<()> {
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // Theme is shared between the render and the footer via an atomic
    // u8 so the renderer can read it cheaply every frame. The TUI does
    // not persist this state across runs; each launch reads the GUI's
    // config file on its own (this is honest: pre-alpha, single-process
    // at a time).
    let theme_id = Arc::new(AtomicU8::new(load_initial_theme() as u8));
    let snapshot_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));

    loop {
        let id = ThemeId::ALL
            [theme_id.load(std::sync::atomic::Ordering::Relaxed) as usize % ThemeId::ALL.len()];
        let palette = palette_for(id);
        terminal.draw(|frame| {
            render::render_debug_dashboard(frame, view_model, &palette);
            footer::render_footer(frame, view_model, id);
        })?;
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') => {
                        snapshot_pending.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    KeyCode::Char('t') => {
                        let next = id.cycle();
                        theme_id.store(next as u8, std::sync::atomic::Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
        }
        if snapshot_pending.swap(false, std::sync::atomic::Ordering::Relaxed) {
            let snap = snapshot::debug_snapshot(view_model);
            // We deliberately write to stdout (not the alternate screen)
            // by leaving the alternate screen temporarily.
            execute!(io::stdout(), LeaveAlternateScreen)?;
            println!("{snap}");
            execute!(io::stdout(), EnterAlternateScreen)?;
        }
    }
    Ok(())
}

/// Initial theme for the TUI. Reads from the GUI's persisted config so
/// the two UIs agree on first launch; falls back to `Claude` when no
/// config exists.
fn load_initial_theme() -> ThemeId {
    crate::theme::serialize::load_user_config().theme
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
    }
}
