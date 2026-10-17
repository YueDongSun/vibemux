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

use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
    process::ExitCode,
};

use vibemux_dsh::{DSH_SESSION_CONFIG_FILE_NAME, DshSessionConfigSummary, load_session_config};
use vibemux_frontend::dsh_tui;
use vibemux_frontend::tui::Theme;

/// Exit code for a usage error, matching the diagnostic TUI.
const USAGE_EXIT: u8 = 4;

#[derive(Debug)]
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

/// Parse the flags after the program name. The arguments stay `OsString`s,
/// so a path flag keeps a path that is not valid Unicode, and any other
/// argument that is not valid Unicode is a usage error rather than a panic.
fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
    current_dir: PathBuf,
    default_theme: Theme,
) -> Result<Arguments, String> {
    let mut project_root = current_dir;
    let mut config_path = None;
    let mut theme = default_theme;
    let mut print_config = false;
    let mut json = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--project-root") => {
                let value = arguments.next().ok_or("--project-root needs a path")?;
                project_root = PathBuf::from(value);
            }
            Some("--config") => {
                let value = arguments.next().ok_or("--config needs a path")?;
                config_path = Some(PathBuf::from(value));
            }
            Some("--theme") => {
                theme = arguments
                    .next()
                    .as_deref()
                    .and_then(OsStr::to_str)
                    .and_then(Theme::from_name)
                    .ok_or_else(|| {
                        format!(
                            "unknown or missing --theme value (expected one of: {})",
                            expected_themes()
                        )
                    })?;
            }
            Some("--print-config") => print_config = true,
            Some("--json") => json = true,
            _ => {
                return Err(format!("unknown argument: {}", argument.to_string_lossy()));
            }
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
    let Ok(current_dir) = std::env::current_dir() else {
        eprintln!("no working directory");
        return ExitCode::from(USAGE_EXIT);
    };
    let arguments = match parse_arguments(
        std::env::args_os().skip(1),
        current_dir,
        Theme::from_environment(),
    ) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn non_unicode() -> OsString {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[0x61, 0xD800])
    }

    #[cfg(unix)]
    fn non_unicode() -> OsString {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![b'a', 0xff])
    }

    fn parse(arguments: Vec<OsString>) -> Result<Arguments, String> {
        parse_arguments(arguments, PathBuf::from("cwd"), Theme::Classic)
    }

    fn strings(arguments: &[&str]) -> Vec<OsString> {
        arguments.iter().map(OsString::from).collect()
    }

    #[test]
    fn flags_parse_and_defaults_come_from_the_caller() {
        let defaults = parse(Vec::new()).expect("no flags");
        assert_eq!(defaults.project_root, PathBuf::from("cwd"));
        assert_eq!(defaults.config_path, None);
        assert_eq!(defaults.theme, Theme::Classic);
        assert!(!defaults.print_config && !defaults.json);

        let parsed = parse(strings(&[
            "--project-root",
            "root",
            "--config",
            "session.json",
            "--theme",
            "nord",
            "--print-config",
            "--json",
        ]))
        .expect("all flags");
        assert_eq!(parsed.project_root, PathBuf::from("root"));
        assert_eq!(parsed.config_path, Some(PathBuf::from("session.json")));
        assert_eq!(parsed.theme, Theme::Nord);
        assert!(parsed.print_config && parsed.json);
    }

    #[test]
    fn a_path_flag_keeps_a_non_unicode_path() {
        let parsed = parse([strings(&["--project-root"]), vec![non_unicode()]].concat())
            .expect("project root");
        assert_eq!(parsed.project_root.as_os_str(), non_unicode());
        let parsed = parse([strings(&["--config"]), vec![non_unicode()]].concat()).expect("config");
        assert_eq!(
            parsed.config_path.as_deref().map(|path| path.as_os_str()),
            Some(non_unicode().as_os_str())
        );
    }

    #[test]
    fn a_non_unicode_flag_or_theme_is_a_usage_error() {
        let flag = parse(vec![non_unicode()]).expect_err("flag refused");
        assert!(flag.starts_with("unknown argument: a"), "{flag}");
        let theme = parse([strings(&["--theme"]), vec![non_unicode()]].concat())
            .expect_err("theme refused");
        assert!(
            theme.starts_with("unknown or missing --theme value"),
            "{theme}"
        );
    }

    #[test]
    fn a_missing_value_is_a_usage_error() {
        for (flag, message) in [
            ("--project-root", "--project-root needs a path"),
            ("--config", "--config needs a path"),
            ("--theme", "unknown or missing --theme value"),
        ] {
            let error = parse(strings(&[flag])).expect_err("refused");
            assert!(error.starts_with(message), "{flag}: {error}");
        }
    }
}
