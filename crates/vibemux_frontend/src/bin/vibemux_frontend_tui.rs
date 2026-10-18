#![forbid(unsafe_code)]
//! TUI debug entrypoint. Slim, read-only terminal view of the probe
//! report and the active theme.
//!
//! Flags (ported from the single-shell frontend):
//! - `--json`              print the versioned probe report as JSON and exit
//! - `--theme <name>`      start with any theme from `Theme::ALL`
//!   (`VIBEMUX_FRONTEND_THEME` applies when no flag is given); unknown
//!   themes or unknown arguments exit with code 4.

use std::{
    ffi::{OsStr, OsString},
    process::ExitCode,
};

use vibemux_frontend::{ViewModel, tui};

/// The `--theme` usage hint, derived from `Theme::ALL` so the error
/// text can never drift from the real theme registry.
fn expected_themes() -> String {
    tui::Theme::ALL
        .iter()
        .map(|theme| theme.name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse the flags after the program name into `(json, theme)`. The
/// arguments stay `OsString`s, so one that is not valid Unicode is a usage
/// error rather than a panic.
fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
    default_theme: tui::Theme,
) -> Result<(bool, tui::Theme), String> {
    let mut theme = default_theme;
    let mut json = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--json") => json = true,
            Some("--theme") => {
                theme = arguments
                    .next()
                    .as_deref()
                    .and_then(OsStr::to_str)
                    .and_then(tui::Theme::from_name)
                    .ok_or_else(|| {
                        format!(
                            "unknown or missing --theme value (expected one of: {})",
                            expected_themes()
                        )
                    })?;
            }
            _ => {
                return Err(format!("unknown argument: {}", argument.to_string_lossy()));
            }
        }
    }
    Ok((json, theme))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let (json, theme) =
        match parse_arguments(std::env::args_os().skip(1), tui::Theme::from_environment()) {
            Ok(parsed) => parsed,
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(4);
            }
        };

    let config = match vibemux_frontend::probe_env::probe_config_from_environment() {
        Ok(config) => config,
        Err(code) => {
            eprintln!("{code}");
            return ExitCode::from(4);
        }
    };
    let report = vibemux_probe::run_probe(&config).await;
    if json {
        // Machine-readable evidence for scripts and CI; theme selection
        // never affects this output.
        match serde_json::to_string_pretty(&report) {
            Ok(text) => {
                println!("{text}");
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("frontend_tui failed to serialize report: {error}");
                return ExitCode::from(4);
            }
        }
    }

    let view_model = ViewModel::from_report(&report);
    if let Err(error) = tui::run_tui(&view_model, theme) {
        eprintln!("frontend_tui failed: {error}");
        return ExitCode::from(4);
    }
    ExitCode::SUCCESS
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

    fn parse(arguments: &[OsString]) -> Result<(bool, tui::Theme), String> {
        parse_arguments(arguments.to_vec(), tui::Theme::Classic)
    }

    #[test]
    fn flags_parse_over_the_default_theme() {
        assert_eq!(parse(&[]), Ok((false, tui::Theme::Classic)));
        assert_eq!(
            parse(&["--theme".into(), "gruvbox".into(), "--json".into()]),
            Ok((true, tui::Theme::Gruvbox))
        );
    }

    #[test]
    fn a_non_unicode_flag_or_theme_is_a_usage_error() {
        let flag = parse(&[non_unicode()]).expect_err("flag refused");
        assert!(flag.starts_with("unknown argument: a"), "{flag}");
        for theme in [
            vec!["--theme".into(), non_unicode()],
            vec!["--theme".into()],
        ] {
            let error = parse(&theme).expect_err("theme refused");
            assert!(
                error.starts_with("unknown or missing --theme value"),
                "{error}"
            );
        }
    }
}
