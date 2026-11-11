//! The webview lockdown.
//!
//! The config, the capability file, and the command list must agree, and
//! nothing may reach the webview that the review did not grant: no
//! plugin, no remote URL, no global Tauri object, and no navigation away
//! from the app origin.

use std::collections::BTreeSet;

use vibemux_desktop::command_names::COMMAND_NAMES;

const REVIEWED_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";
const DEV_URL: &str = "http://localhost:5180";

fn crate_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(relative: &str) -> serde_json::Value {
    let path = crate_dir().join(relative);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{relative} is unreadable: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{relative} is not JSON: {error}"))
}

fn collect_strings(value: &serde_json::Value, found: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => found.push(text.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_strings(item, found);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                collect_strings(item, found);
            }
        }
        _ => {}
    }
}

#[test]
fn csp_is_exactly_the_reviewed_string() {
    let config = read_json("tauri.conf.json");
    assert_eq!(
        config["app"]["security"]["csp"].as_str(),
        Some(REVIEWED_CSP),
        "the content security policy changed"
    );
}

#[test]
fn no_remote_urls_in_config() {
    let config = read_json("tauri.conf.json");
    let mut strings = Vec::new();
    collect_strings(&config, &mut strings);
    let remote: BTreeSet<String> = strings
        .iter()
        .filter(|text| text.starts_with("http://") || text.starts_with("https://"))
        .cloned()
        .collect();
    assert_eq!(
        remote,
        BTreeSet::from([DEV_URL.to_owned()]),
        "the config names a remote URL that the app would load"
    );
}

#[test]
fn global_tauri_is_off_and_the_prototype_is_frozen() {
    let config = read_json("tauri.conf.json");
    assert_eq!(config["app"]["withGlobalTauri"].as_bool(), Some(false));
    assert_eq!(
        config["app"]["security"]["freezePrototype"].as_bool(),
        Some(true)
    );
}

#[test]
fn the_windows_are_created_in_rust() {
    let config = read_json("tauri.conf.json");
    let windows = config["app"]["windows"]
        .as_array()
        .expect("app.windows is not an array");
    assert!(
        windows.is_empty(),
        "the config declares a window, so it would miss the navigation guard"
    );
}

fn permissions(capability: &serde_json::Value) -> BTreeSet<String> {
    capability["permissions"]
        .as_array()
        .expect("permissions is not an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("permission is not a string")
                .to_owned()
        })
        .collect()
}

fn provider_permissions() -> BTreeSet<String> {
    BTreeSet::from([
        "allow-provider-profiles-get".to_owned(),
        "allow-provider-profiles-set".to_owned(),
    ])
}

#[test]
fn the_combined_capabilities_grant_exactly_the_command_list() {
    let shared = read_json("capabilities/main_window.json");
    let provider = read_json("capabilities/provider_profiles.json");
    let mut granted = permissions(&shared);
    assert!(granted.is_disjoint(&provider_permissions()));
    granted.extend(permissions(&provider));
    let mut expected: BTreeSet<String> = BTreeSet::from([
        "core:event:allow-listen".to_owned(),
        "core:event:allow-unlisten".to_owned(),
    ]);
    for name in COMMAND_NAMES {
        expected.insert(format!("allow-{}", name.replace('_', "-")));
    }
    assert_eq!(granted, expected);
    assert_eq!(COMMAND_NAMES.len(), 25);
}

#[test]
fn provider_configuration_is_granted_only_to_the_main_window() {
    let capability = read_json("capabilities/provider_profiles.json");
    assert_eq!(capability["windows"], serde_json::json!(["main"]));
    assert_eq!(permissions(&capability), provider_permissions());
    assert!(capability.get("remote").is_none());
    assert_ne!(
        capability.get("local"),
        Some(&serde_json::Value::Bool(false))
    );
}

#[test]
fn no_unreviewed_capability_file_can_add_a_grant() {
    let files: BTreeSet<_> = std::fs::read_dir(crate_dir().join("capabilities"))
        .expect("capability directory")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(
        files,
        BTreeSet::from([
            std::ffi::OsString::from("main_window.json"),
            std::ffi::OsString::from("provider_profiles.json"),
        ])
    );
}

#[test]
fn the_typescript_command_names_match_the_native_manifest() {
    let source = std::fs::read_to_string(crate_dir().join("ui/src/lib/ui_contract.ts"))
        .expect("TypeScript contract");
    let list = source
        .split_once("export const COMMAND_NAMES = [")
        .expect("command declaration")
        .1
        .split_once("] as const")
        .expect("command array")
        .0;
    let names: BTreeSet<_> = list
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix('\'')
                .and_then(|line| line.strip_suffix("',"))
        })
        .collect();
    assert_eq!(names, BTreeSet::from(COMMAND_NAMES));
}

#[test]
fn the_capability_covers_the_main_and_task_windows() {
    let capability = read_json("capabilities/main_window.json");
    let windows: Vec<&str> = capability["windows"]
        .as_array()
        .expect("windows is not an array")
        .iter()
        .map(|value| value.as_str().expect("window is not a string"))
        .collect();
    assert!(windows.contains(&"main"));
    assert!(windows.contains(&"task_*"));
}

#[test]
fn no_plugins_are_configured() {
    let config = read_json("tauri.conf.json");
    assert!(
        config.get("plugins").is_none(),
        "a plugin is configured in tauri.conf.json"
    );
    assert!(
        config["app"].get("plugins").is_none(),
        "a plugin is configured in tauri.conf.json"
    );

    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml"))
        .unwrap_or_else(|error| panic!("Cargo.toml is unreadable: {error}"));
    assert!(
        !manifest.contains("tauri-plugin-"),
        "the crate depends on a Tauri plugin"
    );
}

#[test]
fn the_bundle_is_not_active() {
    let config = read_json("tauri.conf.json");
    assert_eq!(config["bundle"]["active"].as_bool(), Some(false));
}

#[test]
fn the_crate_has_no_gui_dependency() {
    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml"))
        .unwrap_or_else(|error| panic!("Cargo.toml is unreadable: {error}"));
    for forbidden in ["egui", "eframe", "ratatui", "crossterm", "winit"] {
        assert!(
            !manifest.contains(forbidden),
            "the desktop crate depends on {forbidden}"
        );
    }
}
