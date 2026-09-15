#![forbid(unsafe_code)]
//! On-disk persistence for the frontend user settings.
//!
//! Reads/writes a small JSON document keyed by platform:
//! - Windows: `%APPDATA%\vibemux\frontend.json`
//! - Off-Windows: `$XDG_CONFIG_HOME/vibemux/frontend.json` (falling
//!   back to `$HOME/.config/vibemux/frontend.json`)
//!
//! All reads are tolerant: missing or corrupt files return
//! [`UserConfig::default`]. Writes are atomic (write tmp, rename).
//! We never hold an OS lock; concurrent writers can clobber each
//! other, but the worst case is a lost write of the theme id.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{ThemeId, palette_for};

/// On-disk schema version. Bump when the JSON layout changes.
pub const SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

/// Persisted across launches. Only `theme` and `window_size` are
/// honored on write today; backend choice and telemetry opt-in are
/// deliberately not persisted in this slice.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserConfig {
    pub schema_version: u16,
    pub theme: ThemeId,
    pub window_size: WindowSize,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            theme: ThemeId::default(),
            window_size: WindowSize {
                width: 1280,
                height: 800,
            },
        }
    }
}

/// Resolve the user config file path for the current platform. Always
/// returns a valid `PathBuf`; never touches the filesystem.
#[must_use]
pub fn config_path() -> PathBuf {
    if let Some(appdata) = env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("vibemux").join("frontend.json");
    }
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("vibemux").join("frontend.json");
    }
    if let Some(home) = env::var_os("USERPROFILE").or_else(|| env::var_os("HOME")) {
        return PathBuf::from(home)
            .join(".config")
            .join("vibemux")
            .join("frontend.json");
    }
    PathBuf::from("vibemux_frontend.json")
}

/// Load the persisted user config. On any failure (missing file,
/// unreadable, parse error, unknown schema version), return
/// [`UserConfig::default`]. Never panics.
#[must_use]
pub fn load_user_config() -> UserConfig {
    load_user_config_at(&config_path())
}

/// Load from an explicit path. Used by tests and by callers that want
/// to ignore the platform path for any reason.
#[must_use]
pub fn load_user_config_at(path: &Path) -> UserConfig {
    let Ok(bytes) = fs::read_to_string(path) else {
        return UserConfig::default();
    };
    // Strip an optional UTF-8 BOM (`﻿`) so JSON parsers that
    // do not consume it (serde_json) see a clean string.
    let stripped = bytes.strip_prefix('\u{FEFF}').unwrap_or(&bytes);
    let parsed: Result<UserConfig, _> = serde_json::from_str(stripped);
    match parsed {
        Ok(cfg) if cfg.schema_version == SCHEMA_VERSION => cfg,
        Ok(_) => UserConfig::default(),
        Err(_) => UserConfig::default(),
    }
}

/// Persist the user config to the platform path. Errors are swallowed
/// with a one-line stderr message; the in-memory state remains the
/// source of truth for the current session.
pub fn save_user_config(cfg: &UserConfig) {
    let path = config_path();
    if let Err(error) = save_user_config_at(cfg, &path) {
        eprintln!("vibemux_frontend: failed to persist config: {error}");
    }
}

/// Persist to an explicit path. Used by tests.
pub fn save_user_config_at(cfg: &UserConfig, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialised = serde_json::to_string_pretty(cfg)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serialised)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(first) => {
            // Retry once after a small delay; another process may be
            // closing the file on Windows. We do not retry forever.
            std::thread::sleep(std::time::Duration::from_millis(50));
            fs::rename(&tmp, path).map_err(|_| first)
        }
    }
}

/// Build the active `ThemePalette` for the configured theme id.
/// Convenience used by the GUI at startup.
#[must_use]
pub fn palette_for_config(cfg: &UserConfig) -> super::ThemePalette {
    palette_for(cfg.theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn temp_path(name: &str) -> PathBuf {
        let mut dir = env::temp_dir();
        let unique = format!("vibemux-frontend-{}-{}", name, std::process::id(),);
        dir.push(unique);
        dir
    }

    #[test]
    fn user_config_round_trip() {
        let cfg = UserConfig::default();
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: UserConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, back);
    }

    #[test]
    fn user_config_default_has_claude_and_1280x800() {
        let cfg = UserConfig::default();
        assert_eq!(cfg.theme, ThemeId::Claude);
        assert_eq!(cfg.window_size.width, 1280);
        assert_eq!(cfg.window_size.height, 800);
        assert_eq!(cfg.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn missing_user_config_returns_defaults() {
        let path = temp_path("missing");
        let cfg = load_user_config_at(&path);
        assert_eq!(cfg, UserConfig::default());
    }

    #[test]
    fn corrupt_user_config_returns_defaults() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        fs::write(&path, "not json").expect("write");
        let cfg = load_user_config_at(&path);
        assert_eq!(cfg, UserConfig::default());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn bom_prefixed_user_config_is_accepted() {
        let path = temp_path("bom");
        fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        // PowerShell `Set-Content -Encoding UTF8` prepends a UTF-8 BOM.
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice(
            br#"{
  "schema_version": 1,
  "theme": "vscode",
  "window_size": { "width": 1280, "height": 800 }
}"#,
        );
        fs::write(&path, &raw).expect("write");
        let cfg = load_user_config_at(&path);
        assert_eq!(cfg.theme, ThemeId::Vscode);
        assert_eq!(cfg.window_size.width, 1280);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn unknown_schema_version_returns_defaults() {
        let path = temp_path("schema-mismatch");
        fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        let bogus = serde_json::json!({
            "schema_version": 9999_u16,
            "theme": "github",
            "window_size": { "width": 1024_u32, "height": 768_u32 }
        });
        fs::write(&path, serde_json::to_string(&bogus).unwrap()).expect("write");
        let cfg = load_user_config_at(&path);
        assert_eq!(cfg, UserConfig::default());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn save_and_load_round_trip() {
        let path = temp_path("round-trip");
        let cfg = UserConfig {
            schema_version: SCHEMA_VERSION,
            theme: ThemeId::Github,
            window_size: WindowSize {
                width: 1024,
                height: 768,
            },
        };
        save_user_config_at(&cfg, &path).expect("save");
        let back = load_user_config_at(&path);
        assert_eq!(back, cfg);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn config_path_is_absolute_or_cwd() {
        let p = config_path();
        assert!(p.is_absolute() || p.starts_with("vibemux_frontend.json"));
    }

    #[test]
    fn palette_for_config_matches_id() {
        let cfg = UserConfig {
            schema_version: SCHEMA_VERSION,
            theme: ThemeId::Vscode,
            window_size: WindowSize {
                width: 1280,
                height: 800,
            },
        };
        assert_eq!(palette_for_config(&cfg).name, "vscode");
    }
}
