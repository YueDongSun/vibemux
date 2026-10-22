#![forbid(unsafe_code)]
//! Overview: one card per area (folder, harnesses, workspace, daemon) with
//! a link to its section.

use eframe::egui::Ui;
use vibemux_probe::ProbeState;

use crate::dashboard::{DaemonView, Section};

use super::{
    super::{C, design, section_page},
    ConsoleActions, ConsoleSection, ConsoleView, WorkspaceLoad, daemon_views, workspace_view,
};

pub(super) fn render(ui: &mut Ui, c: &C, view: &ConsoleView<'_>, actions: &mut ConsoleActions) {
    let folder = view.folder.map_or_else(
        || "No folder is open".to_string(),
        |folder| folder.display().to_string(),
    );
    let connection = &view.snapshot.connection;
    let mut daemon_text = connection.status.label().to_string();
    if let Some(detail) = connection.detail.as_deref() {
        daemon_text.push_str(" · ");
        daemon_text.push_str(detail);
    }
    area(
        ui,
        c,
        "This folder",
        &[("Folder", folder), ("VibeMux", daemon_text)],
        None,
        actions,
    );
    area(
        ui,
        c,
        "Harnesses",
        &harness_rows(view),
        Some(ConsoleSection::Harnesses),
        actions,
    );
    let workspace_rows = match &view.data.workspace {
        WorkspaceLoad::Loaded(workspace) => workspace_view::summary_rows(workspace),
        WorkspaceLoad::Failed(code) => vec![("Git", workspace_view::failure_text(code))],
        WorkspaceLoad::NotLoaded | WorkspaceLoad::Loading if view.folder.is_some() => {
            vec![("Git", "Reading…".to_string())]
        }
        WorkspaceLoad::NotLoaded | WorkspaceLoad::Loading => {
            vec![("Git", "Open a folder first".to_string())]
        }
    };
    area(
        ui,
        c,
        "Workspace",
        &workspace_rows,
        Some(ConsoleSection::Workspace),
        actions,
    );
    let daemon_rows = view
        .data
        .daemon
        .as_deref()
        .map_or_else(|| vec![("Daemon", "Connecting…".to_string())], daemon_rows);
    area(
        ui,
        c,
        "Daemon",
        &daemon_rows,
        Some(ConsoleSection::Daemon),
        actions,
    );
}

fn area(
    ui: &mut Ui,
    c: &C,
    title: &str,
    rows: &[(&str, String)],
    link: Option<ConsoleSection>,
    actions: &mut ConsoleActions,
) {
    ui.horizontal(|ui| {
        section_page::sub_heading(ui, c, title);
        if let Some(section) = link
            && design::quiet_button(ui, c, &format!("Open {}", section.label())).clicked()
        {
            actions.section = Some(section);
        }
    });
    section_page::value_card(ui, c, rows);
    ui.add_space(20.0);
}

pub(super) fn harness_rows(view: &ConsoleView<'_>) -> Vec<(&'static str, String)> {
    let agents = &view.view_model.agents;
    let detected = agents
        .iter()
        .filter(|agent| agent.launcher_state == ProbeState::Verified)
        .count();
    let ask = if view.ask_ready.is_empty() {
        "Not set up in this folder".to_string()
    } else {
        view.ask_ready.join(", ")
    };
    vec![
        ("Detected", format!("{detected} of {}", agents.len())),
        ("Ask ready", ask),
        ("Probed", view.view_model.observed_at.clone()),
    ]
}

fn daemon_rows(daemon: &DaemonView) -> Vec<(&'static str, String)> {
    let count = |section: &Section<usize>| match section {
        Section::Waiting => "Reading…".to_string(),
        Section::Loaded(count) => count.to_string(),
        Section::Unavailable(reason) => reason.hint.clone(),
    };
    let health = match &daemon.health {
        Section::Loaded(health) => health.status.clone(),
        Section::Waiting => "Reading…".to_string(),
        Section::Unavailable(reason) => reason.hint.clone(),
    };
    let dsh = match &daemon.dsh {
        Section::Loaded(dsh) => dsh.phase.clone(),
        Section::Waiting => "Reading…".to_string(),
        Section::Unavailable(reason) => reason.hint.clone(),
    };
    vec![
        ("Status", daemon_views::status_line(daemon)),
        ("Health", health),
        ("Routes", count(&map_section(&daemon.routes, Vec::len))),
        (
            "Tasks",
            count(&map_section(&daemon.tasks, |list| list.rows.len())),
        ),
        ("DSH session", dsh),
        (
            "Decisions",
            count(&map_section(&daemon.decisions, Vec::len)),
        ),
    ]
}

fn map_section<T, U>(section: &Section<T>, map: impl FnOnce(&T) -> U) -> Section<U> {
    match section {
        Section::Waiting => Section::Waiting,
        Section::Loaded(value) => Section::Loaded(map(value)),
        Section::Unavailable(reason) => Section::Unavailable(reason.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::daemon_model::{HealthView, unavailable};

    #[test]
    fn daemon_rows_count_loaded_sections_and_explain_the_rest() {
        let mut daemon = DaemonView::waiting();
        daemon.connection = "connected".into();
        daemon.health = Section::Loaded(HealthView {
            status: "healthy".into(),
            schema: "9".into(),
            queue: "0 of 64".into(),
        });
        daemon.routes = Section::Loaded(Vec::new());
        daemon.dsh = Section::Unavailable(unavailable("dsh_session_unconfigured"));
        let rows = daemon_rows(&daemon);
        assert_eq!(rows[1], ("Health", "healthy".to_string()));
        assert_eq!(rows[2], ("Routes", "0".to_string()));
        assert_eq!(rows[3], ("Tasks", "Reading…".to_string()));
        assert_eq!(rows[4], ("DSH session", "not configured".to_string()));
    }
}
