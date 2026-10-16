//! Shared keyboard navigation table and its discoverable help overlay.
use egui::{self, Key, RichText};

use super::{C, typography};
use crate::quick_search::NavigationAction;

struct Shortcut {
    key: Key,
    chord: &'static str,
    label: &'static str,
    action: NavigationAction,
    while_typing: bool,
}

const SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        key: Key::K,
        chord: "Ctrl+K",
        label: "Find a task, agent or action",
        action: NavigationAction::Search,
        while_typing: true,
    },
    Shortcut {
        key: Key::N,
        chord: "Ctrl+N",
        label: "New task draft",
        action: NavigationAction::NewTask,
        while_typing: true,
    },
    Shortcut {
        key: Key::B,
        chord: "Ctrl+B",
        label: "Toggle sidebar",
        action: NavigationAction::ToggleSidebar,
        while_typing: true,
    },
    Shortcut {
        key: Key::Comma,
        chord: "Ctrl+,",
        label: "Settings",
        action: NavigationAction::Settings,
        while_typing: true,
    },
    Shortcut {
        key: Key::Slash,
        chord: "Ctrl+/",
        label: "Keyboard shortcuts",
        action: NavigationAction::Shortcuts,
        while_typing: true,
    },
    Shortcut {
        key: Key::Num1,
        chord: "Ctrl+1",
        label: "Agent 1",
        action: NavigationAction::Agent(0),
        while_typing: false,
    },
    Shortcut {
        key: Key::Num2,
        chord: "Ctrl+2",
        label: "Agent 2",
        action: NavigationAction::Agent(1),
        while_typing: false,
    },
    Shortcut {
        key: Key::Num3,
        chord: "Ctrl+3",
        label: "Agent 3",
        action: NavigationAction::Agent(2),
        while_typing: false,
    },
    Shortcut {
        key: Key::Num4,
        chord: "Ctrl+4",
        label: "Agent 4",
        action: NavigationAction::Agent(3),
        while_typing: false,
    },
    Shortcut {
        key: Key::Num5,
        chord: "Ctrl+5",
        label: "Agent 5",
        action: NavigationAction::Agent(4),
        while_typing: false,
    },
    Shortcut {
        key: Key::Num6,
        chord: "Ctrl+6",
        label: "Agent 6",
        action: NavigationAction::Agent(5),
        while_typing: false,
    },
    Shortcut {
        key: Key::Num7,
        chord: "Ctrl+7",
        label: "Agent 7",
        action: NavigationAction::Agent(6),
        while_typing: false,
    },
    Shortcut {
        key: Key::Num8,
        chord: "Ctrl+8",
        label: "Agent 8",
        action: NavigationAction::Agent(7),
        while_typing: false,
    },
    Shortcut {
        key: Key::Num9,
        chord: "Ctrl+9",
        label: "Agent 9",
        action: NavigationAction::Agent(8),
        while_typing: false,
    },
    Shortcut {
        key: Key::Num0,
        chord: "Ctrl+0",
        label: "Agent 10",
        action: NavigationAction::Agent(9),
        while_typing: false,
    },
];

pub(super) fn consume(ctx: &egui::Context) -> Option<NavigationAction> {
    let typing = ctx.wants_keyboard_input();
    let modifiers = ctx.input(|input| input.modifiers);
    // Native Windows events set both ctrl and command. Comparing against the
    // CTRL constant alone rejects that real input combination.
    if modifiers.alt || modifiers.shift || !(modifiers.ctrl || modifiers.command) {
        return None;
    }
    SHORTCUTS.iter().find_map(|shortcut| {
        if (!typing || shortcut.while_typing)
            && ctx.input_mut(|input| input.consume_key(modifiers, shortcut.key))
        {
            Some(shortcut.action.clone())
        } else {
            None
        }
    })
}

pub(super) fn render(
    ctx: &egui::Context,
    colors: &C,
    view_model: &crate::view_model::ViewModel,
) -> bool {
    let modal = egui::Modal::new(egui::Id::new("keyboard_shortcuts")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.label(
            RichText::new(colors.tr("Keyboard shortcuts"))
                .font(typography::region_font(
                    ui,
                    colors.language,
                    typography::TITLE_SIZE,
                ))
                .color(colors.txt),
        );
        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .id_salt("shortcut_list")
            .max_height((ctx.screen_rect().height() - 180.0).max(120.0))
            .show(ui, |ui| {
                egui::Grid::new("shortcut_table")
                    .spacing([24.0, 8.0])
                    .show(ui, |ui| {
                        for shortcut in SHORTCUTS {
                            ui.label(
                                RichText::new(shortcut.chord)
                                    .monospace()
                                    .size(12.0)
                                    .color(colors.accent_text),
                            );
                            let label = match &shortcut.action {
                                NavigationAction::Agent(index) => view_model
                                    .agents
                                    .get(*index)
                                    .map(|agent| {
                                        colors.format("Inspect {agent}", &[("agent", &agent.name)])
                                    })
                                    .unwrap_or_else(|| colors.tr(shortcut.label).to_string()),
                                _ => colors.tr(shortcut.label).to_string(),
                            };
                            ui.label(RichText::new(label).size(13.0).color(colors.txt));
                            ui.end_row();
                        }
                    });
            });
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                colors.tr("Esc closes the current overlay. Agent shortcuts pause while typing."),
            )
            .size(12.0)
            .color(colors.muted),
        );
        ui.add_space(10.0);
        ui.button(colors.tr("Done")).clicked()
    });
    modal.inner || modal.should_close()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_chords_are_unique() {
        for (index, shortcut) in SHORTCUTS.iter().enumerate() {
            assert!(
                !SHORTCUTS[..index]
                    .iter()
                    .any(|other| other.key == shortcut.key)
            );
        }
    }

    #[test]
    fn native_control_and_command_flags_work_without_accepting_alt_or_shift() {
        for (modifiers, accepted) in [
            (
                egui::Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
                true,
            ),
            (
                egui::Modifiers {
                    mac_cmd: true,
                    command: true,
                    ..Default::default()
                },
                true,
            ),
            (
                egui::Modifiers {
                    ctrl: true,
                    command: true,
                    alt: true,
                    ..Default::default()
                },
                false,
            ),
            (
                egui::Modifiers {
                    ctrl: true,
                    command: true,
                    shift: true,
                    ..Default::default()
                },
                false,
            ),
            (egui::Modifiers::NONE, false),
        ] {
            let context = egui::Context::default();
            let _ = context.run(
                egui::RawInput {
                    modifiers,
                    events: vec![egui::Event::Key {
                        key: Key::K,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    assert_eq!(consume(ctx), accepted.then_some(NavigationAction::Search));
                    assert_eq!(ctx.input(|input| input.key_pressed(Key::K)), !accepted);
                },
            );
        }
    }
}
