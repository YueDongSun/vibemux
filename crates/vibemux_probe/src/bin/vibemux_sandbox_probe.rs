//! Explicit fixed offline sandbox experiment; no daemon or model admission.
#![forbid(unsafe_code)]

use std::{ffi::OsString, path::PathBuf, process::ExitCode};
use vibemux_probe::sandbox_probe::{ProbeOutcome, fixture_entry, run_offline_probe};

const USAGE: &str = "usage: vibemux_sandbox_probe --backend <absolute codex.exe> --scratch-parent <owned directory>";

#[tokio::main]
async fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|value| value == "__launch_trampoline")
    {
        #[cfg(any(windows, unix))]
        return ExitCode::from(
            u8::try_from(vibemux_platform::run_launch_trampoline(
                arguments.into_iter().skip(1),
            ))
            .unwrap_or(123),
        );
    }
    if let Some(code) = fixture_entry(&arguments) {
        return ExitCode::from(u8::try_from(code).unwrap_or(4));
    }
    if arguments == [OsString::from("--help")] {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let Some((backend, scratch_parent)) = parse(&arguments) else {
        eprintln!("{USAGE}");
        return ExitCode::from(4);
    };
    let report = run_offline_probe(&backend, &scratch_parent).await;
    let exit = match report.outcome {
        ProbeOutcome::Passed => 0,
        ProbeOutcome::Failed => 2,
        ProbeOutcome::Unavailable => 3,
    };
    match serde_json::to_string_pretty(&report) {
        Ok(encoded) => println!("{encoded}"),
        Err(_) => {
            eprintln!("sandbox_report_encoding_failed");
            return ExitCode::from(2);
        }
    }
    ExitCode::from(exit)
}

fn parse(arguments: &[OsString]) -> Option<(PathBuf, PathBuf)> {
    let mut backend = None;
    let mut scratch_parent = None;
    let mut entries = arguments.iter();
    while let Some(key) = entries.next() {
        let value = PathBuf::from(entries.next()?);
        if !value.is_absolute() {
            return None;
        }
        match key.to_str()? {
            "--backend" if backend.is_none() => backend = Some(value),
            "--scratch-parent" if scratch_parent.is_none() => scratch_parent = Some(value),
            _ => return None,
        }
    }
    Some((backend?, scratch_parent?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caller_cannot_supply_commands_network_or_duplicate_options() {
        for args in [
            vec!["--network", "online"],
            vec!["--backend", "codex"],
            vec!["--backend"],
        ] {
            assert!(parse(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).is_none());
        }
        let root = std::env::current_dir().unwrap();
        let args = vec![
            "--backend".into(),
            root.as_os_str().into(),
            "--scratch-parent".into(),
            root.as_os_str().into(),
        ];
        assert!(parse(&args).is_some());
        let mut duplicate = args.clone();
        duplicate.extend(["--backend".into(), root.as_os_str().into()]);
        assert!(parse(&duplicate).is_none());
    }
}
