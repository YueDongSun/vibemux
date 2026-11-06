//! The desktop app's own settings file: `desktop_settings.json`, schema 1,
//! in the Tauri app config directory.
//!
//! The file is separate from the egui GUI's settings and is never imported
//! from it. A missing, unreadable, oversized, malformed, non-regular, or
//! unknown-version file falls back to the defaults instead of failing the app.
//! Saves create an exclusive, uniquely named sibling before replacing the
//! settings file. Independent app instances use last-writer-wins semantics;
//! their staged files never share a name or overwrite an existing entry.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SETTINGS_SCHEMA_VERSION: u16 = 1;
pub const SETTINGS_FILE_NAME: &str = "desktop_settings.json";
pub const MAX_PROJECT_FOLDERS: usize = 16;
pub const MAX_ASK_CONFIRMED_PROJECTS: usize = 64;
const MAX_SETTINGS_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartPage {
    #[default]
    Chat,
    Tasks,
    Console,
}

/// The whole settings file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopSettings {
    pub schema_version: u16,
    pub appearance: Appearance,
    pub text_size: TextSize,
    pub density: Density,
    pub reduce_motion: bool,
    pub sidebar_collapsed: bool,
    pub start_page: StartPage,
    pub project_folders: Vec<PathBuf>,
    pub ask_confirmed_projects: Vec<PathBuf>,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            appearance: Appearance::default(),
            text_size: TextSize::default(),
            density: Density::default(),
            reduce_motion: false,
            sidebar_collapsed: false,
            start_page: StartPage::default(),
            project_folders: Vec::new(),
            ask_confirmed_projects: Vec::new(),
        }
    }
}

impl DesktopSettings {
    /// Applies the fields a patch carries and leaves the rest alone.
    pub fn apply(&mut self, patch: &SettingsPatch) {
        if let Some(appearance) = patch.appearance {
            self.appearance = appearance;
        }
        if let Some(text_size) = patch.text_size {
            self.text_size = text_size;
        }
        if let Some(density) = patch.density {
            self.density = density;
        }
        if let Some(reduce_motion) = patch.reduce_motion {
            self.reduce_motion = reduce_motion;
        }
        if let Some(sidebar_collapsed) = patch.sidebar_collapsed {
            self.sidebar_collapsed = sidebar_collapsed;
        }
        if let Some(start_page) = patch.start_page {
            self.start_page = start_page;
        }
    }

    /// The folder lists are capped, so a damaged file cannot make the app
    /// carry more folders than the limit allows.
    #[must_use]
    pub fn bounded(mut self) -> Self {
        self.project_folders.truncate(MAX_PROJECT_FOLDERS);
        self.ask_confirmed_projects
            .truncate(MAX_ASK_CONFIRMED_PROJECTS);
        self
    }
}

/// The fields a `settings_set` call may change. The folder lists are not
/// part of a patch: they change through the project commands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsPatch {
    pub appearance: Option<Appearance>,
    pub text_size: Option<TextSize>,
    pub density: Option<Density>,
    pub reduce_motion: Option<bool>,
    pub sidebar_collapsed: Option<bool>,
    pub start_page: Option<StartPage>,
}

/// Loads a regular settings file of at most 64 KiB. Missing, malformed,
/// oversized, non-regular, or unknown-version files give the defaults.
#[must_use]
pub fn load_settings_at(path: &Path) -> DesktopSettings {
    let Some(bytes) = read_settings_bytes(path) else {
        return DesktopSettings::default();
    };
    let Ok(settings) = serde_json::from_slice::<DesktopSettings>(&bytes) else {
        return DesktopSettings::default();
    };
    if settings.schema_version != SETTINGS_SCHEMA_VERSION {
        return DesktopSettings::default();
    }
    settings.bounded()
}

fn read_settings_bytes(path: &Path) -> Option<Vec<u8>> {
    // Reject special files before opening them: opening a FIFO can block.
    // Settings symlinks remain supported for dotfile managers.
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_SETTINGS_BYTES).then_some(bytes)
}

