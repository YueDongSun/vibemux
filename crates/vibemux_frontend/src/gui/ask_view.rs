#![forbid(unsafe_code)]
//! The read-only Ask surfaces: the transcript of this window's Asks, the
//! one-click setup card, the availability note, and the once-per-folder
//! confirmation. They only draw state and report clicks; the app and the
//! live adapter act.

use eframe::egui::{self, RichText, Stroke, Ui};
use uuid::Uuid;
use vibemux_harness::AgentKind;

use crate::ask::{AskEnd, AskPhase, AskUpdate, SetupOutcome, reply_text::MAX_REPLY_BYTES};

use super::{C, design, typography};

/// At most this many Asks stay in the window's transcript.
pub const MAX_ASKS: usize = 20;

/// One Ask in this window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AskEntry {
    pub request_id: Option<Uuid>,
    pub harness: String,
    pub prompt: String,
    pub phase: AskPhase,
    pub reply: String,
    pub truncated: bool,
    pub end: Option<AskEnd>,
}

impl AskEntry {
    #[must_use]
    pub const fn submitted(harness: String, prompt: String) -> Self {
        Self {
            request_id: None,
            harness,
            prompt,
            phase: AskPhase::Submitting,
            reply: String::new(),
            truncated: false,
            end: None,
        }
    }

    #[must_use]
    pub const fn running(&self) -> bool {
        self.end.is_none()
    }

    /// Folds in an update for this entry's request.
    pub fn apply(&mut self, update: AskUpdate) {
        self.request_id = Some(update.request_id);
        self.phase = update.phase;
        if !update.reply.is_empty() || update.end.is_some() {
            self.reply = update.reply;
        }
        self.truncated = update.truncated;
        self.end = update.end;
    }
}

/// What this folder's daemon can do for Ask.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum AskAvailability {
    #[default]
    Unknown,
    Checking,
    /// The folder has no dispatch config yet.
    NotConfigured,
    /// The harnesses whose route is enabled and allowed to execute.
    Ready(Vec<AgentKind>),
    /// The daemon could not list its routes; the stable reason code.
    Unavailable(String),
}

