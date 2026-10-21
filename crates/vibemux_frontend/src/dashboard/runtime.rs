//! Terminal ownership and deterministic in-memory snapshots.
use super::{Command, Context, DashboardModel, ListCounts, State, render_with_state};
use crossterm::{
    cursor::Show,
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::{CrosstermBackend, TestBackend},
};
use std::{
    io::{self, IsTerminal},
    time::Duration,
};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(250);

struct Guard {
    alternate: bool,
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.alternate {
            let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
        }
    }
}

pub fn run_interactive(model: DashboardModel, context: Context) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "interactive terminal required; use --once",
        ));
    }
    enable_raw_mode()?;
    let guard = Guard { alternate: true };
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut state = State::default();
    loop {
        terminal.draw(|frame| render_with_state(frame, &model, &state, &context))?;
        let size = terminal.size()?;
        let counts = ListCounts {
            agents: model.agents.len(),
            branches: context
                .workspace
                .as_ref()
                .map_or(0, |workspace| workspace.branches.len()),
            ..ListCounts::default()
        };
        if event::poll(EVENT_POLL_INTERVAL)?
            && let Event::Key(key) = event::read()?
            && state.handle_key(key, counts, (size.width, size.height)) == Command::Quit
        {
            break;
        }
    }
    terminal.show_cursor()?;
    drop(guard);
    Ok(())
}

pub fn snapshot(
    model: &DashboardModel,
    state: &State,
    context: &Context,
    width: u16,
    height: u16,
) -> io::Result<String> {
    if width == 0 || height == 0 || width > 240 || height > 80 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot dimensions out of bounds",
        ));
    }
    // `TestBackend` cannot fail; its error type is `Infallible`.
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .map_err(|never| -> io::Error { match never {} })?;
    terminal
        .draw(|frame| render_with_state(frame, model, state, context))
        .map_err(|never| -> io::Error { match never {} })?;
    let buffer = terminal.backend().buffer();
    Ok(buffer
        .content()
        .chunks(usize::from(width))
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n"))
}
