#![forbid(unsafe_code)]
//! Tasks, Daemon, Sessions, and Decisions: the daemon feed's content-free
//! sections (ADR 042 §2). Each section that failed says why with its code
//! and hint; a missing section never hides the others.

use eframe::egui::{RichText, Ui};

use crate::dashboard::{DaemonView, Section};

use super::{
    super::{C, design, section_page},
    ConsoleActions, ConsoleView, notice, toolbar,
};

pub(super) fn tasks(ui: &mut Ui, c: &C, view: &ConsoleView<'_>, actions: &mut ConsoleActions) {
    let Some(daemon) = feed(ui, c, view, actions) else {
        return;
    };
    section(ui, c, &daemon.tasks, |ui, list| {
        if list.rows.is_empty() {
            notice(ui, c, "No tasks in this workspace yet.");
            return;
        }
        section_page::card(ui, c, |ui| {
            for (index, task) in list.rows.iter().enumerate() {
                if index > 0 {
                    section_page::separator(ui, c);
                }
                section_page::row(ui, c, &task.title, &task.status, |ui| {
                    if design::action_button(ui, c, "Details", true).clicked() {
                        actions.open_task = Some(task.task_id.clone());
                    }
                });
            }
        });
        if list.more {
            ui.add_space(8.0);
            section_page::footnote(
                ui,
                c,
                &format!("Showing the newest {} tasks.", list.rows.len()),
            );
        }
    });
    let Some(detail) = daemon.task_detail.as_ref() else {
        return;
    };
    ui.add_space(20.0);
    section(ui, c, detail, |ui, detail| {
        ui.horizontal(|ui| {
            section_page::sub_heading(ui, c, &detail.title);
            let known = view
                .snapshot
                .tasks
                .iter()
                .any(|task| task.task_id == detail.task_id);
            if known && design::quiet_button(ui, c, "Open window").clicked() {
                actions.task_window = Some(detail.task_id.clone());
            }
        });
        for (heading, lines) in [
            ("Runs", &detail.runs),
            ("Events", &detail.events),
            ("Artifacts", &detail.artifacts),
        ] {
            lines_card(ui, c, heading, lines);
            ui.add_space(12.0);
        }
        if detail.truncated {
            section_page::footnote(ui, c, "Older runs, events, or artifacts are not shown.");
        }
    });
}

pub(super) fn daemon(ui: &mut Ui, c: &C, view: &ConsoleView<'_>, actions: &mut ConsoleActions) {
    let Some(daemon) = feed(ui, c, view, actions) else {
        return;
    };
    section_page::sub_heading(ui, c, "Health");
    section(ui, c, &daemon.health, |ui, health| {
        section_page::value_card(
            ui,
            c,
            &[
                ("Status", health.status.clone()),
                ("Store schema", health.schema.clone()),
                ("Writer queue", health.queue.clone()),
            ],
        );
    });
    ui.add_space(20.0);
    section_page::sub_heading(ui, c, "Dispatch routes");
    section(ui, c, &daemon.routes, |ui, routes| {
        if routes.is_empty() {
            notice(ui, c, "No routes are configured.");
            return;
        }
        let rows: Vec<(&str, String)> = routes
            .iter()
            .map(|route| {
                (
                    route.harness.as_str(),
                    format!("{} · {}", route.protocol, route.mode),
                )
            })
            .collect();
        section_page::value_card(ui, c, &rows);
    });
    ui.add_space(20.0);
    section_page::sub_heading(ui, c, "Plugins");
    section(ui, c, &daemon.plugins, |ui, plugins| {
        if plugins.is_empty() {
            notice(ui, c, "No plugins are configured.");
            return;
        }
        let rows: Vec<(&str, String)> = plugins
            .iter()
            .map(|plugin| {
                (
                    plugin.plugin_id.as_str(),
                    format!(
                        "{} · {} restarts · {}",
                        plugin.state, plugin.restarts, plugin.last_error
                    ),
                )
            })
            .collect();
        section_page::value_card(ui, c, &rows);
    });
}

