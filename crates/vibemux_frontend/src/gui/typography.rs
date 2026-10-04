#![forbid(unsafe_code)]
//! System font loading: Segoe UI for the interface, a serif family for
//! display text, and a CJK fallback. Fonts are read once from the Windows
//! font directory; nothing is bundled (ADR 030 §3).

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId};

pub const SERIF_FAMILY_NAME: &str = "vibemux_serif";
const SANS_FONT_KEY: &str = "vibemux_ui";
const SERIF_FONT_KEY: &str = "vibemux_serif_face";
const CJK_FONT_KEY: &str = "vibemux_system_cjk";
const SANS_CANDIDATES: [&str; 1] = ["segoeui.ttf"];
const SERIF_CANDIDATES: [&str; 2] = ["georgia.ttf", "cambria.ttc"];
const CJK_CANDIDATES: [&str; 4] = ["msyh.ttc", "simhei.ttf", "meiryo.ttc", "YuGothM.ttc"];

pub const GREETING_SIZE: f32 = 30.0;
pub const TITLE_SIZE: f32 = 22.0;
pub const TASK_TITLE_SIZE: f32 = 17.0;
pub const PROSE_SIZE: f32 = 16.0;
pub const PROSE_LINE_HEIGHT: f32 = 24.0;

#[must_use]
pub fn serif_family() -> FontFamily {
    FontFamily::Name(SERIF_FAMILY_NAME.into())
}

/// The serif display font, or the sans font while the serif family is not yet
/// bound (egui panics on an unbound family, and `set_fonts` takes effect on
/// the next pass).
#[must_use]
pub fn display_font(ctx: &egui::Context, size: f32) -> FontId {
    let family = serif_family();
    if ctx.fonts(|fonts| fonts.families().contains(&family)) {
        FontId::new(size, family)
    } else {
        FontId::proportional(size)
    }
}

/// Build font definitions from whichever candidate faces `read_font` returns.
/// Each face is optional; the serif family always exists and ends with the
/// proportional chain, so display text never loses glyphs.
#[must_use]
pub fn build_font_definitions(
    mut read_font: impl FnMut(&str) -> Option<Vec<u8>>,
) -> FontDefinitions {
    let mut first_found = |candidates: &[&str]| candidates.iter().find_map(|name| read_font(name));
    let sans = first_found(&SANS_CANDIDATES);
    let serif = first_found(&SERIF_CANDIDATES);
    let cjk = first_found(&CJK_CANDIDATES);

    let mut definitions = FontDefinitions::default();
    if let Some(bytes) = sans {
        definitions
            .font_data
            .insert(SANS_FONT_KEY.into(), FontData::from_owned(bytes).into());
        definitions
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, SANS_FONT_KEY.into());
    }
    if let Some(bytes) = cjk {
        definitions
            .font_data
            .insert(CJK_FONT_KEY.into(), FontData::from_owned(bytes).into());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let chain = definitions.families.entry(family).or_default();
            let position = chain.len().min(1);
            chain.insert(position, CJK_FONT_KEY.into());
        }
    }
    let mut serif_chain = Vec::new();
    if let Some(bytes) = serif {
        definitions
            .font_data
            .insert(SERIF_FONT_KEY.into(), FontData::from_owned(bytes).into());
        serif_chain.push(SERIF_FONT_KEY.to_string());
    }
    serif_chain.extend(
        definitions
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    definitions.families.insert(serif_family(), serif_chain);
    definitions
}

#[cfg(windows)]
pub fn install_fonts(ctx: &egui::Context) {
    use std::{env, fs, path::PathBuf};
    let fonts_dir = env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    ctx.set_fonts(build_font_definitions(|name| {
        fs::read(fonts_dir.join(name)).ok()
    }));
}

#[cfg(not(windows))]
pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(build_font_definitions(|_| None));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_fonts(available: &'static [&'static str]) -> impl FnMut(&str) -> Option<Vec<u8>> {
        move |name| available.contains(&name).then(|| name.as_bytes().to_vec())
    }

    #[test]
    fn every_face_found_builds_sans_serif_and_cjk_chains() {
        let definitions =
            build_font_definitions(fake_fonts(&["segoeui.ttf", "georgia.ttf", "msyh.ttc"]));
        let proportional = &definitions.families[&egui::FontFamily::Proportional];
        assert_eq!(proportional[0], "vibemux_ui");
        assert_eq!(proportional[1], "vibemux_system_cjk");
        assert_eq!(
            definitions.families[&egui::FontFamily::Monospace][1],
            "vibemux_system_cjk"
        );
        let serif = &definitions.families[&serif_family()];
        assert_eq!(serif[0], "vibemux_serif_face");
        assert!(serif.iter().any(|face| face == "vibemux_system_cjk"));
    }

    #[test]
    fn cambria_is_the_serif_fallback() {
        let definitions = build_font_definitions(fake_fonts(&["cambria.ttc"]));
        assert!(definitions.font_data.contains_key("vibemux_serif_face"));
    }

    #[test]
    fn missing_faces_still_bind_the_serif_family_to_the_sans_chain() {
        let definitions = build_font_definitions(fake_fonts(&[]));
        assert_eq!(
            definitions.families[&serif_family()],
            definitions.families[&egui::FontFamily::Proportional]
        );
    }

    #[test]
    fn display_font_falls_back_until_the_serif_family_is_bound() {
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            assert_eq!(
                display_font(context, 20.0).family,
                egui::FontFamily::Proportional
            );
        });
        context.set_fonts(build_font_definitions(fake_fonts(&[])));
        let _ = context.run(egui::RawInput::default(), |context| {
            assert_eq!(display_font(context, 20.0).family, serif_family());
        });
    }
}
