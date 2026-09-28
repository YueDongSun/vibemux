#![forbid(unsafe_code)]
//! Welcome state: the spark mark and a time-of-day serif greeting above the
//! composer. It never shows a user name (ADR 030 §4).

use eframe::egui::{RichText, Ui};

use super::{C, design, typography};

pub const MORNING_START_HOUR: u8 = 5;
pub const AFTERNOON_START_HOUR: u8 = 12;
pub const EVENING_START_HOUR: u8 = 18;

#[must_use]
pub fn greeting_for_hour(local_hour: Option<u8>) -> &'static str {
    match local_hour {
        Some(hour) if (MORNING_START_HOUR..AFTERNOON_START_HOUR).contains(&hour) => "Good morning",
        Some(hour) if (AFTERNOON_START_HOUR..EVENING_START_HOUR).contains(&hour) => {
            "Good afternoon"
        }
        Some(hour) if hour < 24 => "Good evening",
        _ => "Hello",
    }
}

/// The local hour, or `None` when the local offset is unavailable.
#[must_use]
pub fn current_local_hour() -> Option<u8> {
    time::OffsetDateTime::now_local().ok().map(|now| now.hour())
}

pub fn render_heading(ui: &mut Ui, c: &C, greeting: &str) {
    ui.vertical_centered(|ui| {
        design::spark(ui, c.accent, 34.0, 0.0);
        ui.add_space(10.0);
        ui.label(
            RichText::new(greeting)
                .font(typography::display_font(
                    ui.ctx(),
                    typography::GREETING_SIZE,
                ))
                .color(c.txt),
        );
        ui.add_space(22.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_changes_at_five_twelve_and_eighteen() {
        assert_eq!(greeting_for_hour(Some(4)), "Good evening");
        assert_eq!(greeting_for_hour(Some(5)), "Good morning");
        assert_eq!(greeting_for_hour(Some(11)), "Good morning");
        assert_eq!(greeting_for_hour(Some(12)), "Good afternoon");
        assert_eq!(greeting_for_hour(Some(17)), "Good afternoon");
        assert_eq!(greeting_for_hour(Some(18)), "Good evening");
        assert_eq!(greeting_for_hour(Some(23)), "Good evening");
        assert_eq!(greeting_for_hour(Some(0)), "Good evening");
    }

    #[test]
    fn unknown_or_invalid_hour_says_hello() {
        assert_eq!(greeting_for_hour(None), "Hello");
        assert_eq!(greeting_for_hour(Some(24)), "Hello");
    }
}
