//! The state the Tauri app owns: the settings file and the event sink.
//!
//! The daemon credentials never live here. They stay inside
//! `vibemux_client`, which talks to the daemon over the local control
//! channel; the webview only sees the command results.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::desktop_settings::{DesktopSettings, SettingsPatch, load_settings_at, save_settings_at};
use crate::event_sink::EventSink;

pub struct AppState {
    settings_path: PathBuf,
    settings: RwLock<DesktopSettings>,
    sink: Arc<dyn EventSink>,
}

impl AppState {
    #[must_use]
    pub fn new(settings_path: PathBuf, sink: Arc<dyn EventSink>) -> Self {
        let settings = load_settings_at(&settings_path);
        Self {
            settings_path,
            settings: RwLock::new(settings),
            sink,
        }
    }

    /// The current settings.
    #[must_use]
    pub fn settings(&self) -> DesktopSettings {
        self.settings
            .read()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    /// Where the settings file lives.
    #[must_use]
    pub fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    /// Applies a patch and writes the file. The in-memory copy changes
    /// only when the write succeeds.
    pub fn apply_settings(&self, patch: &SettingsPatch) -> io::Result<()> {
        let updated = {
            let settings = self.settings.read().map_err(io::Error::other)?;
            let mut updated = settings.clone();
            updated.apply(patch);
            updated
        };
        save_settings_at(&self.settings_path, &updated)?;
        let mut settings = self.settings.write().map_err(io::Error::other)?;
        *settings = updated;
        Ok(())
    }

    /// The sink the services emit their events to.
    #[must_use]
    pub fn sink(&self) -> &Arc<dyn EventSink> {
        &self.sink
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_settings::{Appearance, Density};
    use crate::event_sink::RecordingEventSink;

    #[test]
    fn the_settings_file_is_read_at_construction() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let mut settings = DesktopSettings::default();
        settings.appearance = Appearance::Dark;
        settings.density = Density::Compact;
        save_settings_at(&path, &settings).expect("saved");

        let state = AppState::new(path, Arc::new(RecordingEventSink::default()));
        assert_eq!(state.settings().appearance, Appearance::Dark);
        assert_eq!(state.settings().density, Density::Compact);
    }

    #[test]
    fn a_patch_is_written_to_the_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let state = AppState::new(path.clone(), Arc::new(RecordingEventSink::default()));
        let patch = SettingsPatch {
            appearance: Some(Appearance::Dark),
            ..SettingsPatch::default()
        };
        state.apply_settings(&patch).expect("applied");
        assert_eq!(state.settings().appearance, Appearance::Dark);
        let stored = load_settings_at(&path);
        assert_eq!(stored.appearance, Appearance::Dark);
    }
}
