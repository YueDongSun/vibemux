#![forbid(unsafe_code)]
//! The `Ctrl+K` quick switcher popup (ADR 030 §5). Ranking lives in
//! `crate::quick_search`; this file draws the popup and reads its keys.

use eframe::egui::{self, Key, Modifiers, RichText};

use crate::quick_search::{SearchEntry, SearchTarget, rank};

use super::C;

const MODAL_ID: &str = "quick_switcher";
const QUERY_FIELD_ID: &str = "quick_switcher_query";
const POPUP_WIDTH: f32 = 460.0;

#[derive(Debug, Default)]
pub struct QuickSwitcherState {
    pub open: bool,
    pub query: String,
    /// Index into the ranked results.
    pub selected: usize,
}

impl QuickSwitcherState {
    /// Opens with an empty query and the first result selected.
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
    }
}

/// Draws the open switcher and returns the entry the operator picked with
/// `Enter` or a click. `Up`/`Down` move the selection and `Esc` closes.
pub fn show(
    ctx: &egui::Context,
    colors: &C,
    state: &mut QuickSwitcherState,
    entries: &[SearchEntry],
) -> Option<SearchTarget> {
    let results = rank(entries, &state.query);
    // Consumed before the query field sees them.
    let (down, up, enter, escape) = ctx.input_mut(|input| {
        (
            input.consume_key(Modifiers::NONE, Key::ArrowDown),
            input.consume_key(Modifiers::NONE, Key::ArrowUp),
            input.consume_key(Modifiers::NONE, Key::Enter),
            input.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if escape {
        state.close();
        return None;
    }
    if !results.is_empty() {
        let count = results.len();
        if down {
            state.selected = (state.selected + 1) % count;
        }
        if up {
            state.selected = (state.selected + count - 1) % count;
        }
        state.selected = state.selected.min(count - 1);
    }
    let mut picked = enter
        .then(|| {
            results
                .get(state.selected)
                .map(|entry| entry.target.clone())
        })
        .flatten();
    let modal = egui::Modal::new(egui::Id::new(MODAL_ID)).show(ctx, |ui| {
        ui.set_width(POPUP_WIDTH);
        let field = ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .id(egui::Id::new(QUERY_FIELD_ID))
                .hint_text("Search tasks, actions, and agents")
                .desired_width(f32::INFINITY),
        );
        if !field.has_focus() {
            field.request_focus();
        }
        if field.changed() {
            state.selected = 0;
        }
        ui.add_space(8.0);
        if results.is_empty() {
            ui.label(RichText::new("No matches").color(colors.muted));
        }
        for (index, entry) in results.iter().enumerate() {
            let active = index == state.selected;
            let row = ui.horizontal(|ui| {
                let label = ui.selectable_label(
                    active,
                    RichText::new(&entry.label).color(if active {
                        colors.accent_text
                    } else {
                        colors.txt
                    }),
                );
                ui.label(RichText::new(&entry.detail).size(11.0).color(colors.muted));
                label
            });
            if row.inner.clicked() {
                picked = Some(entry.target.clone());
            }
        }
    });
    if picked.is_some() || modal.should_close() {
        state.close();
    }
    picked
}
