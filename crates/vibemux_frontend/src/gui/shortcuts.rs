#![forbid(unsafe_code)]
//! The keyboard shortcut table (ADR 030 §5). One table drives both the key
//! handling and the `Ctrl+/` overlay, so the overlay cannot drift from what
//! the keys do.

use eframe::egui::{self, Key, RichText};

use super::C;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShortcutAction {
    QuickSwitcher,
    NewTask,
    ToggleSidebar,
    Settings,
    Overlay,
}

#[derive(Clone, Copy, Debug)]
pub struct Shortcut {
    /// Pressed with `Ctrl` (`Cmd` on macOS).
    pub key: Key,
    pub keys_label: &'static str,
    pub description: &'static str,
    pub action: ShortcutAction,
}

pub const SHORTCUTS: [Shortcut; 5] = [
    Shortcut {
        key: Key::K,
        keys_label: "Ctrl+K",
        description: "Quick switcher",
        action: ShortcutAction::QuickSwitcher,
    },
    Shortcut {
        key: Key::N,
        keys_label: "Ctrl+N",
        description: "New task",
        action: ShortcutAction::NewTask,
    },
    Shortcut {
        key: Key::B,
        keys_label: "Ctrl+B",
        description: "Toggle sidebar",
        action: ShortcutAction::ToggleSidebar,
    },
    Shortcut {
        key: Key::Comma,
        keys_label: "Ctrl+,",
        description: "Settings",
        action: ShortcutAction::Settings,
    },
    Shortcut {
        key: Key::Slash,
        keys_label: "Ctrl+/",
        description: "Keyboard shortcuts",
        action: ShortcutAction::Overlay,
    },
];

/// Existing keys the overlay also lists; their handlers keep their own rule
/// of not firing while a text field has focus.
pub const OTHER_KEYS: [(&str, &str); 2] = [
    ("Esc", "Close the top-most overlay"),
    ("Ctrl+1 to Ctrl+0", "Select an agent"),
];

/// Removes the first table chord from this frame's events, before any text
/// widget sees it, and returns its action. The caller skips this during IME
/// composition.
pub fn consume(ctx: &egui::Context) -> Option<ShortcutAction> {
    ctx.input_mut(|input| {
        let position = input.events.iter().position(|event| {
            matches!(
                event,
                egui::Event::Key { key, pressed: true, modifiers, .. }
                    if (modifiers.ctrl || modifiers.command)
                        && !modifiers.alt
                        && !modifiers.shift
                        && SHORTCUTS.iter().any(|shortcut| shortcut.key == *key)
            )
        })?;
        let egui::Event::Key { key, .. } = input.events.remove(position) else {
            return None;
        };
        SHORTCUTS
            .iter()
            .find(|shortcut| shortcut.key == key)
            .map(|shortcut| shortcut.action)
    })
}

pub fn render_overlay(ctx: &egui::Context, colors: &C, open: &mut bool) {
    egui::Window::new("Keyboard shortcuts")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        .show(ctx, |ui| {
            egui::Grid::new("keyboard_shortcuts_grid")
                .num_columns(2)
                .spacing([18.0, 8.0])
                .show(ui, |ui| {
                    let rows = SHORTCUTS
                        .iter()
                        .map(|shortcut| (shortcut.keys_label, shortcut.description))
                        .chain(OTHER_KEYS);
                    for (keys, description) in rows {
                        ui.label(RichText::new(keys).monospace().color(colors.accent_text));
                        ui.label(RichText::new(description).color(colors.txt));
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            ui.label(
                RichText::new("Shortcuts change only what this window shows.")
                    .size(12.0)
                    .color(colors.muted),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_table_has_no_duplicate_chords() {
        for (index, shortcut) in SHORTCUTS.iter().enumerate() {
            assert!(
                SHORTCUTS[index + 1..]
                    .iter()
                    .all(|other| other.key != shortcut.key && other.action != shortcut.action),
                "{} repeats",
                shortcut.keys_label
            );
            // The existing agent chords own Ctrl+digit.
            assert!(!shortcut.keys_label.contains(char::is_numeric));
        }
    }
}
