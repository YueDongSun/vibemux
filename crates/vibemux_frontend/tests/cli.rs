use std::process::Command;

fn run_frontend(arguments: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_vibemux_frontend"))
        .args(arguments)
        .output()
        .expect("frontend binary runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn json_mode_emits_versioned_probe_report() {
    let (code, stdout, _stderr) = run_frontend(&["--json"]);
    assert_eq!(code, 0);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json must emit valid JSON");
    assert_eq!(value["schema_version"], 1, "report must be versioned");
    assert!(value["platform"].is_string());
    assert!(value["agents"].is_array());
}

#[test]
fn theme_combines_with_once_and_json_flags() {
    // --theme with --json: JSON output stays theme-independent.
    let (code, stdout, _stderr) = run_frontend(&["--theme", "mono", "--json"]);
    assert_eq!(code, 0);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json must stay valid JSON with --theme");
    assert_eq!(value["schema_version"], 1);

    // --theme with --once: plain snapshot stays theme-independent too.
    let (code, stdout, _stderr) = run_frontend(&["--theme", "light", "--once"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("VibeMux Unified Frontend"));
    assert!(stdout.contains("agents:"));

    // Environment-selected theme must not break either mode.
    let output = Command::new(env!("CARGO_BIN_EXE_vibemux_frontend"))
        .args(["--once"])
        .env("VIBEMUX_FRONTEND_THEME", "light")
        .output()
        .expect("frontend binary runs");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("VibeMux Unified Frontend"));
}

#[test]
fn unknown_arguments_exit_with_code_four() {
    let (code, stdout, stderr) = run_frontend(&["--bogus"]);
    assert_eq!(code, 4);
    assert!(stdout.is_empty());
    assert!(stderr.contains("unknown argument"));
    let (code, _stdout, stderr) = run_frontend(&["--theme", "neon"]);
    assert_eq!(code, 4);
    assert!(stderr.contains("unknown or missing --theme value"));
}
