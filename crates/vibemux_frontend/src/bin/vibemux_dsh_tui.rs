#![forbid(unsafe_code)]
//! DSH terminal surface entry point.
//!
//! Flags:
//! - `--project-root <path>`  project whose `.vibemux/dsh_session.json` is
//!   loaded (default: the current directory)
//! - `--theme <name>`         start with any theme from `Theme::ALL`
//!   (`VIBEMUX_FRONTEND_THEME` applies when no flag is given)
//! - `--config <path>`        override the session config path
//! - `--print-config`         print the content-free config summary and exit
//! - `--json`                 with `--print-config`, print JSON instead
//!
//! The surface is a separate binary because the existing diagnostic TUI is
//! deliberately read-only, while this one owns a live runtime session. It
//! never opens the core database and writes no canonical state. The daemon
//! exposes the same shared driver through its separate DSH control operations.

use std::{path::PathBuf, process::ExitCode};

use vibemux_dsh::{DSH_SESSION_CONFIG_FILE_NAME, DshSessionConfigSummary, load_session_config};
use vibemux_frontend::dsh_tui;
use vibemux_frontend::tui::Theme;

/// Exit code for a usage error, matching the diagnostic TUI.
const USAGE_EXIT: u8 = 4;

struct Arguments {
    project_root: PathBuf,
    config_path: Option<PathBuf>,
    theme: Theme,
    print_config: bool,
    json: bool,
}

fn expected_themes() -> String {
    Theme::ALL
        .iter()
        .map(|theme| theme.name())
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut project_root =
        std::env::current_dir().map_err(|_| "no working directory".to_string())?;
    let mut config_path = None;
    let mut theme = Theme::from_environment();
    let mut print_config = false;
    let mut json = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--project-root" => {
                let value = arguments.next().ok_or("--project-root needs a path")?;
                project_root = PathBuf::from(value);
            }
            "--config" => {
                let value = arguments.next().ok_or("--config needs a path")?;
                config_path = Some(PathBuf::from(value));
            }
            "--theme" => {
                let value = arguments.next().ok_or_else(|| {
                    format!(
                        "unknown or missing --theme value (expected one of: {})",
                        expected_themes()
                    )
                })?;
                theme = Theme::from_name(&value).ok_or_else(|| {
                    format!(
                        "unknown or missing --theme value (expected one of: {})",
                        expected_themes()
                    )
                })?;
            }
            "--print-config" => print_config = true,
            "--json" => json = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Arguments {
        project_root,
        config_path,
        theme,
        print_config,
        json,
    })
}

fn main() -> ExitCode {
    let arguments = match parse_arguments() {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(USAGE_EXIT);
        }
    };

    let config_path = arguments.config_path.unwrap_or_else(|| {
        arguments
            .project_root
            .join(".vibemux")
            .join(DSH_SESSION_CONFIG_FILE_NAME)
    });
    // The project root is the containment boundary for the trusted launcher
    // pair: a repository must not be able to ship the code a session runs.
    let canonical_root = std::fs::canonicalize(&arguments.project_root)
        .unwrap_or_else(|_| arguments.project_root.clone());
    let config = match load_session_config(&config_path, &canonical_root) {
        Ok(Some(config)) => config,
        Ok(None) => {
            eprintln!(
                "vibemux dsh: no session configured at {}\n\
                 Create it, then retry. See docs/dsh_session_protocol.md for the schema.",
                config_path.display()
            );
            return ExitCode::from(USAGE_EXIT);
        }
        Err(error) => {
            eprintln!(
                "vibemux dsh: configuration rejected ({})\n\
                 The launcher pair and every path are verified before a runtime starts; \
                 see docs/dsh_session_protocol.md.",
                error.code()
            );
            return ExitCode::from(USAGE_EXIT);
        }
    };

    if arguments.print_config {
        let summary = DshSessionConfigSummary::from(&config);
        if arguments.json {
            match serde_json::to_string_pretty(&summary) {
                Ok(text) => {
                    println!("{text}");
                    return ExitCode::SUCCESS;
                }
                Err(_) => return ExitCode::from(USAGE_EXIT),
            }
        }
        println!("profile           {}", summary.profile);
        println!("provider          {}", summary.provider);
        println!("model             {}", summary.model);
        println!(
            "reasoning effort  {}",
            if summary.has_reasoning_effort {
                "set"
            } else {
                "unset"
            }
        );
        println!(
            "max tokens        {}",
            summary
                .max_tokens
                .map_or_else(|| "adapter default".to_string(), |value| value.to_string())
        );
        println!("environment       {} names", summary.environment_name_count);
        println!("frame bytes       {}", summary.limits.frame_bytes);
        println!("record bytes      {}", summary.limits.record_bytes);
        println!("record count      {}", summary.limits.record_count);
        println!(
            "initialize ms     {}",
            summary.limits.initialize_deadline_ms
        );
        println!("turn ms           {}", summary.limits.turn_deadline_ms);
        println!("shutdown ms       {}", summary.limits.shutdown_grace_ms);
        return ExitCode::SUCCESS;
    }

    let launch = match config.launch(&canonical_root) {
        Ok(launch) => launch,
        Err(error) => {
            eprintln!("vibemux dsh: {}", error.code());
            return ExitCode::from(USAGE_EXIT);
        }
    };
    let settings = match config.settings(session_id(), &canonical_root) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("vibemux dsh: {}", error.code());
            return ExitCode::from(USAGE_EXIT);
        }
    };

    // A multi-threaded runtime keeps the reader task alive while the main
    // thread blocks in the terminal event loop; the main thread is not a
    // worker, so `Handle::block_on` is allowed here.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("vibemux dsh: runtime could not start");
            return ExitCode::from(USAGE_EXIT);
        }
    };
    let handle = runtime.handle().clone();
    let label = format!(
        "profile {} provider {} model {}",
        config.profile, config.provider, config.model
    );
    let settings_for_label = settings.clone();
    match dsh_tui::open_and_run(&handle, &launch, settings, &label, arguments.theme) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "vibemux dsh: {} (provider {} model {})",
                error.code(),
                settings_for_label.provider,
                settings_for_label.model
            );
            ExitCode::from(usage_or_failure(error))
        }
    }
}

/// A spawn or handshake failure is an operational failure (1); everything
/// else here is a usage-shaped refusal (4).
fn usage_or_failure(error: vibemux_dsh::DshDriverError) -> u8 {
    if error.is_session_failure() {
        1
    } else {
        USAGE_EXIT
    }
}

/// The SDK session id. The protocol lets the client choose it, and an unknown
/// id lazily creates the agent+session pair, so a fresh id per run is the
/// correct default for an operator surface.
fn session_id() -> String {
    let epochs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("vibemux_tui_{}_{}", std::process::id(), epochs)
}
