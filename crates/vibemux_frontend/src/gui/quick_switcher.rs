//! Searchable local navigation with one bounded, keyboard-accessible result list.
use egui::{self, Key, Modifiers, RichText};

use super::{C, design, typography};
use crate::quick_search::{self, NavigationAction, SearchEntry};

pub(super) const SEARCH_INPUT_ID: &str = "quick_switcher_query";
const SEARCH_WIDTH: f32 = 560.0;
const RESULT_HEIGHT: f32 = 48.0;
/// Space for the heading, query, footer, partial-list note and outer margins.
const RESULTS_RESERVED_HEIGHT: f32 = 264.0;
const QUERY_LIMIT: usize = 256;

#[derive(Default)]
pub(super) struct QuickSwitcher {
    pub open: bool,
    query: String,
    selected: usize,
    focus_requested: bool,
    previous_focus: Option<egui::Id>,
}

impl QuickSwitcher {
    pub fn show(&mut self, ctx: &egui::Context) {
        self.previous_focus = ctx.memory(|memory| memory.focused());
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.focus_requested = true;
        // Transfer focus before the background composer handles this frame.
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new(SEARCH_INPUT_ID)));
    }

    pub fn close(&mut self, ctx: &egui::Context) {
        self.open = false;
        ctx.memory_mut(|memory| {
            memory.surrender_focus(egui::Id::new(SEARCH_INPUT_ID));
            if let Some(previous) = self.previous_focus.take() {
                memory.request_focus(previous);
            }
        });
    }

    pub fn render(
        &mut self,
        ctx: &egui::Context,
        colors: &C,
        entries: &[SearchEntry],
        has_more_tasks: bool,
    ) -> Option<NavigationAction> {
        let mut selected_action = None;
        let modal = egui::Modal::new(egui::Id::new("quick_switcher"))
            .frame(egui::Frame::popup(&ctx.style()).fill(colors.raised)
                .corner_radius(design::CARD_RADIUS).inner_margin(18))
            .show(ctx, |ui| {
                ui.set_width(SEARCH_WIDTH.min(ctx.screen_rect().width() - 64.0));
                ui.horizontal(|ui| {
                    design::icon(ui, design::Icon::Search, colors.accent_text, 22.0);
                    ui.label(RichText::new(colors.tr("Jump to anything"))
                        .font(typography::region_font(ui, colors.language, typography::TITLE_SIZE)).color(colors.txt));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("Esc").monospace().size(11.0).color(colors.muted));
                    });
                });
                ui.add_space(12.0);
                let composing = super::supervisor_app::ime_composition_active(ctx);
                let (up, down, enter) = if composing {
                    (false, false, false)
                } else {
                    ctx.input_mut(|input| (
                        input.consume_key(Modifiers::NONE, Key::ArrowUp),
                        input.consume_key(Modifiers::NONE, Key::ArrowDown),
                        input.consume_key(Modifiers::NONE, Key::Enter),
                    ))
                };
                let search_field = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::new(SEARCH_INPUT_ID))
                        .hint_text(colors.tr("Search tasks, IDs, agents or actions…"))
                        .char_limit(QUERY_LIMIT).desired_width(f32::INFINITY),
                );
                if self.focus_requested || up || down {
                    search_field.request_focus();
                    // A new modal is measured invisibly before it becomes
                    // interactive. Repeat the focus request on its visible pass.
                    self.focus_requested = ui.is_sizing_pass();
                }
                // request_focus resets egui's event filter. Keep arrow keys
                // owned by this palette so native directional focus cannot
                // overwrite the selected result at the end of the frame.
                ctx.memory_mut(|memory| memory.set_focus_lock_filter(search_field.id, egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                }));
                if search_field.changed() {
                    self.selected = 0;
                }
                ui.add_space(12.0);
                let matches = quick_search::search(entries, &self.query);
                self.selected = self.selected.min(matches.len().saturating_sub(1));
                if !matches.is_empty() {
                    if down {
                        self.selected = (self.selected + 1) % matches.len();
                    } else if up {
                        self.selected = (self.selected + matches.len() - 1) % matches.len();
                    }
                } else {
                    ui.add_space(18.0);
                    ui.label(RichText::new(colors.tr("No matches in the loaded workspace")).strong().color(colors.txt));
                    ui.label(RichText::new(colors.tr("Try a task title, ID, harness name or Settings.")).size(13.0).color(colors.muted));
                    ui.add_space(18.0);
                }
                egui::ScrollArea::vertical()
                    .id_salt("quick_switcher_results")
                    .max_height((ctx.screen_rect().height() - RESULTS_RESERVED_HEIGHT).max(96.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                    for (index, entry) in matches.iter().enumerate() {
                    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), RESULT_HEIGHT));
                    let response = ui.interact(rect, egui::Id::new(("quick_switcher_result", index)), egui::Sense::click());
                    if response.has_focus() {
                        self.selected = index;
                        ctx.memory_mut(|memory| memory.set_focus_lock_filter(response.id, egui::EventFilter {
                            vertical_arrows: true,
                            ..Default::default()
                        }));
                    }
                    if index == self.selected && (up || down || search_field.changed() || response.gained_focus()) {
                        response.scroll_to_me(Some(egui::Align::Center));
                    }
                    if index == self.selected || response.hovered() {
                        ui.painter().rect_filled(rect, design::BUTTON_RADIUS,
                            if index == self.selected { colors.accent_bg } else { colors.surf });
                    }
                    let width = (rect.width() - 24.0).max(0.0);
                    for (text, size, offset, color) in [
                        (entry.label.as_str(), typography::BODY_SIZE, 6.0, colors.txt),
                        (entry.detail.as_str(), 11.0, 27.0, colors.muted),
                    ] {
                        let galley = egui::WidgetText::from(RichText::new(text).size(size).color(color))
                            .into_galley(ui, Some(egui::TextWrapMode::Truncate), width, egui::TextStyle::Body);
                        ui.painter().galley(rect.min + egui::vec2(12.0, offset), galley, color);
                    }
                    design::paint_focus_ring(ui, &response, design::BUTTON_RADIUS, colors);
                    response.widget_info(|| egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel, true, index == self.selected, &entry.label));
                    if response.clicked() {
                        selected_action = Some(entry.action.clone());
                    }
                    }
                });
                if enter {
                    selected_action = matches.get(self.selected).map(|entry| entry.action.clone());
                }
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new(colors.tr("↑ ↓ to navigate    Enter to open    Esc to close"))
                    .size(11.0).color(colors.muted));
                if has_more_tasks {
                    ui.label(RichText::new(colors.tr("Searching loaded tasks only. Load more in Conversation for older tasks."))
                        .size(11.0).color(colors.muted));
                }
            });
        if selected_action.is_some()
            || (!super::supervisor_app::ime_composition_active(ctx) && modal.should_close())
        {
            self.close(ctx);
        }
        selected_action
    }
}
