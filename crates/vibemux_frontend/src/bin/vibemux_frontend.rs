use std::{io, time::Duration};

use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use vibemux_frontend::{DashboardModel, render_dashboard};
use vibemux_probe::{ProbeConfig, run_probe};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
    }
}

#[tokio::main]
async fn main() {
    let once = std::env::args()
        .skip(1)
        .any(|argument| argument == "--once");
    let report = run_probe(&ProbeConfig::from_environment()).await;
    let model = DashboardModel::from_report(&report);
    if once {
        println!("{}", model.plain_snapshot());
        return;
    }
    if let Err(error) = run_interactive(&model) {
        eprintln!("frontend failed: {error}");
        std::process::exit(4);
    }
}

fn run_interactive(model: &DashboardModel) -> io::Result<()> {
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    loop {
        terminal.draw(|frame| render_dashboard(frame, model))?;
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    break;
                }
            }
        }
    }
    terminal.show_cursor()?;
    Ok(())
}
