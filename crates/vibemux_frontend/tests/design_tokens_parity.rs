#![forbid(unsafe_code)]
//! Parity and contrast pins for `config/design_tokens.css`.
//!
//! `config/design_tokens.css` is the shared stylesheet the mobile web
//! surface (`crates/vibemux_web/web/src/style.css`) and a future Tauri
//! desktop UI both load. This test keeps its light block in lockstep
//! with the compiled-in `claude_light` palette, its two dark blocks in
//! lockstep with each other and with the compiled-in `claude` palette
//! (both exported to `config/theme_palettes.json`, pinned separately by
//! `palette_parity.rs`), and every semantic text/background pair at or
//! above WCAG AA (4.5:1).
//!
//! Parsing is plain string splitting on purpose (no regex, no new
//! dependency): the CSS here is a small, fully controlled subset
//! (`--name: value;` declarations inside three flat, non-nested
//! blocks), so `find`/`split_once`/`split(';')` are enough.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use vibemux_frontend::theme::contrast::{TEXT_CONTRAST_MINIMUM, contrast_ratio};
use vibemux_frontend::theme::parse_hex_rgb;

const TOKENS: &str = include_str!("../../../config/design_tokens.css");
const PALETTES: &str = include_str!("../../../config/theme_palettes.json");
/// The web surface's own stylesheet, which must import `TOKENS` rather
/// than redeclare any of its custom properties.
const STYLE_CSS: &str = include_str!("../../../crates/vibemux_web/web/src/style.css");

/// The light block's selector is the bare `:root {`; neither dark
/// selector contains that exact substring (they continue with `:not(`
/// or `[data-theme=`), so a plain substring search is unambiguous.
const LIGHT_SELECTOR: &str = ":root {";
/// `@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { … } }`.
const DARK_MEDIA_SELECTOR: &str = ":root:not([data-theme=\"light\"]) {";
/// `:root[data-theme="dark"] { … }`.
const DARK_EXPLICIT_SELECTOR: &str = ":root[data-theme=\"dark\"] {";

/// Return the declaration text between `selector`'s own trailing `{`
/// (each selector constant already ends in `" {"`) and its matching
/// `}`. The three token blocks never nest braces, so the first `}`
/// found is always the right one.
fn block_body<'a>(css: &'a str, selector: &str) -> &'a str {
    let start = css
        .find(selector)
        .unwrap_or_else(|| panic!("selector {selector:?} not found in config/design_tokens.css"));
    let body = &css[start + selector.len()..];
    let close = body
        .find('}')
        .unwrap_or_else(|| panic!("no closing '}}' for selector {selector:?}"));
    &body[..close]
}

/// Parse `name: value;` declarations (including `color-scheme` and
/// every `--token`) into a name -> value map.
fn parse_declarations(body: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for raw_declaration in body.split(';') {
        let declaration = raw_declaration.trim();
        if declaration.is_empty() {
            continue;
        }
        let (name, value) = declaration
            .split_once(':')
            .unwrap_or_else(|| panic!("malformed declaration {declaration:?}"));
        map.insert(name.trim().to_string(), value.trim().to_string());
    }
    map
}

/// Parse every `--token: value;` custom property, keyed by the token
/// name with the leading `--` stripped. Despite the name, values are
/// not validated as hex here (the non-color `font_`/`size_`/`radius_`
/// tokens pass through too); callers that need `#RRGGBB` enforcement
/// use `parse_hex_rgb` themselves.
fn parse_hex_tokens(body: &str) -> BTreeMap<String, String> {
    parse_declarations(body)
        .into_iter()
        .filter_map(|(name, value)| {
            name.strip_prefix("--")
                .map(|bare_name| (bare_name.to_string(), value))
        })
        .collect()
}

