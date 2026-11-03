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
    println!("cargo:rerun-if-changed=ui/dist");
    // Every linked executable, including the library's unit-test harness, can
    // import TaskDialogIndirect through Wry. It requires Common Controls v6.
    // The tests-only Cargo directive does not cover the library test binary.
    // Keep one manifest producer: the linker embeds it for every target,
    // while Tauri still supplies the icon resources below.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
    let ui_index = std::path::Path::new("ui/dist/index.html");
    assert!(
        ui_index.is_file(),
        "vibemux_desktop_frontend_missing: run npm ci && npm run build in crates/vibemux_desktop/ui before cargo"
    );

    let icons = std::path::Path::new("icons");
    icon::write_icons(icons).expect("failed to prepare the desktop app icons");

    let attributes = tauri_build::Attributes::new()
        .windows_attributes(
            tauri_build::WindowsAttributes::new_without_app_manifest()
                .window_icon_path(icons.join("icon.ico")),
        )
        .app_manifest(tauri_build::AppManifest::new().commands(&command_names::COMMAND_NAMES));
    tauri_build::try_build(attributes).expect("tauri-build failed");
}
