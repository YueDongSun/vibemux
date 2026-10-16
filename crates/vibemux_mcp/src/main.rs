#![forbid(unsafe_code)]

use std::time::Duration;
use vibemux_mcp::config::{HELP, LaunchMode, parse_arguments};

fn main() {
    let mode = match parse_arguments(std::env::args_os().skip(1)) {
        Ok(mode) => mode,
        Err(code) => {
            eprintln!("{code}");
            std::process::exit(2);
        }
    };
    if matches!(mode, LaunchMode::Help) {
        print!("{HELP}");
        return;
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("runtime_unavailable");
            std::process::exit(1);
        }
    };
    let result = runtime.block_on(async {
        let reader = tokio::io::BufReader::new(tokio::io::stdin());
        let writer = tokio::io::stdout();
        match mode {
            LaunchMode::Host(config) => vibemux_mcp::session::run(reader, writer, config).await,
            LaunchMode::IpcReader => vibemux_mcp::control_reader::run(reader, writer).await,
            LaunchMode::Help => Ok(()),
        }
    });
    // Tokio's stdio uses a blocking OS read. A client that leaves stdin open
    // after a fatal frame must not keep the process alive during runtime drop.
    runtime.shutdown_timeout(Duration::from_millis(100));
    if let Err(code) = result {
        eprintln!("{code}");
        std::process::exit(1);
    }
}
