//! Integration tests for the frontend binaries after the two-shell split:
//! flag handling lives on `vibemux_frontend_tui`; the plain snapshot is the
//! `vibemux_frontend_dump` binary (which superseded the removed `--once`
//! flag). The egui GUI binary takes no flags.

#[path = "support/cli_fixture.rs"]
mod cli_fixture;
use cli_fixture::CliFixture;

fn run_tui(arguments: &[&str]) -> (i32, String, String) {
    CliFixture::new().run(env!("CARGO_BIN_EXE_vibemux_frontend_tui"), arguments, &[])
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
    // --theme with --json: JSON output stays theme-independent. The two
    // structural themes and the two 256-color brand themes are covered;
    // classic is the no-flag default exercised everywhere else.
    for theme in ["mono", "light", "nord", "gruvbox"] {
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
    let (code, stdout, _) = CliFixture::new().run(
        env!("CARGO_BIN_EXE_vibemux_frontend_tui"),
        &["--json"],
        &[("VIBEMUX_FRONTEND_THEME", "neon-does-not-exist")],
    );
    assert_eq!(code, 0, "invalid env theme must not fail");
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
    // The usage hint is derived from Theme::ALL; pin that every theme
    // name is advertised so the error text cannot drift from the
    // registry.
    for theme in [
        "classic",
        "high-contrast",
        "mono",
        "light",
        "nord",
        "gruvbox",
    ] {
        assert!(
            stderr.contains(theme),
            "--theme usage hint must list {theme}"
        );
    }
}

#[test]
fn dump_emits_plain_theme_independent_snapshot() {
    // The dump binary replaces the removed --once flag: plain text, ASCII,
    // unaffected by theme selection.
    let (code, stdout, _) = CliFixture::new().run(
        env!("CARGO_BIN_EXE_vibemux_frontend_dump"),
        &[],
        &[("VIBEMUX_FRONTEND_THEME", "light")],
    );
    assert_eq!(code, 0);
    assert!(stdout.contains("VibeMux Central Console"));
    assert!(stdout.contains("agents:"));
    assert!(stdout.is_ascii(), "dump output must stay ASCII");
}

#[test]
fn invalid_probe_override_fails_before_observation() {
    let fixture = CliFixture::new();
    for binary in [
        env!("CARGO_BIN_EXE_vibemux_frontend_tui"),
        env!("CARGO_BIN_EXE_vibemux_frontend_dump"),
    ] {
        let (code, stdout, stderr) = fixture.run(
            binary,
            &["--json"],
            &[(
                vibemux_frontend::probe_env::FRONTEND_PROBE_PORT_ENV,
                "65536",
            )],
        );
        assert_eq!(code, 4);
        assert!(stdout.is_empty());
        assert_eq!(stderr.trim(), "frontend_probe_port_invalid");
    }
    assert_eq!(
        fixture.connections(),
        0,
        "invalid override must not contact the fixture"
    );
}