/// Writes a bounded settings document through an exclusively created sibling
/// and a rename. Failed writes remove only the file this call created.
/// Independent app instances may replace each other's complete snapshots;
/// this function does not merge their settings.
pub fn save_settings_at(path: &Path, settings: &DesktopSettings) -> io::Result<()> {
    let mut stored = settings.clone().bounded();
    stored.schema_version = SETTINGS_SCHEMA_VERSION;
    let text = serde_json::to_string_pretty(&stored).map_err(io::Error::other)?;
    if text.len() > MAX_SETTINGS_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "desktop settings exceed the 64 KiB limit",
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let temporary = path.with_file_name(name);
    write_new(&temporary, text.as_bytes())?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    written.inspect_err(|_| {
        let _ = fs::remove_file(path);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let settings = load_settings_at(&directory.path().join("desktop_settings.json"));
        assert_eq!(settings, DesktopSettings::default());
        assert_eq!(settings.appearance, Appearance::System);
        assert!(settings.project_folders.is_empty());
        assert!(settings.ask_confirmed_projects.is_empty());
    }

    #[test]
    fn invalid_json_gives_the_defaults() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        std::fs::write(&path, "{not json").expect("file");
        assert_eq!(load_settings_at(&path), DesktopSettings::default());
    }

    #[test]
    fn an_unknown_schema_version_gives_the_defaults() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let settings = DesktopSettings {
            schema_version: SETTINGS_SCHEMA_VERSION + 1,
            ..DesktopSettings::default()
        };
        std::fs::write(&path, serde_json::to_string(&settings).expect("JSON")).expect("file");
        assert_eq!(load_settings_at(&path), DesktopSettings::default());
    }

    #[test]
    fn saving_and_loading_round_trips() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let settings = DesktopSettings {
            appearance: Appearance::Dark,
            text_size: TextSize::Large,
            density: Density::Compact,
            reduce_motion: true,
            sidebar_collapsed: true,
            start_page: StartPage::Console,
            project_folders: vec![PathBuf::from("C:/projects/one")],
            ask_confirmed_projects: vec![PathBuf::from("C:/projects/one")],
            ..DesktopSettings::default()
        };
        save_settings_at(&path, &settings).expect("saved");
        assert_eq!(load_settings_at(&path), settings);
    }

    #[test]
    fn the_folder_lists_are_capped() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let settings = DesktopSettings {
            project_folders: (0..20)
                .map(|index| PathBuf::from(format!("C:/projects/{index}")))
                .collect(),
            ask_confirmed_projects: (0..80)
                .map(|index| PathBuf::from(format!("C:/confirmed/{index}")))
                .collect(),
            ..DesktopSettings::default()
        };
        save_settings_at(&path, &settings).expect("saved");
        let loaded = load_settings_at(&path);
        assert_eq!(loaded.project_folders.len(), MAX_PROJECT_FOLDERS);
        assert_eq!(
            loaded.ask_confirmed_projects.len(),
            MAX_ASK_CONFIRMED_PROJECTS
        );
    }

    #[test]
    fn a_patch_changes_only_the_fields_it_carries() {
        let mut settings = DesktopSettings {
            appearance: Appearance::Dark,
            ..DesktopSettings::default()
        };
        let patch = SettingsPatch {
            density: Some(Density::Compact),
            ..SettingsPatch::default()
        };
        settings.apply(&patch);
        assert_eq!(settings.appearance, Appearance::Dark);
        assert_eq!(settings.density, Density::Compact);
    }

    #[test]
    fn settings_at_the_byte_limit_load_but_larger_files_give_defaults() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let settings = DesktopSettings {
            appearance: Appearance::Dark,
            ..DesktopSettings::default()
        };
        let mut bytes = serde_json::to_vec(&settings).expect("JSON");
        bytes.resize(MAX_SETTINGS_BYTES, b' ');
        fs::write(&path, &bytes).expect("bounded settings file");
        assert_eq!(load_settings_at(&path), settings);
        bytes.push(b' ');
        fs::write(&path, &bytes).expect("oversized settings file");
        assert_eq!(load_settings_at(&path), DesktopSettings::default());
    }

    #[test]
    fn an_oversized_save_preserves_existing_settings() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let original = DesktopSettings::default();
        save_settings_at(&path, &original).expect("initial settings");
        let oversized = DesktopSettings {
            project_folders: vec![PathBuf::from("x".repeat(MAX_SETTINGS_BYTES))],
            ..original.clone()
        };
        let error = save_settings_at(&path, &oversized).expect_err("oversized save refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(load_settings_at(&path), original);
        assert_eq!(fs::read_dir(directory.path()).expect("entries").count(), 1);
    }

    #[test]
    fn a_directory_is_not_a_settings_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            load_settings_at(directory.path()),
            DesktopSettings::default()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_rejected_without_opening_it() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .expect("mkfifo")
                .success()
        );
        assert_eq!(load_settings_at(&path), DesktopSettings::default());
    }

    #[test]
    fn a_staging_collision_preserves_the_existing_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("taken.tmp");
        fs::write(&path, b"another writer owns this").expect("existing file");
        let error = write_new(&path, b"new settings").expect_err("collision refused");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&path).expect("existing file"),
            b"another writer owns this"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_staging_symlink_is_not_followed_or_removed() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("unrelated.txt");
        let settings = directory.path().join("desktop_settings.json");
        let staging = settings.with_extension("json.tmp");
        fs::write(&target, b"unrelated user data").expect("unrelated file");
        std::os::unix::fs::symlink(&target, &staging).expect("staging symlink");

        let error = write_new(&staging, b"new settings").expect_err("symlink refused");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        let expected = DesktopSettings::default();
        save_settings_at(&settings, &expected).expect("unique staging save");
        assert_eq!(load_settings_at(&settings), expected);
        assert!(
            !fs::symlink_metadata(&settings)
                .expect("settings")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(&target).expect("unrelated file"),
            b"unrelated user data"
        );
        assert_eq!(fs::read_link(&staging).expect("untouched symlink"), target);
    }

    #[test]
    fn failed_replacement_removes_its_staging_file_only() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        fs::create_dir(&path).expect("non-file destination");
        let owned_elsewhere = directory.path().join("another-writer.tmp");
        fs::write(&owned_elsewhere, b"keep").expect("unrelated staging file");
        assert!(save_settings_at(&path, &DesktopSettings::default()).is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read(&owned_elsewhere).expect("untouched file"), b"keep");
        assert_eq!(fs::read_dir(directory.path()).expect("entries").count(), 2);
    }
}
