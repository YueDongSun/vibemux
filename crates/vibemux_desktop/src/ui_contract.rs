//! The DTOs and events the Rust side sends to the webview.
//!
//! Events carry no vendor text: they name what changed, and the webview
//! fetches what it shows. `ask_reply` is the only command that returns
//! vendor content.

use serde::Serialize;

pub use crate::provider_profiles::ProviderProfilesView;

use crate::desktop_settings::{Appearance, Density, DesktopSettings, StartPage, TextSize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    pub schema_version: u16,
    pub appearance: Appearance,
    pub text_size: TextSize,
    pub density: Density,
    pub reduce_motion: bool,
    pub sidebar_collapsed: bool,
    pub start_page: StartPage,
}

impl From<DesktopSettings> for SettingsView {
    fn from(settings: DesktopSettings) -> Self {
        Self {
            schema_version: settings.schema_version,
            appearance: settings.appearance,
            text_size: settings.text_size,
            density: settings.density,
            reduce_motion: settings.reduce_motion,
            sidebar_collapsed: settings.sidebar_collapsed,
            start_page: settings.start_page,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UiEvent {
    ProjectsChanged,
    SnapshotChanged { project_key: String, revision: u64 },
    AskChanged { ask_id: String, phase: String },
    ConsoleChanged { project_key: String },
}

impl UiEvent {
    /// The event name the webview listens for.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::ProjectsChanged => "projects_changed",
            Self::SnapshotChanged { .. } => "snapshot_changed",
            Self::AskChanged { .. } => "ask_changed",
            Self::ConsoleChanged { .. } => "console_changed",
        }
    }

    /// The event payload. It carries the `event` tag, so a payload names
    /// the event it belongs to.
    #[must_use]
    pub fn payload(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AboutInfo {
    pub app_version: String,
    pub webview_version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_views_do_not_disclose_native_folder_lists() {
        let settings = DesktopSettings {
            appearance: Appearance::Dark,
            text_size: TextSize::Large,
            project_folders: vec!["private-project-folder".into()],
            ask_confirmed_projects: vec!["private-consent-folder".into()],
            ..DesktopSettings::default()
        };
        let value = serde_json::to_value(SettingsView::from(settings)).expect("view");
        assert_eq!(value["appearance"], "dark");
        assert_eq!(value["text_size"], "large");
        assert_eq!(value.as_object().expect("object").len(), 7);
        for field in ["project_folders", "ask_confirmed_projects", "settings_path"] {
            assert!(value.get(field).is_none());
        }
        let text = value.to_string();
        assert!(!text.contains("private-project-folder"));
        assert!(!text.contains("private-consent-folder"));
    }

    #[test]
    fn the_event_name_matches_the_tagged_payload() {
        let event = UiEvent::SnapshotChanged {
            project_key: "abc123".to_owned(),
            revision: 7,
        };
        assert_eq!(event.name(), "snapshot_changed");
        let payload = event.payload();
        assert_eq!(payload["event"], "snapshot_changed");
        assert_eq!(payload["project_key"], "abc123");
        assert_eq!(payload["revision"], 7);
    }

    #[test]
    fn an_event_carries_no_content_field() {
        let event = UiEvent::AskChanged {
            ask_id: "ask-1".to_owned(),
            phase: "running".to_owned(),
        };
        let payload = event.payload();
        assert_eq!(payload["event"], "ask_changed");
        assert_eq!(payload["ask_id"], "ask-1");
        assert_eq!(payload["phase"], "running");
        assert_eq!(payload.as_object().map(|object| object.len()), Some(3));
    }
}
