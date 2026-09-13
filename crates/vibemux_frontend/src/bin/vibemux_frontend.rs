use std::{io, time::Duration};

use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use vibemux_frontend::{DashboardModel, Theme, render_dashboard_with_theme};
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
    let mut once = false;
    let mut as_json = false;
    let mut theme = Theme::from_environment();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--once" => once = true,
            "--json" => as_json = true,
            "--theme" => match arguments.next().as_deref().and_then(Theme::from_name) {
                Some(parsed) => theme = parsed,
                None => {
                    eprintln!(
                        "unknown or missing --theme value (expected one of: {})",
                        Theme::ALL
                            .iter()
                            .map(|item| item.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    std::process::exit(4);
                }
            },
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(4);
            }
        }
    }
    let report = run_probe(&ProbeConfig::from_environment()).await;
    if as_json {
        // Machine-readable mode for scripts and CI; identical probe evidence
        // the dashboard renders, with no theme-dependent formatting.
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("probe report serializes")
        );
        return;
    }
    let model = DashboardModel::from_report(&report);
    if once {
        println!("{}", model.plain_snapshot());
        return;
    }
    if let Err(error) = run_interactive(&model, theme) {
        eprintln!("frontend failed: {error}");
        std::process::exit(4);
    }
}

fn run_interactive(model: &DashboardModel, theme: Theme) -> io::Result<()> {
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut theme = theme;
    loop {
        terminal.draw(|frame| render_dashboard_with_theme(frame, model, theme))?;
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    // Live theme switching: cycle the palette without leaving
                    // the dashboard so styles can be compared in place.
                    KeyCode::Char('t') => theme = theme.next(),
                    _ => {}
                }
            }
        }
    }
    terminal.show_cursor()?;
    Ok(())
}
