#![forbid(unsafe_code)]
//! On-disk persistence for the frontend user settings.
//!
//! Reads/writes a small JSON document keyed by platform:
//! - Windows: `%APPDATA%\vibemux\frontend.json`
//! - Off-Windows: `$XDG_CONFIG_HOME/vibemux/frontend.json` (falling
//!   back to `$HOME/.config/vibemux/frontend.json`)
//!
//! All reads are tolerant: missing, corrupt, or oversized files return
//! [`UserConfig::default`]. Writes are atomic (write a uniquely named
//! staged file, rename), and a failed save removes the file it staged.
//! We never hold an OS lock; concurrent writers can clobber each
//! other, but the worst case is a lost write of the theme id.

use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::{Density, ThemeId, palette_for};

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

/// The page the GUI opens on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartPage {
    #[default]
    Conversation,
    /// Saved as `agents` before the Console replaced the Agents page.
    #[serde(alias = "agents")]
    Console,
}

impl StartPage {
    pub const ALL: [Self; 2] = [Self::Conversation, Self::Console];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Conversation => "Conversation",
            Self::Console => "Console",
        }
    }
}

/// How large text and controls are drawn. Applied as the egui zoom
/// factor, so text, spacing, and controls scale together.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
}

impl TextSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Default, Self::Large];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Default => "Default",
            Self::Large => "Large",
        }
    }

    #[must_use]
    pub const fn zoom_factor(self) -> f32 {
        match self {
            Self::Small => 0.9,
            Self::Default => 1.0,
            Self::Large => 1.15,
        }
    }
}

/// Persisted across launches: theme, startup window size, sidebar state,
/// start page, density, and text size. Backend choice and telemetry
/// opt-in are deliberately not persisted. Fields added after the first
/// release default when absent, so older files keep loading under the
/// same schema version.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserConfig {
    pub schema_version: u16,
    pub theme: ThemeId,
    pub window_size: WindowSize,
    /// Sidebar collapsed to its icon rail (ADR 030 §7).
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub start_page: StartPage,
    /// Spacing between controls; independent of the theme.
    #[serde(default = "default_density")]
    pub density: Density,
    #[serde(default)]
    pub text_size: TextSize,
    /// The project folder the GUI opens and observes. `None` falls back to
    /// the directory the GUI was started from.
    #[serde(default)]
    pub active_project: Option<PathBuf>,
    /// Recently opened project folders, most recent first, at most
    /// [`MAX_RECENT_PROJECTS`].
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
    /// Folders where the user confirmed that Ask may run harness turns,
    /// which may use paid inference. Asked once per folder.
    #[serde(default)]
    pub ask_confirmed_projects: Vec<PathBuf>,
}

/// How many confirmed Ask folders are remembered.
pub const MAX_ASK_CONFIRMED_PROJECTS: usize = 64;

/// How many recent project folders the GUI remembers.
pub const MAX_RECENT_PROJECTS: usize = 8;

impl UserConfig {
    /// Makes `folder` the active project and moves it to the front of the
    /// recent list, without duplicates and within the bound.
    pub fn remember_project(&mut self, folder: PathBuf) {
        self.recent_projects
            .retain(|known| !same_folder(known, &folder));
        self.recent_projects.insert(0, folder.clone());
        self.recent_projects.truncate(MAX_RECENT_PROJECTS);
        self.active_project = Some(folder);
    }

    /// Drops `folder` from the recent list. The active project is kept.
    pub fn forget_project(&mut self, folder: &Path) {
        self.recent_projects
            .retain(|known| !same_folder(known, folder));
    }

    /// Whether the user already confirmed Ask for `folder`.
    #[must_use]
    pub fn ask_confirmed(&self, folder: &Path) -> bool {
        self.ask_confirmed_projects
            .iter()
            .any(|known| same_folder(known, folder))
    }

