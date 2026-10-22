#![forbid(unsafe_code)]
//! Claude-style composer: rounded box, target chip, circular send button.
//! Sending stays unavailable (ADR 028); Enter attempts to send and never
//! inserts a newline, Shift+Enter does (ADR 030 §4).

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, RichText, Sense, Ui, vec2};
use vibemux_harness::dispatch::AdapterSupport;
use vibemux_probe::ProbeState;

use super::{
    C,
    design::{self, Icon},
    supervisor_state::{ComposerTarget, SupervisorUiState},
};
use crate::view_model::{AgentView, ViewModel};

pub const COMPOSER_TEXT_ID: &str = "coordinator_composer_text";
pub const SEND_BUTTON_ID: &str = "coordinator_send_button";
pub const COMPOSER_PLACEHOLDER: &str = "How can I help today?";
const SEND_BUTTON_SIZE: f32 = 32.0;
/// The draft area never takes more than this share of the window height.
const COMPOSER_TEXT_MAX_SHARE: f32 = 0.35;
const COMPOSER_TEXT_MIN_HEIGHT: f32 = 48.0;
const COMPOSER_TEXT_MAX_HEIGHT: f32 = 240.0;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetOption {
    pub target: ComposerTarget,
    pub label: String,
    pub enabled: bool,
    pub reason: Option<String>,
}

#[must_use]
pub fn target_options(view_model: &ViewModel) -> Vec<TargetOption> {
    let mut options = vec![TargetOption {
        target: ComposerTarget::Coordinator,
        label: "Coordinator".to_string(),
        enabled: true,
        reason: None,
    }];
    options.extend(view_model.agents.iter().map(|agent| {
        let reason = target_unavailable_reason(agent);
        TargetOption {
            target: ComposerTarget::Harness(agent.name.clone()),
            label: agent.name.clone(),
            enabled: reason.is_none(),
            reason: reason.map(str::to_string),
        }
    }));
    options
}

/// A harness can take a prompt only when it is detected and a structured
/// adapter can execute a turn for it; the first missing fact is the reason.
fn target_unavailable_reason(agent: &AgentView) -> Option<&'static str> {
    if agent.launcher_state != ProbeState::Verified {
        return Some("Not detected on this machine");
    }
    match agent.adapter.support {
        AdapterSupport::Execute => None,
        AdapterSupport::ProbeOnly => Some("Structured adapter is probe-only"),
        AdapterSupport::TerminalOnly => Some("No structured adapter (terminal UI only)"),
    }
}

/// The line under the composer: what Send will do for this target.
#[must_use]
pub fn caption_text(target: &ComposerTarget, ask_ready: bool) -> String {
    match target {
        ComposerTarget::Coordinator => "Draft only · coordinator chat is not connected".to_string(),
        ComposerTarget::Harness(name) if ask_ready => {
            format!("Ask {name} · read-only, it cannot change files · may use paid inference")
        }
        ComposerTarget::Harness(name) => {
            format!("Draft for {name} · set up Ask in this folder to send")
        }
    }
}

pub fn render(ui: &mut Ui, c: &C, state: &mut SupervisorUiState, options: &[TargetOption]) {
    let now = ui.input(|input| input.time);
    design::composer_frame(c).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let mut draft = state.composer_draft().to_string();
        // Long drafts scroll inside the composer instead of growing it past
        // the window, which would hide the send row, the caption, and the
        // conversation.
        let max_text_height = (ui.ctx().content_rect().height() * COMPOSER_TEXT_MAX_SHARE)
            .clamp(COMPOSER_TEXT_MIN_HEIGHT, COMPOSER_TEXT_MAX_HEIGHT);
        let response = egui::ScrollArea::vertical()
            .id_salt("composer_text_scroll")
            .max_height(max_text_height)
            .show(ui, |ui| {
                // egui 0.34 maps hint atoms through the local weak-text color.
                // Keep the established palette value without changing other UI.
                ui.visuals_mut().weak_text_color = Some(c.muted);
                ui.add(
                    egui::TextEdit::multiline(&mut draft)
                        .id(egui::Id::new(COMPOSER_TEXT_ID))
                        .desired_width(f32::INFINITY)
                        .desired_rows(2)
                        .hint_text(RichText::new(COMPOSER_PLACEHOLDER).color(c.muted))
                        .frame(egui::Frame::NONE)
                        .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                        .font(egui::TextStyle::Body),
                )
            })
            .inner;
        if state.take_composer_focus_request() {
            response.request_focus();
        }
        if response.changed() {
            state.set_composer_draft(draft);
        }
        let enter_pressed = response.has_focus()
            && ui.input(|input| {
                let composing = input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Ime(_)));
                !composing && input.key_pressed(Key::Enter) && !input.modifiers.shift
            });
        if enter_pressed {
            attempt_send(state, now);
        }
        ui.add_space(8.0);
        // A bounded row: centering inside an unbounded `horizontal` would
        // push the send button to the middle of all remaining height.
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), SEND_BUTTON_SIZE),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                target_chip(ui, c, state, options);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let enabled = state.send_enabled();
                    let response = send_button(ui, c, enabled).on_hover_text(if enabled {
                        "Send as a read-only Ask"
                    } else {
                        state.send_disabled_reason()
                    });
                    if response.clicked() {
                        attempt_send(state, now);
                    }
                });
            },
        );
    });
    ui.add_space(6.0);
    let hint = state.not_sent_hint_visible(now);
    let ask_ready = match state.composer_target() {
        ComposerTarget::Harness(name) => state.ask_ready_for(name),
        ComposerTarget::Coordinator => false,
    };
    ui.label(
        RichText::new(if hint {
            format!("Not sent: {}", state.send_disabled_reason())
        } else {
            caption_text(state.composer_target(), ask_ready)
        })
        .size(11.5)
        .color(if hint { c.warn } else { c.muted }),
    );
    if hint {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
}

