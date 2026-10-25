//! The desktop app's own settings file: `desktop_settings.json`, schema 1,
//! in the Tauri app config directory.
//!
//! The file is separate from the egui GUI's settings and is never imported
//! from it. A missing, unreadable, malformed, or unknown-version file
//! falls back to the defaults instead of failing the app.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SETTINGS_SCHEMA_VERSION: u16 = 1;
pub const SETTINGS_FILE_NAME: &str = "desktop_settings.json";
pub const MAX_PROJECT_FOLDERS: usize = 16;
pub const MAX_ASK_CONFIRMED_PROJECTS: usize = 64;

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

/// Reads the settings file. A missing, malformed, or unknown-version file
/// gives the defaults.
#[must_use]
pub fn load_settings_at(path: &Path) -> DesktopSettings {
    let Ok(text) = std::fs::read_to_string(path) else {
        return DesktopSettings::default();
    };
    let Ok(settings) = serde_json::from_str::<DesktopSettings>(&text) else {
        return DesktopSettings::default();
    };
    if settings.schema_version != SETTINGS_SCHEMA_VERSION {
        return DesktopSettings::default();
    }
    settings.bounded()
}

/// Writes the settings file through a temporary file and a rename, so a
/// crash mid-write cannot leave a half-written file behind.
pub fn save_settings_at(path: &Path, settings: &DesktopSettings) -> io::Result<()> {
    let mut stored = settings.clone().bounded();
    stored.schema_version = SETTINGS_SCHEMA_VERSION;
    let text = serde_json::to_string_pretty(&stored).map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
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
}
