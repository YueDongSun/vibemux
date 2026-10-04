#![forbid(unsafe_code)]
//! WCAG 2.x relative luminance and contrast math on `(r, g, b)` triples.
//! Pure functions; the GUI converts results to `egui::Color32`.

/// WCAG AA minimum contrast for normal-size text.
pub const TEXT_CONTRAST_MINIMUM: f32 = 4.5;
/// Relative luminance above which a background counts as light.
pub const LIGHT_BACKGROUND_LUMINANCE: f32 = 0.5;
/// Blend step used when deriving a readable text color.
const BLEND_STEP: f32 = 0.05;

pub type Rgb = (u8, u8, u8);

fn channel_to_linear(channel: u8) -> f32 {
    let value = f32::from(channel) / 255.0;
    if value <= 0.039_28 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[must_use]
pub fn relative_luminance(color: Rgb) -> f32 {
    0.2126 * channel_to_linear(color.0)
        + 0.7152 * channel_to_linear(color.1)
        + 0.0722 * channel_to_linear(color.2)
}

#[must_use]
pub fn contrast_ratio(first: Rgb, second: Rgb) -> f32 {
    let first = relative_luminance(first);
    let second = relative_luminance(second);
    let (lighter, darker) = if first >= second {
        (first, second)
    } else {
        (second, first)
    };
    (lighter + 0.05) / (darker + 0.05)
}

#[must_use]
pub fn blend(from: Rgb, toward: Rgb, amount: f32) -> Rgb {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    (
        mix(from.0, toward.0),
        mix(from.1, toward.1),
        mix(from.2, toward.2),
    )
}

/// Return `color` unchanged when it already reaches `minimum` on every
/// background; otherwise blend it toward `toward` (the palette's primary
/// text color, which always passes) in fixed steps until it does.
#[must_use]
pub fn readable_text_color(color: Rgb, backgrounds: &[Rgb], toward: Rgb, minimum: f32) -> Rgb {
    let passes = |candidate: Rgb| {
        backgrounds
            .iter()
            .all(|background| contrast_ratio(candidate, *background) >= minimum)
    };
    let mut amount = 0.0_f32;
    loop {
        let candidate = blend(color, toward, amount);
        if passes(candidate) || amount >= 1.0 {
            return candidate;
        }
        amount = (amount + BLEND_STEP).min(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgb = (0, 0, 0);
    const WHITE: Rgb = (255, 255, 255);

    #[test]
    fn luminance_spans_zero_to_one() {
        assert!(relative_luminance(BLACK).abs() < 1e-6);
        assert!((relative_luminance(WHITE) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn contrast_is_symmetric_and_bounded() {
        assert!((contrast_ratio(BLACK, WHITE) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(WHITE, BLACK) - 21.0).abs() < 0.01);
        assert!((contrast_ratio((201, 100, 66), (201, 100, 66)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn blend_hits_both_endpoints() {
        assert_eq!(blend((10, 20, 30), (110, 120, 130), 0.0), (10, 20, 30));
        assert_eq!(blend((10, 20, 30), (110, 120, 130), 1.0), (110, 120, 130));
        assert_eq!(blend((0, 0, 0), (100, 100, 100), 0.5), (50, 50, 50));
    }

    #[test]
    fn passing_color_is_returned_unchanged() {
        assert_eq!(
            readable_text_color(WHITE, &[BLACK], (200, 200, 200), TEXT_CONTRAST_MINIMUM),
            WHITE
        );
    }

    #[test]
    fn failing_accent_is_pulled_toward_text_until_readable() {
        let terracotta = (0xC9, 0x64, 0x42);
        let backgrounds = [(0xFA, 0xF9, 0xF5), (0xF5, 0xF4, 0xED), (0xFF, 0xFF, 0xFF)];
        let text = (0x14, 0x14, 0x13);
        let readable = readable_text_color(terracotta, &backgrounds, text, TEXT_CONTRAST_MINIMUM);
        assert_ne!(readable, terracotta);
        for background in backgrounds {
            assert!(contrast_ratio(readable, background) >= TEXT_CONTRAST_MINIMUM);
        }
    }
}