fn attempt_send(state: &mut SupervisorUiState, now: f64) {
    if !state.try_send() {
        state.show_not_sent_hint(now);
    }
}

fn send_button(ui: &mut Ui, c: &C, enabled: bool) -> egui::Response {
    let (_, rect) = ui.allocate_space(vec2(SEND_BUTTON_SIZE, SEND_BUTTON_SIZE));
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, egui::Id::new(SEND_BUTTON_ID), sense);
    let (fill, arrow) = if enabled {
        (c.accent, if c.light { egui::Color32::WHITE } else { c.bg })
    } else {
        (c.border, c.muted)
    };
    ui.painter()
        .circle_filled(rect.center(), SEND_BUTTON_SIZE / 2.0, fill);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)));
    design::icon(&mut child, Icon::Send, arrow, 16.0);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "Send"));
    response
}

fn target_chip(ui: &mut Ui, c: &C, state: &mut SupervisorUiState, options: &[TargetOption]) {
    let current = options
        .iter()
        .find(|option| &option.target == state.composer_target())
        .map_or("Coordinator", |option| option.label.as_str())
        .to_string();
    egui::ComboBox::from_id_salt("composer_target")
        .selected_text(RichText::new(current).size(12.5).color(c.muted))
        .show_ui(ui, |ui| {
            for option in options {
                let selected = &option.target == state.composer_target();
                let response = ui.add_enabled(
                    option.enabled,
                    egui::Button::selectable(selected, option.label.as_str()),
                );
                let response = match option.reason.as_deref() {
                    Some(reason) => response.on_disabled_hover_text(reason),
                    None => response,
                };
                if response.clicked() {
                    state.set_composer_target(option.target.clone());
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_model::Health;
    use vibemux_harness::dispatch::adapter_capability;
    use vibemux_probe::{AgentKind, RouteKind};

    fn agent(kind: AgentKind, launcher_state: ProbeState) -> AgentView {
        AgentView {
            name: kind.display_name().to_string(),
            launcher_state,
            authentication_state: ProbeState::NotRun,
            inference_state: ProbeState::NotRun,
            version: "-".to_string(),
            route: RouteKind::Unknown,
            code: "test".to_string(),
            adapter: adapter_capability(kind),
        }
    }

    fn view_model(agents: Vec<AgentView>) -> ViewModel {
        ViewModel {
            title: "VibeMux".to_string(),
            platform: "test".to_string(),
            schema_version: 1,
            observed_at_epoch_seconds: 0,
            observed_at: "1970-01-01T00:00:00Z".to_string(),
            health: Health::Warning,
            overall_status: "test".to_string(),
            agents,
            gateway_summary: "unavailable".to_string(),
            gateway_state: ProbeState::Unavailable,
            telemetry_summary: "unavailable".to_string(),
            telemetry_failures: 0,
            telemetry_available: false,
            a2a_summary: "unavailable".to_string(),
            a2a_state: ProbeState::Unavailable,
            env_allowlist: Vec::new(),
        }
    }

    #[test]
    fn coordinator_comes_first_and_undetected_harnesses_are_disabled() {
        let options = target_options(&view_model(vec![
            agent(AgentKind::Codex, ProbeState::Verified),
            agent(AgentKind::Grok, ProbeState::Unavailable),
        ]));
        assert_eq!(options[0].target, ComposerTarget::Coordinator);
        assert!(options[0].enabled);
        assert!(options[1].enabled);
        assert!(!options[2].enabled);
        assert_eq!(
            options[2].reason.as_deref(),
            Some("Not detected on this machine")
        );
    }

    #[test]
    fn detected_harnesses_without_an_executing_adapter_say_why() {
        let options = target_options(&view_model(vec![
            agent(AgentKind::Claude, ProbeState::Verified),
            agent(AgentKind::OpenCode, ProbeState::Verified),
            agent(AgentKind::Kimi, ProbeState::Verified),
        ]));
        assert!(options[1].enabled);
        assert_eq!(options[1].reason, None);
        assert!(!options[2].enabled);
        assert_eq!(
            options[2].reason.as_deref(),
            Some("Structured adapter is probe-only")
        );
        assert!(!options[3].enabled);
        assert_eq!(
            options[3].reason.as_deref(),
            Some("No structured adapter (terminal UI only)")
        );
    }

    #[test]
    fn captions_say_what_send_will_do() {
        assert_eq!(
            caption_text(&ComposerTarget::Coordinator, false),
            "Draft only · coordinator chat is not connected"
        );
        assert_eq!(
            caption_text(&ComposerTarget::Harness("Codex".to_string()), false),
            "Draft for Codex · set up Ask in this folder to send"
        );
        let ready = caption_text(&ComposerTarget::Harness("Codex".to_string()), true);
        assert!(ready.starts_with("Ask Codex"));
        assert!(ready.contains("read-only"));
        assert!(ready.contains("paid inference"));
    }
}
