use std::{
    ffi::OsString,
    io::{self, IsTerminal},
    path::PathBuf,
    sync::Arc,
};
use vibemux_frontend::dashboard::{
    Context, DaemonView, DashboardModel, daemon_feed::DaemonFeed, run_interactive,
};
use vibemux_probe::snapshot;

#[derive(Debug, Default)]
struct Arguments {
    once: bool,
    report_path: Option<PathBuf>,
    /// The project whose daemon the console reads; the current directory
    /// when absent.
    project_root: Option<PathBuf>,
}

/// `None` is a usage error: an unknown flag, a missing or empty value, or a
/// repeated flag.
fn parse_arguments(arguments: &[OsString]) -> Option<Arguments> {
    let mut parsed = Arguments::default();
    let mut index = 0;
    while index < arguments.len() {
        let value = || {
            arguments
                .get(index + 1)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        match arguments[index].to_str() {
            Some("--once") if !parsed.once => {
                parsed.once = true;
                index += 1;
            }
            Some("--report") if parsed.report_path.is_none() => {
                parsed.report_path = Some(value()?);
                index += 2;
            }
            Some("--project-root") if parsed.project_root.is_none() => {
                parsed.project_root = Some(value()?);
                index += 2;
            }
            _ => return None,
        }
    }
    Some(parsed)
}

#[tokio::main]
async fn main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 1 && matches!(arguments[0].to_str(), Some("--help" | "-h")) {
        println!(
            "VibeMux read-only console (the egui console is vibemux_frontend)\n\nUsage: vibemux_dashboard [--once] [--report <saved_report.json>] [--project-root <path>]\nUnified entrypoint: vibemuxctl ui\nKeys: 1-7 views | Tab next | L layout | z zoom | / filter | r refresh | t theme | ? help | q quit\nNo model invocation, daemon start, or native TUI attachment."
        );
        return;
    }
    if arguments.len() == 1 && arguments[0] == "--version" {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let Some(Arguments {
        once,
        report_path,
        project_root,
    }) = parse_arguments(&arguments)
    else {
        usage();
    };
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
    let mut context = Context {
        source: snapshot.source.to_string(),
        ..Context::default()
    };
    let root = match project_root.map_or_else(std::env::current_dir, Ok) {
        Ok(root) => root,
        Err(_) => {
            eprintln!("ui_invalid_project_root");
            std::process::exit(2);
        }
    };
    // A feed that cannot start leaves the daemon views saying why.
    let feed = match DaemonFeed::start(&root) {
        Ok(feed) => Some(feed),
        Err(error) => {
            context.daemon = Some(Arc::new(DaemonView::not_running(&error.to_string())));
            None
        }
    };
    let result = tokio::task::spawn_blocking(move || run_interactive(model, context, feed)).await;
    if !matches!(result, Ok(Ok(()))) {
        eprintln!("ui_terminal_failed");
        std::process::exit(4);
    }
}

fn usage() -> ! {
    eprintln!("ui_invalid_arguments");
    std::process::exit(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn parse(arguments: &[&str]) -> Option<Arguments> {
        parse_arguments(&arguments.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn project_root_and_report_parse_once_each() {
        let parsed = parse(&[
            "--project-root",
            "C:/project",
            "--once",
            "--report",
            "r.json",
        ])
        .expect("valid arguments");
        assert_eq!(parsed.project_root, Some(PathBuf::from("C:/project")));
        assert_eq!(parsed.report_path, Some(PathBuf::from("r.json")));
        assert!(parsed.once);
        assert_eq!(parse(&[]).map(|parsed| parsed.project_root), Some(None));
    }

    #[test]
    fn missing_or_repeated_values_are_usage_errors() {
        assert!(parse(&["--project-root"]).is_none());
        assert!(parse(&["--project-root", ""]).is_none());
        assert!(parse(&["--project-root", "a", "--project-root", "b"]).is_none());
        assert!(parse(&["--once", "--once"]).is_none());
        assert!(parse(&["--layout"]).is_none());
    }
}