impl AskAvailability {
    /// Display names of the harnesses an Ask can run now.
    #[must_use]
    pub fn ready_names(&self) -> Vec<String> {
        match self {
            Self::Ready(harnesses) => harnesses
                .iter()
                .map(|harness| harness.display_name().to_string())
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// The one-click setup's progress.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum SetupJob {
    #[default]
    Idle,
    Running,
    Finished(SetupOutcome),
}

/// Clicks on the Ask surfaces this frame.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct AskViewActions {
    pub cancel: bool,
    pub enable: bool,
    pub dismiss_setup: bool,
    pub recheck: bool,
}

impl AskViewActions {
    pub fn merge(&mut self, other: Self) {
        self.cancel |= other.cancel;
        self.enable |= other.enable;
        self.dismiss_setup |= other.dismiss_setup;
        self.recheck |= other.recheck;
    }
}

/// Whether the setup card shows: while setup runs or reports, or when the
/// composer targets a harness in a folder without routes.
#[must_use]
pub fn setup_card_visible(
    availability: &AskAvailability,
    setup: &SetupJob,
    harness_target: bool,
) -> bool {
    !matches!(setup, SetupJob::Idle)
        || (harness_target && matches!(availability, AskAvailability::NotConfigured))
}

/// Why the targeted harness cannot be asked here, when routes exist but do
/// not cover it or the daemon could not list them.
#[must_use]
pub fn availability_note(availability: &AskAvailability, target: Option<&str>) -> Option<String> {
    let target = target?;
    match availability {
        AskAvailability::Unavailable(code) => {
            Some(format!("Ask is unavailable in this folder ({code})."))
        }
        AskAvailability::Ready(harnesses)
            if !harnesses
                .iter()
                .any(|harness| harness.display_name() == target) =>
        {
            Some(format!(
                "This folder's .vibemux\\harness_dispatch.json has no enabled route that lets {target} run a turn. Edit it, then restart VibeMux."
            ))
        }
        _ => None,
    }
}

pub fn render_setup_card(ui: &mut Ui, c: &C, setup: &SetupJob) -> AskViewActions {
    let mut actions = AskViewActions::default();
    card(ui, c, |ui| {
        ui.label(
            RichText::new("Ask Claude or Codex")
                .font(typography::display_font(
                    ui.ctx(),
                    typography::TASK_TITLE_SIZE,
                ))
                .color(c.txt),
        );
        ui.add_space(4.0);
        match setup {
            SetupJob::Idle => {
                muted(
                    ui,
                    c,
                    "VibeMux runs one turn of the harness and shows its reply; it cannot change files. Claude gets no tools, so it answers from your prompt alone; Codex can read this folder under a read-only sandbox. Setup writes .vibemux\\harness_dispatch.json with read-only routes for the Claude and Codex executables found on this machine, then restarts VibeMux for this folder.",
                );
                ui.add_space(12.0);
                if design::accent_button(ui, c, "Enable Claude and Codex (read-only)").clicked() {
                    actions.enable = true;
                }
            }
            SetupJob::Running => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    muted(ui, c, "Setting up Ask and restarting VibeMux…");
                });
            }
            SetupJob::Finished(SetupOutcome::Done {
                config_path,
                enabled,
                skipped,
            }) => {
                let names = enabled
                    .iter()
                    .map(|harness| harness.display_name())
                    .collect::<Vec<_>>()
                    .join(" and ");
                ui.label(
                    RichText::new(format!("Ask is ready for {names}."))
                        .size(13.5)
                        .color(c.ok),
                );
                muted(ui, c, &format!("Routes saved to {}", config_path.display()));
                for (harness, reason) in skipped {
                    muted(
                        ui,
                        c,
                        &format!(
                            "{} was skipped: {}.",
                            harness.display_name(),
                            reason.message()
                        ),
                    );
                }
                ui.add_space(10.0);
                if design::action_button(ui, c, "Dismiss", true).clicked() {
                    actions.dismiss_setup = true;
                }
            }
            SetupJob::Finished(SetupOutcome::Failed(message)) => {
                ui.add(egui::Label::new(RichText::new(message).size(13.0).color(c.danger)).wrap());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if design::accent_button(ui, c, "Try again").clicked() {
                        actions.enable = true;
                    }
                    if design::action_button(ui, c, "Dismiss", true).clicked() {
                        actions.dismiss_setup = true;
                    }
                });
            }
        }
    });
    actions
}

/// The setup card and the availability note shown above the conversation.
pub fn render_notices(
    ui: &mut Ui,
    c: &C,
    setup: &SetupJob,
    show_setup: bool,
    note: Option<&str>,
) -> AskViewActions {
    let mut actions = AskViewActions::default();
    if show_setup {
        actions.merge(render_setup_card(ui, c, setup));
        ui.add_space(12.0);
    }
    if let Some(note) = note {
        actions.merge(render_note(ui, c, note));
        ui.add_space(12.0);
    }
    actions
}

/// One line explaining why Ask cannot run, with a way to check again.
pub fn render_note(ui: &mut Ui, c: &C, note: &str) -> AskViewActions {
    let mut actions = AskViewActions::default();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(note).size(12.5).color(c.warn));
        if design::quiet_button(ui, c, "Check again").clicked() {
            actions.recheck = true;
        }
    });
    actions
}

/// The transcript, oldest first. Only a running Ask offers Cancel.
pub fn render_asks(ui: &mut Ui, c: &C, asks: &[AskEntry]) -> AskViewActions {
    let mut actions = AskViewActions::default();
    for entry in asks {
        card(ui, c, |ui| render_entry(ui, c, entry, &mut actions));
        ui.add_space(12.0);
    }
    if !asks.is_empty() {
        muted(
            ui,
            c,
            "Replies live in the daemon's memory and are lost when it restarts.",
        );
    }
    actions
}

