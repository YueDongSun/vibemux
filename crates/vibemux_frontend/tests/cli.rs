//! Integration tests for the frontend binaries after the two-shell split:
//! flag handling lives on `vibemux_frontend_tui`; the plain snapshot is the
//! `vibemux_frontend_dump` binary (which superseded the removed `--once`
//! flag). The egui GUI binary takes no flags.

use std::process::Command;

fn run_tui(arguments: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_vibemux_frontend_tui"))
        .args(arguments)
        .output()
        .expect("tui binary runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn json_mode_emits_versioned_probe_report() {
    let (code, stdout, _stderr) = run_tui(&["--json"]);
    assert_eq!(code, 0);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json must emit valid JSON");
    assert_eq!(value["schema_version"], 1, "report must be versioned");
    assert!(value["platform"].is_string());
    assert!(value["agents"].is_array());
}

#[test]
fn theme_flag_combines_with_json() {
    // --theme with --json: JSON output stays theme-independent.
    for theme in ["mono", "light"] {
        let (code, stdout, _stderr) = run_tui(&["--theme", theme, "--json"]);
        assert_eq!(code, 0, "--theme {theme} --json must succeed");
        let value: serde_json::Value =
            serde_json::from_str(&stdout).expect("--json must stay valid JSON with --theme");
        assert_eq!(value["schema_version"], 1);
    }
}

#[test]
fn invalid_theme_environment_falls_back_to_classic() {
    // An unparsable VIBEMUX_FRONTEND_THEME must degrade to the default
    // theme instead of failing the run; only the CLI flag errors hard.
    let output = Command::new(env!("CARGO_BIN_EXE_vibemux_frontend_tui"))
        .args(["--json"])
        .env("VIBEMUX_FRONTEND_THEME", "neon-does-not-exist")
        .output()
        .expect("tui binary runs");
    assert!(output.status.success(), "invalid env theme must not fail");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("invalid env theme still emits JSON");
    assert_eq!(value["schema_version"], 1);
}

#[test]
fn unknown_arguments_exit_with_code_four() {
    let (code, stdout, stderr) = run_tui(&["--bogus"]);
    assert_eq!(code, 4);
    assert!(stdout.is_empty());
    assert!(stderr.contains("unknown argument"));
    let (code, _stdout, stderr) = run_tui(&["--theme", "neon"]);
    assert_eq!(code, 4);
    assert!(stderr.contains("unknown or missing --theme value"));
}

#[test]
fn dump_emits_plain_theme_independent_snapshot() {
    // The dump binary replaces the removed --once flag: plain text, ASCII,
    // unaffected by theme selection.
    let output = Command::new(env!("CARGO_BIN_EXE_vibemux_frontend_dump"))
        .env("VIBEMUX_FRONTEND_THEME", "light")
        .output()
        .expect("dump binary runs");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("VibeMux Central Console"));
    assert!(stdout.contains("agents:"));
    assert!(stdout.is_ascii(), "dump output must stay ASCII");
}
