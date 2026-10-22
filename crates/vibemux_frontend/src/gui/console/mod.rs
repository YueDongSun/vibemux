#![forbid(unsafe_code)]
//! The Console page: the terminal console's views (ADR 041, ADR 042) as
//! Claude-style cards behind a section list. Probe facts come from the
//! GUI's view model, the Git inventory from `vibemuxctl`, and daemon facts
//! from the content-free daemon feed. Nothing here starts a daemon, a
//! harness, or a model turn: the page asks only for reads (refresh,
//! re-probe, a task's detail) and for navigation.

mod daemon_views;
mod harnesses;
mod overview;
mod state;
mod workspace_view;

use std::path::Path;

use eframe::egui::{self, RichText, Ui};

pub use state::{ConsoleData, ProbeJob, WorkspaceLoad};

use crate::{supervisor_model::SupervisorSnapshot, view_model::ViewModel};

use super::{
    C, design,
    section_page::{self, nav_item},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConsoleSection {
    #[default]
    Overview,
    Harnesses,
    Workspace,
    Tasks,
    Daemon,
    Sessions,
    Decisions,
}

impl ConsoleSection {
    pub const ALL: [Self; 7] = [
        Self::Overview,
        Self::Harnesses,
        Self::Workspace,
        Self::Tasks,
        Self::Daemon,
        Self::Sessions,
        Self::Decisions,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Harnesses => "Harnesses",
            Self::Workspace => "Workspace",
            Self::Tasks => "Tasks",
            Self::Daemon => "Daemon",
            Self::Sessions => "Sessions",
            Self::Decisions => "Decisions",
        }
    }
}

/// What the page reads to draw itself.
pub struct ConsoleView<'a> {
    pub section: ConsoleSection,
    pub selected_harness: Option<usize>,
    pub view_model: &'a ViewModel,
    /// Display names of the harnesses an Ask can run in this folder.
    pub ask_ready: &'a [String],
    pub folder: Option<&'a Path>,
    /// The supervisor snapshot: the connection, and the tasks a task window
    /// can open.
    pub snapshot: &'a SupervisorSnapshot,
    pub data: &'a ConsoleData,
}

/// What the user asked for this frame; the caller applies it.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct ConsoleActions {
    pub section: Option<ConsoleSection>,
    /// `Some(Some(index))` opens a harness, `Some(None)` returns to the list.
    pub harness: Option<Option<usize>>,
    /// Go to the conversation with this harness targeted. Never sends.
    pub ask: Option<String>,
    pub reprobe: bool,
    pub refresh: bool,
    /// Read this task's detail through the daemon feed.
    pub open_task: Option<String>,
    /// Open this task's window.
    pub task_window: Option<String>,
}

pub fn render_page(ui: &mut Ui, c: &C, view: &ConsoleView<'_>) -> ConsoleActions {
    let mut actions = ConsoleActions::default();
    let mut chosen = None;
    let title = match view.section {
        ConsoleSection::Overview => "Console",
        section => section.label(),
    };
    section_page::shell(
        ui,
        c,
        "console_content",
        title,
        |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Console").size(12.5).color(c.muted));
            ui.add_space(10.0);
            for section in ConsoleSection::ALL {
                if nav_item(ui, c, section.label(), section == view.section) {
                    chosen = Some(section);
                }
                ui.add_space(2.0);
            }
        },
        |ui| match view.section {
            ConsoleSection::Overview => overview::render(ui, c, view, &mut actions),
            ConsoleSection::Harnesses => harnesses::render(ui, c, view, &mut actions),
            ConsoleSection::Workspace => workspace_view::render(ui, c, view, &mut actions),
            ConsoleSection::Tasks => daemon_views::tasks(ui, c, view, &mut actions),
            ConsoleSection::Daemon => daemon_views::daemon(ui, c, view, &mut actions),
            ConsoleSection::Sessions => daemon_views::sessions(ui, c, view, &mut actions),
            ConsoleSection::Decisions => daemon_views::decisions(ui, c, view, &mut actions),
        },
    );
    if chosen.is_some() {
        actions.section = chosen;
    }
    actions
}

/// A muted status line with one action on the right. Returns true when the
/// action is clicked.
fn toolbar(ui: &mut Ui, c: &C, status: &str, action: &str, enabled: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(status).size(12.5).color(c.muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clicked = design::action_button(ui, c, action, enabled).clicked();
        });
    });
    ui.add_space(14.0);
    clicked
}

/// A one-line explanation in place of data that is not there.
fn notice(ui: &mut Ui, c: &C, text: &str) {
    section_page::card(ui, c, |ui| {
        ui.add_space(12.0);
        section_page::footnote(ui, c, text);
        ui.add_space(12.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_has_a_distinct_label() {
        let labels: std::collections::BTreeSet<_> = ConsoleSection::ALL
            .iter()
            .map(|section| section.label())
            .collect();
        assert_eq!(labels.len(), ConsoleSection::ALL.len());
        assert_eq!(ConsoleSection::default(), ConsoleSection::Overview);
    }
}