pub(super) fn sessions(ui: &mut Ui, c: &C, view: &ConsoleView<'_>, actions: &mut ConsoleActions) {
    let Some(daemon) = feed(ui, c, view, actions) else {
        return;
    };
    section_page::sub_heading(ui, c, "DSH session");
    section(ui, c, &daemon.dsh, |ui, dsh| {
        section_page::value_card(
            ui,
            c,
            &[
                ("Phase", dsh.phase.clone()),
                ("Provider", dsh.provider.clone()),
                ("Model", dsh.model.clone()),
                ("Turns", dsh.turns.clone()),
                (
                    "Transport",
                    if dsh.transport_open { "open" } else { "closed" }.to_string(),
                ),
            ],
        );
    });
    ui.add_space(20.0);
    section_page::sub_heading(ui, c, "Workflow slots");
    section(ui, c, &daemon.slots, |ui, slots| {
        let mut rows = vec![("Evidence class", slots.evidence_class.clone())];
        rows.extend(slots.rows.iter().map(|slot| ("Slot", slot.clone())));
        section_page::value_card(ui, c, &rows);
    });
}

pub(super) fn decisions(ui: &mut Ui, c: &C, view: &ConsoleView<'_>, actions: &mut ConsoleActions) {
    let Some(daemon) = feed(ui, c, view, actions) else {
        return;
    };
    section(ui, c, &daemon.decisions, |ui, decisions| {
        if decisions.is_empty() {
            notice(ui, c, "No decisions are recorded.");
            return;
        }
        section_page::card(ui, c, |ui| {
            for (index, decision) in decisions.iter().enumerate() {
                if index > 0 {
                    section_page::separator(ui, c);
                }
                let description = format!(
                    "chose {} · {} · {}",
                    decision.chosen, decision.source, decision.created
                );
                let title = format!("{} · {}", decision.decision_id, decision.status);
                section_page::row(ui, c, &title, &description, |ui| {
                    if decision.consumed {
                        ui.label(RichText::new("used").size(12.0).color(c.muted));
                    }
                });
            }
        });
        ui.add_space(8.0);
        section_page::footnote(
            ui,
            c,
            "Decisions are advice; a decided dispatch still passes every dispatch gate.",
        );
    });
}

/// The feed's newest observation under its status line, or a notice that
/// explains why there is none.
fn feed<'a>(
    ui: &mut Ui,
    c: &C,
    view: &'a ConsoleView<'_>,
    actions: &mut ConsoleActions,
) -> Option<&'a DaemonView> {
    if view.folder.is_none() {
        notice(ui, c, "Open a folder to see its daemon.");
        return None;
    }
    let Some(daemon) = view.data.daemon.as_deref() else {
        if toolbar(ui, c, "Connecting to the daemon…", "Refresh", false) {
            actions.refresh = true;
        }
        ui.spinner();
        return None;
    };
    if toolbar(ui, c, &status_line(daemon), "Refresh", true) {
        actions.refresh = true;
    }
    Some(daemon)
}

pub(super) fn status_line(daemon: &DaemonView) -> String {
    match &daemon.observed_at {
        Some(observed_at) => format!("Daemon {} · read {observed_at}", daemon.connection),
        None => format!("Daemon: {}", daemon.connection),
    }
}

/// Draws `body` for a loaded section, a spinner while it is read, and the
/// reason when it is unavailable.
fn section<T>(ui: &mut Ui, c: &C, section: &Section<T>, body: impl FnOnce(&mut Ui, &T)) {
    match section {
        Section::Waiting => {
            ui.spinner();
        }
        Section::Unavailable(reason) => {
            notice(
                ui,
                c,
                &format!("Unavailable: {} ({}).", reason.hint, reason.code),
            );
        }
        Section::Loaded(value) => body(ui, value),
    }
}

fn lines_card(ui: &mut Ui, c: &C, heading: &str, lines: &[String]) {
    section_page::sub_heading(ui, c, &format!("{heading} ({})", lines.len()));
    if lines.is_empty() {
        notice(ui, c, "None.");
        return;
    }
    section_page::card(ui, c, |ui| {
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                section_page::separator(ui, c);
            }
            ui.add_space(8.0);
            section_page::footnote(ui, c, line);
            ui.add_space(8.0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_names_the_connection_and_read_time() {
        let mut daemon = DaemonView::waiting();
        assert_eq!(status_line(&daemon), "Daemon: connecting to the daemon");
        daemon.connection = "connected".into();
        daemon.observed_at = Some("12:00:01".into());
        assert_eq!(status_line(&daemon), "Daemon connected · read 12:00:01");
    }
}