fn render_entry(ui: &mut Ui, c: &C, entry: &AskEntry, actions: &mut AskViewActions) {
    ui.label(
        RichText::new(format!("You asked {} · read-only", entry.harness))
            .size(12.0)
            .color(c.muted),
    );
    ui.add(
        egui::Label::new(RichText::new(&entry.prompt).size(14.0).color(c.txt))
            .wrap()
            .selectable(true),
    );
    ui.add_space(8.0);
    separator(ui, c);
    ui.add_space(8.0);
    if !entry.reply.is_empty() {
        ui.add(
            egui::Label::new(
                RichText::new(&entry.reply)
                    .font(typography::display_font(ui.ctx(), typography::PROSE_SIZE))
                    .line_height(Some(typography::PROSE_LINE_HEIGHT))
                    .color(c.txt),
            )
            .wrap()
            .selectable(true),
        );
    }
    if entry.truncated {
        muted(
            ui,
            c,
            &format!("The reply was cut at {} KiB.", MAX_REPLY_BYTES / 1024),
        );
    }
    if entry.end == Some(AskEnd::Completed) {
        return;
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| match &entry.end {
        None => {
            ui.spinner();
            muted(ui, c, &progress_label(entry));
            if entry.phase != AskPhase::Cancelling
                && design::action_button(ui, c, "Cancel", true).clicked()
            {
                actions.cancel = true;
            }
        }
        Some(AskEnd::Completed) => {}
        Some(AskEnd::Unverified) => {
            ui.label(
                RichText::new("Ended without a verified finish; the reply may be incomplete.")
                    .size(12.5)
                    .color(c.warn),
            );
        }
        Some(AskEnd::Cancelled) => muted(ui, c, "Cancelled."),
        Some(AskEnd::Failed(code)) => {
            ui.label(
                RichText::new(format!("Failed: {code}"))
                    .size(12.5)
                    .color(c.danger),
            );
        }
    });
}

fn progress_label(entry: &AskEntry) -> String {
    match entry.phase {
        AskPhase::Submitting => format!("Sending to {}…", entry.harness),
        AskPhase::Cancelling => "Cancelling…".to_string(),
        AskPhase::Running | AskPhase::Finished => format!("{} is working…", entry.harness),
    }
}

/// The once-per-folder confirmation before the first Ask.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfirmState {
    pub open: bool,
    pub harness: String,
    pub prompt: String,
    pub folder: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmChoice {
    None,
    Send,
    Cancel,
}

pub fn render_confirm(ctx: &egui::Context, c: &C, state: &ConfirmState) -> ConfirmChoice {
    let mut choice = ConfirmChoice::None;
    let response = egui::Modal::new(egui::Id::new("ask_confirm_dialog"))
        .frame(
            egui::Frame::new()
                .fill(c.raised)
                .stroke(Stroke::new(1.0, c.border))
                .corner_radius(design::CARD_RADIUS)
                .inner_margin(egui::Margin::same(22)),
        )
        .show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(
                RichText::new(format!("Ask {}?", state.harness))
                    .font(typography::display_font(ui.ctx(), typography::TITLE_SIZE))
                    .color(c.txt),
            );
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "VibeMux will run one {} turn in {}. {} It may use paid inference. VibeMux asks this once per folder.",
                        state.harness,
                        state.folder,
                        turn_scope(&state.harness)
                    ))
                    .size(13.5)
                    .color(c.muted),
                )
                .wrap(),
            );
            ui.add_space(16.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if design::accent_button(ui, c, "Send").clicked() {
                    choice = ConfirmChoice::Send;
                }
                if design::action_button(ui, c, "Cancel", true).clicked() {
                    choice = ConfirmChoice::Cancel;
                }
            });
        });
    if response.should_close() && choice == ConfirmChoice::None {
        choice = ConfirmChoice::Cancel;
    }
    choice
}

