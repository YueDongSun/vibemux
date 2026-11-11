//! The state the Tauri app owns: native configuration and the event sink.
//!
//! The daemon credentials never live here. They stay inside
//! `vibemux_client`, which talks to the daemon over the local control
//! channel; the webview only sees the command results.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, TryLockError};

use crate::desktop_settings::{DesktopSettings, SettingsPatch, load_settings_at, save_settings_at};
use crate::event_sink::EventSink;
use crate::provider_profiles::{
    PROVIDER_PROFILES_FILE_NAME, ProviderProfilesConfig, ProviderProfilesError,
    ProviderProfilesView, load_provider_profiles_at, save_provider_profiles_at,
};

pub struct AppState {
    settings_path: PathBuf,
    settings: RwLock<DesktopSettings>,
    provider_profiles: Mutex<()>,
    sink: Arc<dyn EventSink>,
}

impl AppState {
    #[must_use]
    pub fn new(settings_path: PathBuf, sink: Arc<dyn EventSink>) -> Self {
        let settings = load_settings_at(&settings_path);
        Self {
            settings_path,
            settings: RwLock::new(settings),
            provider_profiles: Mutex::new(()),
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

    /// Applies a patch and writes the file. The write lock is held across
    /// the whole transaction — reading the current settings, applying the
    /// patch, writing the file, and publishing the new value — so
    /// overlapping calls on this instance serialize instead of losing updates.
    /// Other app instances keep separate snapshots; their complete saves use
    /// last-writer-wins semantics. The in-memory copy changes only when the
    /// write succeeds.
    pub fn apply_settings(&self, patch: &SettingsPatch) -> io::Result<()> {
        let mut settings = self
            .settings
            .write()
            .map_err(|_| io::Error::other("the settings lock is poisoned"))?;
        let mut updated = settings.clone();
        updated.apply(patch);
        save_settings_at(&self.settings_path, &updated)?;
        *settings = updated;
        Ok(())
    }

    /// Reads current on-disk metadata, not a cached execution configuration.
    /// The caller must run this filesystem operation on a blocking worker.
    pub fn provider_profiles(&self) -> Result<ProviderProfilesView, ProviderProfilesError> {
        let _guard = self.lock_provider_profiles()?;
        load_provider_profiles_at(&self.provider_profiles_path()?).map(Into::into)
    }

    /// Publishes one complete, validated metadata snapshot. It emits no event
    /// and changes no daemon settings, credential grant, or admitted route.
    pub fn save_provider_profiles(
        &self,
        config: ProviderProfilesConfig,
    ) -> Result<ProviderProfilesView, ProviderProfilesError> {
        let _guard = self.lock_provider_profiles()?;
        save_provider_profiles_at(&self.provider_profiles_path()?, &config)?;
        Ok(config.into())
    }

    fn provider_profiles_path(&self) -> Result<PathBuf, ProviderProfilesError> {
        if !self.settings_path.is_absolute() {
            return Err(ProviderProfilesError::Unavailable);
        }
        self.settings_path
            .parent()
            .map(|directory| directory.join(PROVIDER_PROFILES_FILE_NAME))
            .ok_or(ProviderProfilesError::Unavailable)
    }

    fn lock_provider_profiles(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, ()>, ProviderProfilesError> {
        self.provider_profiles
            .try_lock()
            .map_err(|error| match error {
                TryLockError::WouldBlock => ProviderProfilesError::Busy,
                TryLockError::Poisoned(_) => ProviderProfilesError::Unavailable,
            })
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
    fn provider_profiles_are_separate_from_appearance_and_refresh_from_disk() {
        use crate::provider_profiles::{ProviderApiType, ProviderAuthType, ProviderProfile};

        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("desktop_settings.json");
        let first = AppState::new(path.clone(), Arc::new(RecordingEventSink::default()));
        first
            .apply_settings(&SettingsPatch {
                appearance: Some(Appearance::Dark),
                ..SettingsPatch::default()
            })
            .expect("appearance");
        let appearance_bytes = std::fs::read(&path).expect("appearance bytes");
        let second = AppState::new(path.clone(), Arc::new(RecordingEventSink::default()));
        let configured = ProviderProfilesConfig {
            schema_version: 1,
            profiles: vec![ProviderProfile {
                id: "local".to_owned(),
                name: "Synthetic local provider".to_owned(),
                api_type: ProviderApiType::OpenaiChatCompletions,
                base_url: "http://127.0.0.1:11434/v1".to_owned(),
                model: "operator-model".to_owned(),
                credential_env: None,
                auth_type: ProviderAuthType::None,
            }],
            orchestrator_profile_id: Some("local".to_owned()),
        };
        let saved = first
            .save_provider_profiles(configured.clone())
            .expect("save");
        assert_eq!(saved.config, configured);
        assert_eq!(second.provider_profiles().expect("fresh disk read"), saved);
        assert_eq!(first.settings().appearance, Appearance::Dark);
        assert_eq!(
            std::fs::read(&path).expect("appearance remains intact"),
            appearance_bytes
        );
        assert!(directory.path().join(PROVIDER_PROFILES_FILE_NAME).is_file());
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("entries")
                .count(),
            2
        );
    }

    #[test]
    fn provider_configuration_does_not_fall_back_to_the_working_directory() {
        let state = AppState::new(
            PathBuf::from("synthetic-missing-desktop-settings.json"),
            Arc::new(RecordingEventSink::default()),
        );
        assert_eq!(
            state.provider_profiles(),
            Err(ProviderProfilesError::Unavailable)
        );
        assert_eq!(
            state.save_provider_profiles(ProviderProfilesConfig::default()),
            Err(ProviderProfilesError::Unavailable)
        );
    }

    #[test]
    fn concurrent_provider_operations_fail_busy_instead_of_waiting() {
        let directory = tempfile::tempdir().expect("directory");
        let state = AppState::new(
            directory.path().join("desktop_settings.json"),
            Arc::new(RecordingEventSink::default()),
        );
        let guard = state.provider_profiles.lock().expect("owned lock");
        assert_eq!(state.provider_profiles(), Err(ProviderProfilesError::Busy));
        assert_eq!(
            state.save_provider_profiles(ProviderProfilesConfig::default()),
            Err(ProviderProfilesError::Busy)
        );
        drop(guard);
        assert_eq!(
            state.provider_profiles().expect("released").config,
            ProviderProfilesConfig::default()
        );
    }

    #[test]
    fn the_settings_file_is_read_at_construction() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let settings = DesktopSettings {
            appearance: Appearance::Dark,
            density: Density::Compact,
            ..DesktopSettings::default()
        };
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

    #[test]
    fn overlapping_patches_are_both_applied() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let state = Arc::new(AppState::new(
            path.clone(),
            Arc::new(RecordingEventSink::default()),
        ));

        let appearance_state = Arc::clone(&state);
        let appearance = std::thread::spawn(move || {
            appearance_state.apply_settings(&SettingsPatch {
                appearance: Some(Appearance::Dark),
                ..SettingsPatch::default()
            })
        });
        let density_state = Arc::clone(&state);
        let density = std::thread::spawn(move || {
            density_state.apply_settings(&SettingsPatch {
                density: Some(Density::Compact),
                ..SettingsPatch::default()
            })
        });
        appearance.join().expect("thread").expect("applied");
        density.join().expect("thread").expect("applied");

        let stored = load_settings_at(&path);
        assert_eq!(stored.appearance, Appearance::Dark);
        assert_eq!(stored.density, Density::Compact);
    }

    #[test]
    fn independent_instances_save_complete_snapshots_without_staging_collisions() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let states = [
            AppState::new(path.clone(), Arc::new(RecordingEventSink::default())),
            AppState::new(path.clone(), Arc::new(RecordingEventSink::default())),
        ];
        let handles: Vec<_> = states
            .into_iter()
            .enumerate()
            .map(|(index, state)| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let patch = SettingsPatch {
                        appearance: Some(if index == 0 {
                            Appearance::Dark
                        } else {
                            Appearance::Light
                        }),
                        density: Some(if index == 0 {
                            Density::Compact
                        } else {
                            Density::Comfortable
                        }),
                        ..SettingsPatch::default()
                    };
                    let mut outcomes = Vec::new();
                    for _ in 0..32 {
                        barrier.wait();
                        outcomes.push(state.apply_settings(&patch));
                    }
                    for result in outcomes {
                        result.expect("independent settings save");
                    }
                    state.settings()
                })
            })
            .collect();
        let snapshots: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("writer"))
            .collect();
        let bytes = std::fs::read(&path).expect("settings file");
        let stored: DesktopSettings = serde_json::from_slice(&bytes).expect("complete JSON");
        assert!(snapshots.contains(&stored));
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("entries")
                .count(),
            1
        );
    }
}
