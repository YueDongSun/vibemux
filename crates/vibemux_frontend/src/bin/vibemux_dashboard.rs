use std::{
    io::{self, IsTerminal},
    path::PathBuf,
};
use vibemux_frontend::dashboard::{Context, DashboardModel, run_interactive};
use vibemux_probe::snapshot;

#[tokio::main]
async fn main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 1 && matches!(arguments[0].to_str(), Some("--help" | "-h")) {
        println!(
            "VibeMux read-only dashboard (the egui console is vibemux_frontend)\n\nUsage: vibemux_dashboard [--once] [--report <saved_report.json>]\nUnified entrypoint: vibemuxctl ui\nNo model invocation or native TUI attachment."
        );
        return;
    }
    if arguments.len() == 1 && arguments[0] == "--version" {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let mut once = false;
    let mut report_path = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].to_str() {
            Some("--once") if !once => {
                once = true;
                index += 1;
            }
            Some("--report") if report_path.is_none() => {
                let Some(value) = arguments.get(index + 1).filter(|value| !value.is_empty()) else {
                    usage();
                };
                report_path = Some(PathBuf::from(value));
                index += 2;
            }
            _ => usage(),
        }
    }
    if !once && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
        eprintln!("ui_terminal_required");
        std::process::exit(4);
    }
    let snapshot = match snapshot::load(report_path).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("{}", error.code());
            std::process::exit(4);
        }
    };
    let model = DashboardModel::from_report(&snapshot.report);
    if once {
        println!("{}\nsource: {}", model.plain_snapshot(), snapshot.source);
        return;
    }
    let context = Context {
        source: snapshot.source.to_string(),
        ..Context::default()
    };
    let result = tokio::task::spawn_blocking(move || run_interactive(model, context)).await;
    if !matches!(result, Ok(Ok(()))) {
        eprintln!("ui_terminal_failed");
        std::process::exit(4);
    }
}

fn usage() -> ! {
    eprintln!("ui_invalid_arguments");
    std::process::exit(2)
}
