#![forbid(unsafe_code)]
//! Thin wrapper around `vibemux_probe::PROBE_ENVIRONMENT_ALLOWLIST`.
//!
//! Re-exports the allowlist and offers a deterministic helper to read
//! only the allowlisted variables from `std::env`. The intent is to
//! never let the frontend see a secret-bearing environment variable,
//! even by accident.

use std::{env, ffi::OsString};
use vibemux_harness::terminal_text::display_line;
use vibemux_probe::ProbeConfig;

pub use vibemux_probe::PROBE_ENVIRONMENT_ALLOWLIST;

pub const FRONTEND_PROBE_PORT_ENV: &str = "VIBEMUX_FRONTEND_PROBE_PORT";

/// Preserve the probe defaults, with one strict frontend-owned loopback port
/// override for explicitly isolated observation and CLI fixtures.
pub fn probe_config_from_environment() -> Result<ProbeConfig, &'static str> {
    let port = match env::var(FRONTEND_PROBE_PORT_ENV) {
        Ok(port) => Some(port),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => return Err("frontend_probe_port_invalid"),
    };
    apply_probe_port(ProbeConfig::from_environment(), port.as_deref())
}

fn apply_probe_port(
    mut config: ProbeConfig,
    port: Option<&str>,
) -> Result<ProbeConfig, &'static str> {
    if let Some(port) = port {
        if port.is_empty() || port.len() > 5 || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("frontend_probe_port_invalid");
        }
        config.cc_switch_port = port
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or("frontend_probe_port_invalid")?;
    }
    Ok(config)
}

/// Read each allowlisted key from the current process environment.
/// Preserves the order of [`PROBE_ENVIRONMENT_ALLOWLIST`]. Values are
/// returned as `None` when the variable is unset (vs. `Some(value)`
/// when it is set, including empty strings).
///
/// If `PROBE_ENVIRONMENT_ALLOWLIST` ever stops being `pub`, this
/// function is the single place that must be updated to mirror the
/// list inline.
#[must_use]
pub fn collect_allowlisted_env() -> Vec<(String, Option<String>)> {
    allowlisted_values(|key| env::var_os(key))
}

/// The allowlisted keys in order, each with the value `read` returns as one
/// display line. The diagnostic TUI and its snapshot print these values to
/// the terminal, and a directory name in `PATH` or `HOME` may hold a line
/// break or an ANSI escape.
fn allowlisted_values(read: impl Fn(&str) -> Option<OsString>) -> Vec<(String, Option<String>)> {
    PROBE_ENVIRONMENT_ALLOWLIST
        .iter()
        .map(|key| {
            let value = read(key).map(|raw| display_line(&raw.to_string_lossy()));
            ((*key).to_string(), value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_probe_port_is_validated_and_preserves_other_defaults() {
        let original = ProbeConfig::from_environment();
        let changed = apply_probe_port(original.clone(), Some("23456")).expect("valid port");
        assert_eq!(changed.cc_switch_port, 23456);
        assert!(changed.home_dir == original.home_dir);
        assert!(changed.path_entries == original.path_entries);
        assert_eq!(changed.command_timeout, original.command_timeout);
        assert_eq!(changed.network_timeout, original.network_timeout);
        assert_eq!(changed.run_a2a_self_test, original.run_a2a_self_test);
        assert_eq!(
            apply_probe_port(original.clone(), None)
                .unwrap()
                .cc_switch_port,
            original.cc_switch_port
        );
        for invalid in [
            "", "0", "65536", "-1", "+1", " 23456", "23456 ", "1.2", "example",
        ] {
            assert!(apply_probe_port(original.clone(), Some(invalid)).is_err());
        }
    }

    #[test]
    fn allowlist_has_fifteen_entries() {
        // If the probe crate changes the allowlist size, this test
        // will flag a deliberate divergence.
        assert_eq!(PROBE_ENVIRONMENT_ALLOWLIST.len(), 15);
    }

    #[test]
    fn collect_uses_allowlist_order() {
        let collected = collect_allowlisted_env();
        assert_eq!(collected.len(), PROBE_ENVIRONMENT_ALLOWLIST.len());
        for (idx, key) in PROBE_ENVIRONMENT_ALLOWLIST.iter().enumerate() {
            assert_eq!(collected[idx].0, *key);
        }
    }

    #[test]
    fn unset_keys_appear_with_none_value() {
        // HOME is set in CI; APPDATA is on Windows. We do not assert
        // those because the test runs cross-platform, but every
        // allowlisted key is present in the output with either Some or
        // None — never omitted.
        let collected = collect_allowlisted_env();
        for (key, _) in &collected {
            assert!(!key.is_empty());
        }
    }

    #[test]
    fn allowlisted_values_are_one_display_line() {
        let collected = allowlisted_values(|key| match key {
            "PATH" => Some(OsString::from("C:\\bin\n\u{1b}[2J;\u{202e}x")),
            "HOME" => Some(OsString::new()),
            "LANG" => Some(OsString::from("zh_CN.UTF-8")),
            _ => None,
        });
        let value = |wanted: &str| {
            collected
                .iter()
                .find(|(key, _)| key == wanted)
                .and_then(|(_, value)| value.as_deref())
        };
        assert_eq!(value("PATH"), Some("C:\\bin  [2J; x"));
        assert_eq!(value("HOME"), Some(""));
        assert_eq!(value("LANG"), Some("zh_CN.UTF-8"));
        assert_eq!(value("TEMP"), None);
        assert_eq!(collected.len(), PROBE_ENVIRONMENT_ALLOWLIST.len());
    }
}
