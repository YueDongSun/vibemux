//! The build script.
//!
//! It checks that the Svelte UI is built, because the bundle loads only
//! bundled assets. It draws the window icon when the committed assets are
//! absent. And it declares the command list in the Tauri app manifest, so
//! a command that is not declared there is denied to the webview.

#[path = "src/command_names.rs"]
mod command_names;

#[path = "icon.rs"]
mod icon;

fn main() {
    let ui_index = std::path::Path::new("ui/dist/index.html");
    assert!(
        ui_index.is_file(),
        "vibemux_desktop_frontend_missing: run npm ci && npm run build in crates/vibemux_desktop/ui before cargo"
    );

    let icons = std::path::Path::new("icons");
    if !icons.join("icon.ico").is_file() || !icons.join("icon.png").is_file() {
        icon::write_icons(icons).expect("failed to draw the desktop app icons");
    }

    let attributes = tauri_build::Attributes::new()
        .windows_attributes(
            tauri_build::WindowsAttributes::new().window_icon_path(icons.join("icon.ico")),
        )
        .app_manifest(tauri_build::AppManifest::new().commands(&command_names::COMMAND_NAMES));
    tauri_build::try_build(attributes).expect("tauri-build failed");
}
