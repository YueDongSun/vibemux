//! The app's windows.
//!
//! The windows are created in Rust, not from the config, so every one of
//! them gets the navigation guard.
//!
//! The window always loads the app URL, which Tauri resolves to the
//! configured dev server only in a dev build (`cargo run
//! --no-default-features` or `tauri dev`). A plain `cargo run` loads the
//! bundled `ui/dist` assets, because the crate's default features include
//! `custom-protocol`. The dev origin is passed to the navigation guard in
//! a dev build only.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::navigation::navigation_allowed;
use crate::{
    app_state::AppState,
    command_error::CommandError,
    task_windows::{TaskWindowReservation, unavailable},
};

/// Set only by the trusted Rust launcher and shared by every application window.
#[derive(Clone, Default)]
pub struct WindowProfile {
    webview_data_dir: Option<PathBuf>,
    demo: bool,
}
impl WindowProfile {
    #[cfg(all(windows, feature = "native_qa"))]
    pub(crate) fn native_qa(webview_data_dir: PathBuf) -> Self {
        Self {
            webview_data_dir: Some(webview_data_dir),
            demo: true,
        }
    }
}

pub fn open_task_window<R: Runtime>(
    app: &AppHandle<R>,
    state: &Arc<AppState>,
    reservation: &TaskWindowReservation,
) -> Result<(), CommandError> {
    if !reservation.fresh {
        if let Some(window) = app.get_webview_window(&reservation.label) {
            window
                .unminimize()
                .and_then(|()| window.show())
                .and_then(|()| window.set_focus())
                .map_err(|_| unavailable())?;
            return Ok(());
        }
        state
            .task_windows
            .release(&reservation.label, reservation.generation);
        return Err(unavailable());
    }
    let result = (|| {
        state
            .terminals
            .register_window(&crate::terminal_bridge::TerminalActor {
                label: reservation.label.clone(),
                generation: reservation.generation,
            })?;
        let profile = app.try_state::<WindowProfile>().ok_or_else(unavailable)?;
        let binding = &reservation.binding;
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("view", "task")
            .append_pair("project_key", &binding.project_key)
            .append_pair("project_id", &binding.project_id.to_string())
            .append_pair("task_id", &binding.task_id.to_string())
            .finish();
        let title = if profile.demo {
            "VibeMux · DEMO · Task"
        } else {
            "VibeMux · Task"
        };
        let window = build_window(
            app,
            &reservation.label,
            title,
            PathBuf::from(format!("index.html?{query}")),
            profile.webview_data_dir.as_deref(),
            true,
        )
        .map_err(|_| unavailable())?;
        let state_for_close = Arc::clone(state);
        let label = reservation.label.clone();
        let generation = reservation.generation;
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                state_for_close.task_windows.release(&label, generation);
                state_for_close
                    .terminals
                    .close_window(&crate::terminal_bridge::TerminalActor {
                        label: label.clone(),
                        generation,
                    });
            }
        });
        if app.get_webview_window(&reservation.label).is_none() {
            return Err(unavailable());
        }
        state.task_windows.mark_ready(reservation)?;
        Ok(())
    })();
    if result.is_err() {
        state
            .task_windows
            .release(&reservation.label, reservation.generation);
        state
            .terminals
            .close_window(&crate::terminal_bridge::TerminalActor {
                label: reservation.label.clone(),
                generation: reservation.generation,
            });
    }
    result
}

/// Opens the main window: 1280 by 820, and never smaller than 900 by 600.
pub fn open_main_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    open_window(app, "VibeMux", None)
}

/// Opens the isolated native QA profile after caller validation.
#[cfg(all(windows, feature = "native_qa"))]
pub(crate) fn open_native_qa_window(
    app: &AppHandle,
    webview_data_dir: &Path,
) -> tauri::Result<WebviewWindow> {
    open_window(
        app,
        "VibeMux · DEMO · isolated native QA",
        Some(webview_data_dir),
    )
}

fn open_window(
    app: &AppHandle,
    title: &str,
    webview_data_dir: Option<&Path>,
) -> tauri::Result<WebviewWindow> {
    build_window(
        app,
        "main",
        title,
        "index.html".into(),
        webview_data_dir,
        false,
    )
}

fn build_window<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    title: &str,
    url: PathBuf,
    webview_data_dir: Option<&Path>,
    task: bool,
) -> tauri::Result<WebviewWindow<R>> {
    let dev_url = tauri::is_dev()
        .then(|| app.config().build.dev_url.clone())
        .flatten();
    let allowed_dev_url = dev_url.clone();
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url))
        .title(title)
        .inner_size(
            if task { 980.0 } else { 1280.0 },
            if task { 720.0 } else { 820.0 },
        )
        .min_inner_size(
            if task { 600.0 } else { 900.0 },
            if task { 440.0 } else { 600.0 },
        )
        .on_navigation(move |url| navigation_allowed(url, allowed_dev_url.as_ref()));
    #[cfg(all(windows, feature = "native_qa"))]
    let window = match webview_data_dir {
        Some(directory) => window.data_directory(directory.to_path_buf()),
        None => window,
    };
    #[cfg(not(all(windows, feature = "native_qa")))]
    let _ = webview_data_dir;
    window.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        desktop_settings::DesktopSettings, event_sink::RecordingEventSink,
        project_registry::ProjectRegistry, task_windows::TaskWindowBinding,
    };

    #[test]
    fn mock_window_uses_only_native_binding_data_and_reuses_one_view() {
        let temp = tempfile::tempdir().expect("project");
        let project = ProjectRegistry::default()
            .register(temp.path(), &DesktopSettings::default())
            .expect("project");
        let state = Arc::new(AppState::new(
            temp.path().join("settings.json"),
            Arc::new(RecordingEventSink::default()),
        ));
        let binding = TaskWindowBinding {
            project_key: project.key.clone(),
            project_id: vibemux_types::ProjectId::new(),
            task_id: vibemux_types::TaskId::new(),
            project,
        };
        let app = tauri::test::mock_builder()
            .manage(WindowProfile::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app");
        let reservation = state
            .task_windows
            .reserve(binding.clone())
            .expect("reserve");
        open_task_window(app.handle(), &state, &reservation).expect("create view");
        let window = app
            .get_webview_window(&reservation.label)
            .expect("owned view");
        let url = window.url().expect("app URL");
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query.len(), 4);
        assert_eq!(query.get("view").map(|value| value.as_ref()), Some("task"));
        assert_eq!(
            query.get("project_key").map(|value| value.as_ref()),
            Some(binding.project_key.as_str())
        );
        assert_eq!(
            query.get("project_id").map(|value| value.as_ref()),
            Some(binding.project_id.to_string().as_str())
        );
        assert_eq!(
            query.get("task_id").map(|value| value.as_ref()),
            Some(binding.task_id.to_string().as_str())
        );
        let reused = state.task_windows.reserve(binding).expect("reuse");
        open_task_window(app.handle(), &state, &reused).expect("focus existing");
        assert_eq!(app.webview_windows().len(), 1);
        // The mock does not emit a native Destroyed event; the registry's
        // generation-aware close behavior is checked independently.
    }
}
