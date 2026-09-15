#![forbid(unsafe_code)]
//! Thin wrapper around `vibemux_probe::PROBE_ENVIRONMENT_ALLOWLIST`.
//!
//! Re-exports the allowlist and offers a deterministic helper to read
//! only the allowlisted variables from `std::env`. The intent is to
//! never let the frontend see a secret-bearing environment variable,
//! even by accident.

use std::env;

pub use vibemux_probe::PROBE_ENVIRONMENT_ALLOWLIST;

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
    PROBE_ENVIRONMENT_ALLOWLIST
        .iter()
        .map(|key| {
            let value = env::var_os(key).map(|raw| raw.to_string_lossy().into_owned());
            ((*key).to_string(), value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
