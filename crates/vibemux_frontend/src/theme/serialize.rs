#![forbid(unsafe_code)]
//! On-disk persistence for the frontend user settings.
//!
//! Reads/writes a small JSON document keyed by platform:
//! - Windows: `%APPDATA%\vibemux\frontend.json`
//! - Off-Windows: `$XDG_CONFIG_HOME/vibemux/frontend.json` (falling
//!   back to `$HOME/.config/vibemux/frontend.json`)
//!
//! All reads are tolerant: missing, corrupt, or oversized files return
//! [`UserConfig::default`]. Writes are atomic (write tmp, rename).
//! We never hold an OS lock; concurrent writers can clobber each
//! other, but the worst case is a lost write of the theme id.

use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{ThemeId, palette_for};

/// On-disk schema version. Bump when the JSON layout changes.
pub const SCHEMA_VERSION: u16 = 1;

/// A settings document is a few hundred bytes of JSON; a file past this
/// bound is not one and is never read whole.
const MAX_USER_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

impl WindowSize {
    pub const MIN_WIDTH: u32 = 800;
    pub const MAX_WIDTH: u32 = 3840;
    pub const MIN_HEIGHT: u32 = 600;
    pub const MAX_HEIGHT: u32 = 2160;

    /// This size moved into the range the settings dialog offers, which is
    /// the only range the GUI ever saves or opens a window at.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            width: self.width.clamp(Self::MIN_WIDTH, Self::MAX_WIDTH),
            height: self.height.clamp(Self::MIN_HEIGHT, Self::MAX_HEIGHT),
        }
    }
}

/// Persisted across launches. Only `theme` and `window_size` are
/// honored on write today; backend choice and telemetry opt-in are
/// deliberately not persisted in this slice.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserConfig {
    pub schema_version: u16,
    pub theme: ThemeId,
    pub window_size: WindowSize,
    /// Sidebar collapsed to its icon rail (ADR 030 §7).
    #[serde(default)]
    pub sidebar_collapsed: bool,
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
            sidebar_collapsed: false,
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
/// unreadable, over 64 KiB, parse error, unknown schema version), return
/// [`UserConfig::default`]. A window size outside the settings range is
/// clamped into it, so a hand-edited file cannot open a zero-sized or
/// oversized window. Never panics.
#[must_use]
pub fn load_user_config() -> UserConfig {
    load_user_config_at(&config_path())
}

/// Load from an explicit path. Used by tests and by callers that want
/// to ignore the platform path for any reason.
#[must_use]
pub fn load_user_config_at(path: &Path) -> UserConfig {
    let Some(bytes) = read_bounded_text(path, MAX_USER_CONFIG_BYTES) else {
        return UserConfig::default();
    };
    // Strip an optional UTF-8 BOM (`﻿`) so JSON parsers that
    // do not consume it (serde_json) see a clean string.
    let stripped = bytes.strip_prefix('\u{FEFF}').unwrap_or(&bytes);
    let parsed: Result<UserConfig, _> = serde_json::from_str(stripped);
    match parsed {
        Ok(cfg) if cfg.schema_version == SCHEMA_VERSION => UserConfig {
            window_size: cfg.window_size.clamped(),
            ..cfg
        },
        Ok(_) => UserConfig::default(),
        Err(_) => UserConfig::default(),
    }
}

/// The UTF-8 text of the regular file at `path`, at most `maximum`
/// bytes of it. The check follows a link, since a dotfiles manager may
/// place the settings file as one.
fn read_bounded_text(path: &Path, maximum: u64) -> Option<String> {
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let bytes = read_at_most(fs::File::open(path).ok()?, maximum)?;
    String::from_utf8(bytes).ok()
}

/// At most `maximum` bytes; a longer source is refused after reading one
/// byte past the bound, so a file grown after the check is not read whole.
fn read_at_most(reader: impl Read, maximum: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= maximum).then_some(bytes)
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
    fn user_config_default_has_claude_light_and_1280x800() {
        let cfg = UserConfig::default();
        assert_eq!(cfg.theme, ThemeId::ClaudeLight);
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
            ..UserConfig::default()
        };
        save_user_config_at(&cfg, &path).expect("save");
        let back = load_user_config_at(&path);
        assert_eq!(back, cfg);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn an_out_of_range_window_size_is_clamped_on_load() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("frontend.json");
        for (stored, loaded) in [
            ((0, 0), (800, 600)),
            ((100_000, 1), (3840, 600)),
            ((1, 100_000), (800, 2160)),
            ((1024, 768), (1024, 768)),
            ((800, 2160), (800, 2160)),
        ] {
            std::fs::write(
                &path,
                serde_json::json!({
                    "schema_version": SCHEMA_VERSION,
                    "theme": "github",
                    "window_size": {"width": stored.0, "height": stored.1}
                })
                .to_string(),
            )
            .expect("write");
            let config = load_user_config_at(&path);
            assert_eq!(
                (config.window_size.width, config.window_size.height),
                loaded,
                "{stored:?}"
            );
            // Clamping touches only the size.
            assert_eq!(config.theme, ThemeId::Github);
        }
    }

    #[test]
    fn a_user_config_past_the_bound_returns_defaults() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("frontend.json");
        let json = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "theme": "github",
            "window_size": {"width": 1024, "height": 768}
        })
        .to_string();
        // Trailing whitespace keeps the document valid JSON at any length.
        let padded_to = |length: u64| {
            let padding = usize::try_from(length).expect("length") - json.len();
            format!("{json}{}", " ".repeat(padding))
        };
        std::fs::write(&path, padded_to(MAX_USER_CONFIG_BYTES)).expect("write");
        assert_eq!(load_user_config_at(&path).theme, ThemeId::Github);
        std::fs::write(&path, padded_to(MAX_USER_CONFIG_BYTES + 1)).expect("write");
        assert_eq!(load_user_config_at(&path), UserConfig::default());
        // A directory at the path is not a settings file.
        assert_eq!(load_user_config_at(directory.path()), UserConfig::default());
    }

    #[test]
    fn a_settings_read_stops_one_byte_past_the_bound() {
        assert_eq!(
            read_at_most(&b"12345678"[..], 8),
            Some(b"12345678".to_vec())
        );
        // An endless source ends at the bound instead of exhausting memory.
        assert_eq!(read_at_most(std::io::repeat(b' '), 8), None);
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
            ..UserConfig::default()
        };
        assert_eq!(palette_for_config(&cfg).name, "vscode");
    }

    #[test]
    fn config_without_new_fields_keeps_theme() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("frontend.json");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"theme":"claude","window_size":{"width":1280,"height":800}}"#,
        )
        .expect("write");
        let config = load_user_config_at(&path);
        assert_eq!(config.theme, ThemeId::Claude);
        assert!(!config.sidebar_collapsed);
    }

    #[test]
    fn sidebar_collapsed_round_trips() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("frontend.json");
        let config = UserConfig {
            sidebar_collapsed: true,
            ..UserConfig::default()
        };
        save_user_config_at(&config, &path).expect("save");
        assert!(load_user_config_at(&path).sidebar_collapsed);
    }
}
