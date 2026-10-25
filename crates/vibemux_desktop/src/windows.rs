//! The app's windows.
//!
//! The windows are created in Rust, not from the config, so every one of
//! them gets the navigation guard.

use tauri::{AppHandle, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::navigation::navigation_allowed;

/// Opens the main window: 1280 by 820, and never smaller than 900 by 600.
pub fn open_main_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let dev_url = app.config().build.dev_url.clone();
    let url = match dev_url.as_ref() {
        Some(dev_url) => WebviewUrl::External(dev_url.clone()),
        None => WebviewUrl::App("index.html".into()),
    };
    let allowed_dev_url = dev_url.clone();
    WebviewWindowBuilder::new(app, "main", url)
        .title("VibeMux")
        .inner_size(1280.0, 820.0)
        .min_inner_size(900.0, 600.0)
        .on_navigation(move |url| navigation_allowed(url, allowed_dev_url.as_ref()))
        .build()
}
