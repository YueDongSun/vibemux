#![forbid(unsafe_code)]
//! Read-only runtime and local-probe diagnostics.

use eframe::egui::{self, RichText, Ui};

use crate::{
    supervisor_model::{ConnectionStatus, SupervisorSnapshot},
    view_model::{Health, ViewModel},
};

use super::{C, chat::wrapped_label};

pub fn render(
    ctx: &egui::Context,
    colors: &C,
    open: &mut bool,
    snapshot: &SupervisorSnapshot,
    view_model: &ViewModel,
) {
    egui::Window::new("Diagnostics")
        .open(open)
        .collapsible(false)
        .default_width(520.0)
        .default_height(540.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if snapshot.mode == crate::supervisor_model::SnapshotMode::Demo {
                        ui.label(RichText::new("DEMO DATA").size(11.0).strong().color(colors.warn));
                        ui.add_space(8.0);
                    }
                    section(ui, colors, "SUPERVISOR");
                    value_row(
                        ui,
                        colors,
                        "Connection",
                        snapshot.connection.status.label(),
                        connection_color(colors, snapshot.connection.status),
                    );
                    if let Some(detail) = snapshot.connection.detail.as_deref() {
                        wrapped_label(ui, RichText::new(detail).size(12.0).color(colors.muted));
                    }
                    value_row(ui, colors, "Project", &snapshot.project_name, colors.txt);
                    value_row(ui, colors, "Tasks in page", &snapshot.tasks.len().to_string(), colors.txt);
                    value_row(
                        ui,
                        colors,
                        "More tasks",
                        if snapshot.next_cursor.is_some() { "available" } else { "none reported" },
                        colors.muted,
                    );
                    ui.add_space(14.0);

                    section(ui, colors, "LOCAL PROBE");
                    value_row(ui, colors, "Platform", &view_model.platform, colors.txt);
                    value_row(ui, colors, "Agents", &view_model.agents.len().to_string(), colors.txt);
                    value_row(ui, colors, "Gateway", &view_model.gateway_summary, colors.txt);
                    value_row(ui, colors, "A2A self-test", &view_model.a2a_summary, colors.txt);
                    value_row(ui, colors, "Observed at", &view_model.observed_at, colors.muted);
                    value_row(
                        ui,
                        colors,
                        "Aggregate health",
                        &view_model.overall_status,
                        health_color(colors, view_model.health),
                    );
                    ui.add_space(14.0);

                    section(ui, colors, "ALLOWLISTED ENVIRONMENT");
                    if view_model.env_allowlist.is_empty() {
                        ui.label(RichText::new("No allowlisted environment values were reported.").size(12.0).color(colors.muted));
                    } else {
                        for (key, value) in &view_model.env_allowlist {
                            value_row(
                                ui,
                                colors,
                                key,
                                if value.is_some() { "set" } else { "unset" },
                                colors.muted,
                            );
                        }
                    }
                    ui.add_space(12.0);
                    wrapped_label(
                        ui,
                        RichText::new("Diagnostics show probe summaries and allowlisted variable presence; values and prompt content are not displayed.")
                            .size(12.0)
                            .color(colors.faint),
                    );
                });
        });
}

fn section(ui: &mut Ui, colors: &C, label: &str) {
    ui.label(RichText::new(label).size(11.0).strong().color(colors.faint));
    ui.add_space(5.0);
}

fn value_row(ui: &mut Ui, colors: &C, label: &str, value: &str, value_color: egui::Color32) {
    ui.horizontal_top(|ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(label).size(12.0).color(colors.muted));
        ui.add_space(8.0);
        wrapped_label(
            ui,
            RichText::new(value)
                .size(12.0)
                .color(value_color)
                .monospace(),
        );
    });
    ui.add_space(5.0);
}

fn connection_color(colors: &C, status: ConnectionStatus) -> egui::Color32 {
    match status {
        ConnectionStatus::Connected => colors.ok,
        ConnectionStatus::Connecting => colors.warn,
        ConnectionStatus::Disconnected | ConnectionStatus::Unavailable => colors.muted,
    }
}

fn health_color(colors: &C, health: Health) -> egui::Color32 {
    match health {
        Health::Ok => colors.ok,
        Health::Warning => colors.warn,
        Health::Failure => colors.danger,
    }
}