    /// Remembers the user's Ask confirmation for `folder`, newest first.
    pub fn confirm_ask(&mut self, folder: PathBuf) {
        self.ask_confirmed_projects
            .retain(|known| !same_folder(known, &folder));
        self.ask_confirmed_projects.insert(0, folder);
        self.ask_confirmed_projects
            .truncate(MAX_ASK_CONFIRMED_PROJECTS);
    }
}

/// Whether two folder paths name the same folder. Windows paths compare
/// case-insensitively; others compare exactly.
#[must_use]
pub fn same_folder(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn default_density() -> Density {
    super::DENSITY_DEFAULT
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
            start_page: StartPage::default(),
            density: default_density(),
            text_size: TextSize::default(),
            active_project: None,
            recent_projects: Vec::new(),
            ask_confirmed_projects: Vec::new(),
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
        Ok(cfg) if cfg.schema_version == SCHEMA_VERSION => {
            let mut recent_projects = cfg.recent_projects;
            recent_projects.truncate(MAX_RECENT_PROJECTS);
            let mut ask_confirmed_projects = cfg.ask_confirmed_projects;
            ask_confirmed_projects.truncate(MAX_ASK_CONFIRMED_PROJECTS);
            UserConfig {
                window_size: cfg.window_size.clamped(),
                recent_projects,
                ask_confirmed_projects,
                ..cfg
            }
        }
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
    let staging = staging_path(path);
    write_new(&staging, serialised.as_bytes())?;
    fs::rename(&staging, path)
        .or_else(|first| {
            // Retry once after a small delay; another process may be
            // closing the file on Windows. We do not retry forever.
            std::thread::sleep(std::time::Duration::from_millis(50));
            fs::rename(&staging, path).map_err(|_| first)
        })
        .inspect_err(|_| {
            let _ = fs::remove_file(&staging);
        })
}

/// A hidden sibling of `path` named for this process and moment. Two
/// saves that still draw the same name do not share it: the second
/// open fails instead of overwriting the first.
fn staging_path(path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".{}.{nanos}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Writes `bytes` to a file created at `path`, which must not exist yet.
/// A failed write removes the file it created; a name already taken is
/// left alone, since another save owns it.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let written = file.write_all(bytes);
    drop(file);
    if written.is_err() {
        let _ = fs::remove_file(path);
    }
    written
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

    #[test]
    fn preferences_added_later_default_when_absent_and_round_trip() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("frontend.json");
        fs::write(
            &path,
            r#"{"schema_version":1,"theme":"nord","window_size":{"width":1280,"height":800}}"#,
        )
        .expect("an older settings file");
        let loaded = load_user_config_at(&path);
        assert_eq!(loaded.theme, ThemeId::Nord);
        assert_eq!(loaded.start_page, StartPage::Conversation);
        assert_eq!(loaded.density, Density::Comfortable);
        assert_eq!(loaded.text_size, TextSize::Default);

        let changed = UserConfig {
            start_page: StartPage::Console,
            density: Density::Compact,
            text_size: TextSize::Large,
            ..loaded
        };
        save_user_config_at(&changed, &path).expect("save");
        assert_eq!(load_user_config_at(&path), changed);
    }

    #[test]
    fn a_saved_agents_start_page_opens_the_console() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("frontend.json");
        fs::write(
            &path,
            r#"{"schema_version":1,"theme":"nord","window_size":{"width":1280,"height":800},"start_page":"agents"}"#,
        )
        .expect("a settings file from before the Console");
        assert_eq!(load_user_config_at(&path).start_page, StartPage::Console);
        let saved = serde_json::to_value(StartPage::Console).expect("encode");
        assert_eq!(saved, "console");
    }

    #[test]
    fn recent_projects_stay_unique_ordered_and_bounded() {
        let mut config = UserConfig::default();
        for index in 0..MAX_RECENT_PROJECTS + 3 {
            config.remember_project(PathBuf::from(format!("projects/{index}")));
        }
        assert_eq!(config.recent_projects.len(), MAX_RECENT_PROJECTS);
        let newest = PathBuf::from(format!("projects/{}", MAX_RECENT_PROJECTS + 2));
        assert_eq!(config.recent_projects[0], newest);

        let again = PathBuf::from("projects/5");
        config.remember_project(again.clone());
        assert_eq!(config.recent_projects[0], again);
        assert_eq!(
            config
                .recent_projects
                .iter()
                .filter(|folder| **folder == again)
                .count(),
            1
        );
        assert_eq!(config.active_project.as_ref(), Some(&again));

        config.forget_project(&again);
        assert!(!config.recent_projects.contains(&again));
        assert_eq!(
            config.active_project.as_ref(),
            Some(&again),
            "forgetting a recent folder keeps the open one"
        );
    }

    #[test]
    fn an_oversized_recent_list_is_cut_on_load() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("frontend.json");
        let config = UserConfig {
            recent_projects: (0..MAX_RECENT_PROJECTS + 4)
                .map(|index| PathBuf::from(format!("projects/{index}")))
                .collect(),
            ..UserConfig::default()
        };
        save_user_config_at(&config, &path).expect("save");
        assert_eq!(
            load_user_config_at(&path).recent_projects.len(),
            MAX_RECENT_PROJECTS
        );
    }

    #[test]
    fn ask_confirmation_is_remembered_per_folder() {
        let mut config = UserConfig::default();
        let first = PathBuf::from("projects/first");
        assert!(!config.ask_confirmed(&first));
        config.confirm_ask(first.clone());
        config.confirm_ask(first.clone());
        assert!(config.ask_confirmed(&first));
        assert_eq!(config.ask_confirmed_projects.len(), 1, "no duplicates");
        assert!(!config.ask_confirmed(Path::new("projects/second")));
        for index in 0..MAX_ASK_CONFIRMED_PROJECTS + 2 {
            config.confirm_ask(PathBuf::from(format!("projects/{index}")));
        }
        assert_eq!(
            config.ask_confirmed_projects.len(),
            MAX_ASK_CONFIRMED_PROJECTS
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_folders_compare_without_case() {
        assert!(same_folder(
            Path::new("C:\\Projects\\App"),
            Path::new("c:\\projects\\app")
        ));
        assert!(!same_folder(
            Path::new("C:\\Projects\\App"),
            Path::new("C:\\Projects\\Other")
        ));
    }

    #[test]
    fn text_sizes_scale_around_the_default() {
        assert!(TextSize::Small.zoom_factor() < TextSize::Default.zoom_factor());
        assert_eq!(TextSize::Default.zoom_factor(), 1.0);
        assert!(TextSize::Large.zoom_factor() > TextSize::Default.zoom_factor());
    }

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
    fn a_save_leaves_only_the_settings_file() {
        let file_names = |directory: &Path| -> Vec<std::ffi::OsString> {
            fs::read_dir(directory)
                .expect("read dir")
                .map(|entry| entry.expect("entry").file_name())
                .collect()
        };
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("frontend.json");
        save_user_config_at(&UserConfig::default(), &path).expect("save");
        assert_eq!(file_names(directory.path()), ["frontend.json"]);

        // A non-empty directory at the path refuses the rename; the staged
        // file is removed and the directory is left as it was.
        let blocked = tempfile::tempdir().expect("tempdir");
        let path = blocked.path().join("frontend.json");
        fs::create_dir(&path).expect("mkdir");
        fs::write(path.join("kept"), b"").expect("write");
        assert!(save_user_config_at(&UserConfig::default(), &path).is_err());
        assert_eq!(file_names(blocked.path()), ["frontend.json"]);
        assert_eq!(file_names(&path), ["kept"]);
    }

    #[test]
    fn a_staged_file_is_a_hidden_sibling_named_for_this_process() {
        let path = Path::new("settings").join("frontend.json");
        let staging = staging_path(&path);
        assert_eq!(staging.parent(), path.parent());
        let name = staging
            .file_name()
            .and_then(|name| name.to_str())
            .expect("name");
        let prefix = format!(".frontend.json.{}.", std::process::id());
        assert!(name.starts_with(&prefix), "{name}");
        assert!(name.ends_with(".tmp"), "{name}");
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
