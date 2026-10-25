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

use tauri::{AppHandle, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::navigation::navigation_allowed;

/// Opens the main window: 1280 by 820, and never smaller than 900 by 600.
pub fn open_main_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let dev_url = tauri::is_dev()
        .then(|| app.config().build.dev_url.clone())
        .flatten();
    let allowed_dev_url = dev_url.clone();
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("VibeMux")
        .inner_size(1280.0, 820.0)
        .min_inner_size(900.0, 600.0)
        .on_navigation(move |url| navigation_allowed(url, allowed_dev_url.as_ref()))
        .build()
}