/// What a read-only turn of `harness` can reach, as its dispatch route
/// sets it (ADR 029): Claude runs with no tools; Codex runs in a read-only
/// sandbox.
#[must_use]
pub fn turn_scope(harness: &str) -> &'static str {
    if harness == AgentKind::Claude.display_name() {
        "Its route gives it no tools, so it answers from your prompt alone and cannot open or change files."
    } else if harness == AgentKind::Codex.display_name() {
        "It can read files in this folder under a read-only sandbox but cannot change them."
    } else {
        "It runs read-only and cannot change files."
    }
}

fn card(ui: &mut Ui, c: &C, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(c.raised)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(design::CARD_RADIUS)
        .inner_margin(egui::Margin::symmetric(20, 16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
}

fn separator(ui: &mut Ui, c: &C) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, c.border));
}

fn muted(ui: &mut Ui, c: &C, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(c.muted)).wrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_follows_its_updates_and_keeps_text_until_it_ends() {
        let mut entry = AskEntry::submitted("Codex".into(), "Explain".into());
        assert!(entry.running());
        let request_id = Uuid::new_v4();
        entry.apply(AskUpdate {
            request_id,
            phase: AskPhase::Running,
            reply: "Partial".into(),
            truncated: false,
            end: None,
        });
        // A later empty intermediate update keeps the text already shown.
        entry.apply(AskUpdate {
            request_id,
            phase: AskPhase::Running,
            reply: String::new(),
            truncated: false,
            end: None,
        });
        assert_eq!(entry.reply, "Partial");
        entry.apply(AskUpdate {
            request_id,
            phase: AskPhase::Finished,
            reply: "Whole answer".into(),
            truncated: false,
            end: Some(AskEnd::Completed),
        });
        assert!(!entry.running());
        assert_eq!(entry.reply, "Whole answer");
        assert_eq!(entry.request_id, Some(request_id));
    }

    #[test]
    fn the_consent_says_what_each_harness_can_reach() {
        assert!(turn_scope("Claude").contains("no tools"));
        assert!(turn_scope("Codex").contains("read-only sandbox"));
        for harness in ["Claude", "Codex", "Other"] {
            assert!(turn_scope(harness).contains("change"));
        }
    }

    #[test]
    fn the_setup_card_shows_for_harness_targets_without_routes_and_setup_results() {
        let unconfigured = AskAvailability::NotConfigured;
        assert!(setup_card_visible(&unconfigured, &SetupJob::Idle, true));
        assert!(!setup_card_visible(&unconfigured, &SetupJob::Idle, false));
        assert!(!setup_card_visible(
            &AskAvailability::Unknown,
            &SetupJob::Idle,
            true
        ));
        let ready = AskAvailability::Ready(vec![AgentKind::Codex]);
        assert!(!setup_card_visible(&ready, &SetupJob::Idle, true));
        assert!(setup_card_visible(&ready, &SetupJob::Running, false));
    }

    #[test]
    fn the_note_explains_a_missing_route_or_a_failed_listing() {
        let ready = AskAvailability::Ready(vec![AgentKind::Codex]);
        assert_eq!(availability_note(&ready, Some("Codex")), None);
        assert!(
            availability_note(&ready, Some("Claude"))
                .is_some_and(|note| note.contains("Claude") && note.contains("restart"))
        );
        assert_eq!(availability_note(&ready, None), None);
        let failed = AskAvailability::Unavailable("control_unsupported_version".into());
        assert!(
            availability_note(&failed, Some("Codex"))
                .is_some_and(|note| note.contains("control_unsupported_version"))
        );
        assert_eq!(
            availability_note(&AskAvailability::Checking, Some("Codex")),
            None
        );
        assert_eq!(ready.ready_names(), vec!["Codex".to_string()]);
    }
}
