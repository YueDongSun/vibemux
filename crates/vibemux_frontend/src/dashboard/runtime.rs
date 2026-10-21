//! Terminal ownership, the key loop, and deterministic in-memory snapshots.
use super::{
    Command, Context, DashboardModel, State, Tab,
    daemon_feed::{DaemonFeed, FeedRequest},
    list_counts, render_with_state, visible_task_ids,
};
use crate::tui::Theme;
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

/// The read a command asks of the daemon feed. `OpenTask` indexes the task
/// rows the filter leaves, which is what the operator sees.
#[must_use]
pub fn feed_request(command: Command, context: &Context, state: &State) -> Option<FeedRequest> {
    match command {
        Command::Refresh => Some(FeedRequest::Refresh),
        Command::OpenTask(index) => context
            .daemon
            .as_deref()
            .and_then(|daemon| {
                visible_task_ids(daemon, state.filter.query(Tab::Tasks))
                    .into_iter()
                    .nth(index)
            })
            .map(FeedRequest::OpenTask),
        Command::Continue | Command::Quit | Command::CycleTheme => None,
    }
}

/// Runs the console until the operator quits. With a feed, the daemon views
/// show its newest observation; without one, they say they need it.
pub fn run_interactive(
    model: DashboardModel,
    mut context: Context,
    mut feed: Option<DaemonFeed>,
) -> io::Result<()> {
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
    let mut state = State {
        theme: Theme::from_environment(),
        ..State::default()
    };
    loop {
        if let Some(view) = feed.as_mut().and_then(DaemonFeed::latest) {
            context.daemon = Some(view);
        }
        terminal.draw(|frame| render_with_state(frame, &model, &state, &context))?;
        if !event::poll(EVENT_POLL_INTERVAL)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        let size = terminal.size()?;
        let counts = list_counts(&model, &context, &state);
        let command = state.handle_key(key, counts, (size.width, size.height));
        match command {
            Command::Quit => break,
            Command::CycleTheme => state.theme = state.theme.next(),
            _ => {
                if let (Some(feed), Some(request)) =
                    (feed.as_ref(), feed_request(command, &context, &state))
                {
                    // A full queue drops the request; the next poll still runs.
                    let _ = feed.request(request);
                }
            }
        }
    }
    terminal.show_cursor()?;
    drop(feed);
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
