#![forbid(unsafe_code)]
//! Parity pin between the compiled-in canonical GUI palettes and the
//! exported `config/theme_palettes.json`.
//!
//! The exported file is the shared color source consumed outside Rust
//! (Python CLI theming and the WezTerm/tmux theme generators). If a
//! palette changes in `theme/palette.rs`, regenerate the file in the
//! same commit; this test fails otherwise. Array order must follow
//! `ThemeId::ALL`.

use serde_json::Value;
use vibemux_frontend::theme::all_palettes;

const EXPORTED: &str = include_str!("../../../config/theme_palettes.json");

#[test]
fn exported_palette_file_matches_compiled_palettes() {
    let expected = serde_json::json!({
        "schema_version": 1,
        "palettes": all_palettes(),
    });
    let actual: Value =
        serde_json::from_str(EXPORTED).expect("config/theme_palettes.json must be valid JSON");
    assert_eq!(
        actual, expected,
        "config/theme_palettes.json drifted from the compiled-in palettes; regenerate it in the same change"
    );
}

#[test]
fn every_exported_hex_value_is_valid() {
    let actual: Value = serde_json::from_str(EXPORTED).expect("valid JSON");
    let hex_fields = [
        "bg",
        "surface",
        "surface_alt",
        "border",
        "text_primary",
        "text_muted",
        "accent",
        "accent_alt",
        "success",
        "warning",
        "danger",
        "terminal_bg",
        "terminal_fg",
        "terminal_cursor",
    ];
    for palette in actual["palettes"].as_array().expect("palettes array") {
        let name = palette["name"].as_str().expect("palette name");
        for field in hex_fields {
            let hex = palette[field]
                .as_str()
                .unwrap_or_else(|| panic!("palette {name} is missing hex field {field}"));
            assert_eq!(
                hex.len(),
                7,
                "palette {name} field {field} must be #RRGGBB, got {hex}"
            );
            assert_eq!(
                &hex[..1],
                "#",
                "palette {name} field {field} must start with '#'"
            );
        }
    }
}
