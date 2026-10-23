//! Enforces spec §5.2: `vibemux_client` is GUI-neutral and must never gain a
//! GUI or terminal-rendering dependency. Plain line parsing of the crate's
//! own `Cargo.toml` is enough to catch this; no new dependency is needed.

/// Dependency keys that would pull a GUI or terminal rendering stack into
/// this crate.
const FORBIDDEN: [&str; 5] = ["egui", "eframe", "ratatui", "crossterm", "winit"];

#[test]
fn manifest_has_no_gui_or_terminal_dependency() {
    let manifest = include_str!("../Cargo.toml");
    let mut in_dependency_table = false;
    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let header = line.trim_start_matches('[').trim_end_matches(']');
            in_dependency_table = header.ends_with("dependencies");
            continue;
        }
        if !in_dependency_table {
            continue;
        }
        let Some((key_part, _value)) = line.split_once('=') else {
            continue;
        };
        let key = key_part.trim().split('.').next().unwrap_or("").trim();
        assert!(
            !FORBIDDEN.contains(&key),
            "vibemux_client/Cargo.toml declares a forbidden GUI/terminal dependency: {key}"
        );
    }
}