fn exported_palette<'a>(palettes: &'a Value, name: &str) -> &'a Value {
    palettes["palettes"]
        .as_array()
        .expect("config/theme_palettes.json must have a palettes array")
        .iter()
        .find(|palette| palette["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("palette {name:?} missing from config/theme_palettes.json"))
}

fn assert_hex_equal(
    block_name: &str,
    css_token: &str,
    css_hex: &str,
    json_field: &str,
    json_hex: &str,
) {
    let css_rgb = parse_hex_rgb(css_hex)
        .unwrap_or_else(|| panic!("{block_name} --{css_token} is not #RRGGBB: {css_hex:?}"));
    let json_rgb = parse_hex_rgb(json_hex).unwrap_or_else(|| {
        panic!("{block_name} palette field {json_field} is not #RRGGBB: {json_hex:?}")
    });
    assert_eq!(
        css_rgb, json_rgb,
        "{block_name} block: --{css_token} ({css_hex}) does not match exported palette field {json_field} ({json_hex})"
    );
}

#[test]
fn base_tokens_match_the_exported_palettes() {
    let palettes: Value =
        serde_json::from_str(PALETTES).expect("config/theme_palettes.json must be valid JSON");

    // (css block name, block body selector, exported palette name)
    let blocks = [
        ("light", LIGHT_SELECTOR, "claude_light"),
        ("dark (media)", DARK_MEDIA_SELECTOR, "claude"),
        ("dark (explicit)", DARK_EXPLICIT_SELECTOR, "claude"),
    ];

    // (css token name, exported palette field name)
    let direct_pairs = [
        ("bg", "bg"),
        ("surface", "surface"),
        ("surface_alt", "surface_alt"),
        ("border", "border"),
        ("text_primary", "text_primary"),
        ("accent", "accent"),
        ("success", "success"),
        ("warning", "warning"),
        ("danger", "danger"),
    ];

    for (block_name, selector, palette_name) in blocks {
        let tokens = parse_hex_tokens(block_body(TOKENS, selector));
        let palette = exported_palette(&palettes, palette_name);

        for (css_token, json_field) in direct_pairs {
            let css_hex = tokens
                .get(css_token)
                .unwrap_or_else(|| panic!("{block_name} block missing --{css_token}"));
            let json_hex = palette[json_field]
                .as_str()
                .unwrap_or_else(|| panic!("palette {palette_name} missing field {json_field}"));
            assert_hex_equal(block_name, css_token, css_hex, json_field, json_hex);
        }

        // --muted_text (CSS) corresponds to text_muted (exported palette).
        let muted_css_hex = tokens
            .get("muted_text")
            .unwrap_or_else(|| panic!("{block_name} block missing --muted_text"));
        let muted_json_hex = palette["text_muted"]
            .as_str()
            .unwrap_or_else(|| panic!("palette {palette_name} missing field text_muted"));
        assert_hex_equal(
            block_name,
            "muted_text",
            muted_css_hex,
            "text_muted",
            muted_json_hex,
        );
    }
}

#[test]
fn the_two_dark_blocks_are_identical() {
    let media_block = parse_declarations(block_body(TOKENS, DARK_MEDIA_SELECTOR));
    let explicit_block = parse_declarations(block_body(TOKENS, DARK_EXPLICIT_SELECTOR));
    assert_eq!(
        media_block, explicit_block,
        "the `@media (prefers-color-scheme: dark)` block and the `:root[data-theme=\"dark\"]` block must declare identical name -> value pairs"
    );
}

#[test]
fn every_text_pair_meets_wcag_aa() {
    let light_tokens = parse_hex_tokens(block_body(TOKENS, LIGHT_SELECTOR));
    // The two dark blocks are pinned identical by `the_two_dark_blocks_are_identical`;
    // checking one is sufficient for contrast coverage.
    let dark_tokens = parse_hex_tokens(block_body(TOKENS, DARK_MEDIA_SELECTOR));

    let assert_contrast = |block_name: &str,
                           tokens: &BTreeMap<String, String>,
                           fg_name: &str,
                           bg_name: &str| {
        let fg_hex = tokens
            .get(fg_name)
            .unwrap_or_else(|| panic!("{block_name} block missing --{fg_name}"));
        let bg_hex = tokens
            .get(bg_name)
            .unwrap_or_else(|| panic!("{block_name} block missing --{bg_name}"));
        let fg = parse_hex_rgb(fg_hex)
            .unwrap_or_else(|| panic!("{block_name} --{fg_name} is not #RRGGBB: {fg_hex:?}"));
        let bg = parse_hex_rgb(bg_hex)
            .unwrap_or_else(|| panic!("{block_name} --{bg_name} is not #RRGGBB: {bg_hex:?}"));
        let ratio = contrast_ratio(fg, bg);
        assert!(
            ratio >= TEXT_CONTRAST_MINIMUM,
            "{block_name} block: --{fg_name}/--{bg_name} contrast ratio {ratio:.2} is below WCAG AA {TEXT_CONTRAST_MINIMUM}"
        );
    };

    for (block_name, tokens) in [("light", &light_tokens), ("dark", &dark_tokens)] {
        for text_name in ["text_primary", "muted_text", "accent_text"] {
            for bg_name in ["bg", "surface", "surface_alt"] {
                assert_contrast(block_name, tokens, text_name, bg_name);
            }
        }
        assert_contrast(block_name, tokens, "on_accent", "accent");
        assert_contrast(block_name, tokens, "success_text", "success_tint");
        assert_contrast(block_name, tokens, "warning_text", "warning_tint");
        assert_contrast(block_name, tokens, "danger_text", "danger_tint");
        for text_name in ["success_text", "warning_text", "danger_text"] {
            assert_contrast(block_name, tokens, text_name, "bg");
        }
        assert_contrast(block_name, tokens, "accent_text", "accent_tint");
    }
}

/// A token name counts as a color token unless it is one of the
/// non-color families (`font_*`, `size_*`, `radius_*`), which only
/// ever live in the light block.
fn is_non_color_token(name: &str) -> bool {
    name.starts_with("font_") || name.starts_with("size_") || name.starts_with("radius_")
}

#[test]
fn every_color_token_has_a_dark_value() {
    let light_tokens = parse_hex_tokens(block_body(TOKENS, LIGHT_SELECTOR));
    let dark_media_tokens = parse_hex_tokens(block_body(TOKENS, DARK_MEDIA_SELECTOR));
    let dark_explicit_tokens = parse_hex_tokens(block_body(TOKENS, DARK_EXPLICIT_SELECTOR));

    let light_color_names: BTreeSet<&String> = light_tokens
        .keys()
        .filter(|name| !is_non_color_token(name))
        .collect();

    for (block_name, dark_tokens) in [
        ("dark (media)", &dark_media_tokens),
        ("dark (explicit)", &dark_explicit_tokens),
    ] {
        let dark_names: BTreeSet<&String> = dark_tokens.keys().collect();

        let missing_in_dark: Vec<&&String> = light_color_names.difference(&dark_names).collect();
        assert!(
            missing_in_dark.is_empty(),
            "{block_name} block is missing a dark value for light color token(s): {missing_in_dark:?}"
        );

        let extra_in_dark: Vec<&&String> = dark_names.difference(&light_color_names).collect();
        assert!(
            extra_in_dark.is_empty(),
            "{block_name} block declares token(s) absent from the light block's color tokens: {extra_in_dark:?}"
        );

        let non_color_in_dark: Vec<&String> = dark_tokens
            .keys()
            .filter(|name| is_non_color_token(name))
            .collect();
        assert!(
            non_color_in_dark.is_empty(),
            "{block_name} block must not declare font_/size_/radius_ token(s): {non_color_in_dark:?}"
        );
    }

    for (block_name, tokens) in [
        ("light", &light_tokens),
        ("dark (media)", &dark_media_tokens),
        ("dark (explicit)", &dark_explicit_tokens),
    ] {
        let non_hex: Vec<(&String, &String)> = tokens
            .iter()
            .filter(|(name, _)| !is_non_color_token(name))
            .filter(|(_, value)| parse_hex_rgb(value).is_none())
            .collect();
        assert!(
            non_hex.is_empty(),
            "{block_name} block has color token(s) that are not #RRGGBB: {non_hex:?}"
        );
    }
}

/// Find the first `--name:` custom-property declaration in `css`, if
/// any, without regex: scan for `--`, then take the following
/// alphanumeric/underscore run as the candidate name, and accept it
/// only if a `:` immediately follows.
fn find_custom_property_declaration(css: &str) -> Option<&str> {
    let mut rest = css;
    let mut consumed = 0usize;
    while let Some(relative_start) = rest.find("--") {
        let after_dashes = &rest[relative_start + 2..];
        let name_len = after_dashes
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        if name_len > 0 && after_dashes.as_bytes().get(name_len) == Some(&b':') {
            let start = consumed + relative_start;
            return Some(&css[start..start + 2 + name_len]);
        }
        let advance = relative_start + 2;
        rest = &rest[advance..];
        consumed += advance;
    }
    None
}

#[test]
fn page_structure_is_pinned() {
    // The light and dark (media) blocks are adjacent; the text between
    // them must be exactly the `@media` wrapper, trailing whitespace
    // aside.
    let dark_media_start = TOKENS.find(DARK_MEDIA_SELECTOR).unwrap_or_else(|| {
        panic!("selector {DARK_MEDIA_SELECTOR:?} not found in config/design_tokens.css")
    });
    let text_before_dark_media = TOKENS[..dark_media_start].trim_end();
    assert!(
        text_before_dark_media.ends_with("@media (prefers-color-scheme: dark) {"),
        "text immediately before the dark media selector must end with the @media wrapper, got: {text_before_dark_media:?}"
    );

    // `color-scheme` must match each block's intent.
    let light_declarations = parse_declarations(block_body(TOKENS, LIGHT_SELECTOR));
    assert_eq!(
        light_declarations.get("color-scheme").map(String::as_str),
        Some("light"),
        "light block color-scheme must be light, got: {:?}",
        light_declarations.get("color-scheme")
    );
    for (block_name, selector) in [
        ("dark (media)", DARK_MEDIA_SELECTOR),
        ("dark (explicit)", DARK_EXPLICIT_SELECTOR),
    ] {
        let declarations = parse_declarations(block_body(TOKENS, selector));
        assert_eq!(
            declarations.get("color-scheme").map(String::as_str),
            Some("dark"),
            "{block_name} block color-scheme must be dark, got: {:?}",
            declarations.get("color-scheme")
        );
    }

    // The light selector must be the only `:root {` block start, so a
    // second light block cannot hide behind the first.
    let light_selector_occurrences = TOKENS.matches(LIGHT_SELECTOR).count();
    assert_eq!(
        light_selector_occurrences, 1,
        "the light selector {LIGHT_SELECTOR:?} must appear exactly once as a block start in config/design_tokens.css, found {light_selector_occurrences} time(s)"
    );

    // `style.css` must import the shared tokens as its first
    // statement and must not redeclare any custom property itself.
    let first_non_empty_line = STYLE_CSS
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_else(|| panic!("crates/vibemux_web/web/src/style.css must not be empty"));
    assert_eq!(
        first_non_empty_line.trim(),
        "@import '../../../../config/design_tokens.css';",
        "crates/vibemux_web/web/src/style.css's first non-empty line must import config/design_tokens.css, got: {:?}",
        first_non_empty_line.trim()
    );
    assert_eq!(
        find_custom_property_declaration(STYLE_CSS),
        None,
        "crates/vibemux_web/web/src/style.css must not declare its own custom property; it should only consume tokens imported from config/design_tokens.css"
    );
}
