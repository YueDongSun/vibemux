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

pub fn render_heading(ui: &mut Ui, c: &C, greeting: &str, subtitle: Option<&str>) {
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
        if let Some(subtitle) = subtitle {
            ui.add_space(8.0);
            ui.label(RichText::new(subtitle).size(13.0).color(c.muted));
        }
        ui.add_space(22.0);
    });
}

/// One honest orientation line for the workspace. Derived from the observed
/// task count, never from a user name or a fabricated promise (ADR 030 §4).
#[must_use]
pub fn workspace_hint(task_count: usize) -> String {
    match task_count {
        0 => "Describe what you want the coordinator to do.".to_string(),
        1 => "1 task in this workspace · open it from Recents or describe a new one.".to_string(),
        count => format!(
            "{count} tasks in this workspace · open one from Recents or describe a new one."
        ),
    }
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

    #[test]
    fn workspace_hint_counts_the_observed_tasks_and_never_invents_a_name() {
        assert!(workspace_hint(0).contains("Describe"));
        assert!(workspace_hint(1).starts_with("1 task"));
        assert!(workspace_hint(4).starts_with("4 tasks"));
        for count in [0, 1, 7] {
            assert!(
                !workspace_hint(count).contains('@'),
                "the hint must not name a person"
            );
        }
    }
}
