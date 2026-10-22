#![forbid(unsafe_code)]
//! The card shown when the active folder has no running daemon, and the
//! state of a start or stop the user asked for. The card only reports the
//! click; `vibemuxctl` does the work off the UI thread.

use eframe::egui::{self, RichText, Stroke, Ui};

use crate::{daemon_control::DaemonCommand, supervisor_model::ConnectionState};

use super::{C, design, typography};

/// The connection reason code that means "nothing is running here".
pub const DAEMON_NOT_RUNNING: &str = "daemon_not_running";

/// A start or stop in flight, or the last one's failure.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum DaemonJob {
    #[default]
    Idle,
    Running(DaemonCommand),
    Failed(String),
}

impl DaemonJob {
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Running(_))
    }
}

/// Whether the card should be shown for this connection.
#[must_use]
pub fn daemon_missing(connection: &ConnectionState) -> bool {
    connection.code.as_deref() == Some(DAEMON_NOT_RUNNING)
}

/// Draws the card. Returns `Start` when the user clicks it.
pub fn render(ui: &mut Ui, c: &C, folder_name: &str, job: &DaemonJob) -> Option<DaemonCommand> {
    let mut clicked = None;
    egui::Frame::new()
        .fill(c.raised)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(design::CARD_RADIUS)
        .inner_margin(egui::Margin::symmetric(22, 18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(format!("VibeMux isn't running for {folder_name}"))
                    .font(typography::display_font(ui.ctx(), typography::TASK_TITLE_SIZE))
                    .color(c.txt),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Start it to see this folder's tasks. It keeps running in the background until you stop it in Settings → Connection.",
                )
                .size(13.0)
                .color(c.muted),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| match job {
                DaemonJob::Running(command) => {
                    ui.spinner();
                    ui.label(
                        RichText::new(command.progress_label())
                            .size(13.0)
                            .color(c.muted),
                    );
                }
                _ => {
                    if start_button(ui, c).clicked() {
                        clicked = Some(DaemonCommand::Start);
                    }
                }
            });
            if let DaemonJob::Failed(message) = job {
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(RichText::new(message).size(12.5).color(c.danger)).wrap(),
                );
            }
        });
    clicked
}

/// The accent "Start VibeMux" button.
pub fn start_button(ui: &mut Ui, c: &C) -> egui::Response {
    let text_color = if c.light { egui::Color32::WHITE } else { c.bg };
    let response = ui.add(
        egui::Button::new(RichText::new("Start VibeMux").size(13.5).color(text_color))
            .fill(c.accent)
            .corner_radius(design::BUTTON_RADIUS)
            .min_size(egui::vec2(0.0, 32.0)),
    );
    design::paint_focus_ring(ui, &response, design::BUTTON_RADIUS, c);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor_model::ConnectionStatus;

    #[test]
    fn the_card_follows_the_reason_code_not_the_text() {
        let mut connection = ConnectionState {
            status: ConnectionStatus::Disconnected,
            detail: Some("The daemon is not running for this project.".into()),
            code: None,
        };
        assert!(!daemon_missing(&connection), "text alone is not a reason");
        connection.code = Some(DAEMON_NOT_RUNNING.into());
        assert!(daemon_missing(&connection));
        connection.code = Some("control_unsupported_version".into());
        assert!(!daemon_missing(&connection));
    }

    #[test]
    fn only_a_command_in_flight_is_running() {
        assert!(DaemonJob::Running(DaemonCommand::Start).is_running());
        assert!(!DaemonJob::Idle.is_running());
        assert!(!DaemonJob::Failed("no".into()).is_running());
    }
}
