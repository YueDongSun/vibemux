//! Explicit conversion from sanitized control responses to display-only state.

use crate::supervisor_model::*;
use vibemux_types::frontend::{FrontendTaskDetail, FrontendTaskSummary};
use vibemuxd::terminal_observer::{TerminalBinding, TerminalSnapshot};

pub fn task_summary(source: FrontendTaskSummary) -> TaskView {
    TaskView {
        task_id: source.task_id.to_string(),
        title: format!(
            "{}{}",
            source.title,
            if source.title_truncated { "…" } else { "" }
        ),
        state: source.status.as_str().into(),
        latest_sequence: source.latest_sequence,
        executor: "No execution details loaded".into(),
        latest_update: "Task observed in the daemon".into(),
        ..Default::default()
    }
}

pub fn task_detail(source: FrontendTaskDetail, previous: Option<&TaskView>) -> TaskView {
    let mut task = task_summary(source.task);
    task.has_more_runs = source.has_more_runs;
    task.has_more_artifacts = source.has_more_artifacts;
    task.has_more_events = source.has_more_events;
    task.recent_events = source
        .events
        .into_iter()
        .map(|event| TaskEventView {
            event_id: event.sequence.to_string(),
            sequence: event.sequence,
            summary: event.event_type.replace('_', " "),
            kind: event.event_type,
            occurred_at: event.at,
            run_id: event.run_id.map(|id| id.to_string()),
        })
        .collect();
    task.latest_update = task
        .recent_events
        .first()
        .map(|event| event.summary.clone())
        .unwrap_or_else(|| "No recorded activity".into());
    task.runs = source
        .runs
        .into_iter()
        .map(|run| {
            let run_id = run.run_id.to_string();
            let terminal = previous
                .and_then(|old| old.runs.iter().find(|candidate| candidate.run_id == run_id))
                .map(|run| run.terminal.clone())
                .unwrap_or_else(|| TerminalView {
                    status: "Not linked — inspect native terminals to associate a pane".into(),
                    ..Default::default()
                });
            RunView {
                run_id: run_id.clone(),
                harness: run.harness,
                role: run.role,
                state: run.status.as_str().into(),
                worktree: format!(
                    "{}{}",
                    run.worktree_path.unwrap_or_else(|| "Not recorded".into()),
                    if run.worktree_path_truncated {
                        "… (display truncated)"
                    } else {
                        ""
                    }
                ),
                branch: run.branch.unwrap_or_default(),
                terminal,
                artifacts: source
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.run_id.to_string() == run_id)
                    .map(|artifact| ArtifactView {
                        artifact_id: artifact.artifact_id.to_string(),
                        name: artifact.artifact_id.to_string(),
                        kind: artifact.media_type.clone(),
                        state: "Recorded reference".into(),
                        sha256: Some(artifact.sha256.clone()),
                        size_bytes: Some(artifact.size_bytes),
                        media_type: Some(artifact.media_type.clone()),
                    })
                    .collect(),
            }
        })
        .collect();
    task.executor = task
        .runs
        .first()
        .map(|run| format!("{} · {}", run.harness, run.role))
        .unwrap_or_else(|| "Unassigned".into());
    task
}

pub fn terminal_snapshot(source: TerminalSnapshot, plugin_id: &str) -> TerminalView {
    let mut terminal = source
        .binding
        .as_ref()
        .map(binding_view)
        .unwrap_or_else(|| TerminalView {
            status: "Native terminals inspected — select a pane to link".into(),
            ..Default::default()
        });
    terminal.checked_at = Some(format_epoch_ms(source.checked_at_epoch_ms));
    terminal.plugin_id = Some(plugin_id.to_owned());
    terminal.instance_id = Some(source.inventory.instance_id.clone());
    terminal.candidates = source
        .inventory
        .panes
        .into_iter()
        .map(|pane| TerminalCandidate {
            plugin_id: plugin_id.into(),
            pane_id: pane.pane_id.clone(),
            instance_id: source.inventory.instance_id.clone(),
            display_name: format!(
                "Pane {} · window {} · {}",
                pane.pane_id, pane.window_id, pane.workspace
            ),
        })
        .collect();
    if terminal.binding_id.is_none() && terminal.candidates.is_empty() {
        terminal.status = "No native pane matches this Run's worktree".into();
    }
    terminal
}

pub fn binding_view(binding: &TerminalBinding) -> TerminalView {
    TerminalView {
        status: "Linked terminal — observation only".into(),
        binding_id: Some(binding.binding_id.clone()),
        plugin_id: Some(binding.query.plugin_id.clone()),
        pane_id: Some(binding.pane.pane_id.clone()),
        instance_id: Some(binding.instance_id.clone()),
        ..Default::default()
    }
}

fn format_epoch_ms(value: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(value) * 1_000_000)
        .ok()
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "Unknown observation time".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_observation_is_explicitly_not_task_success() {
        let snapshot = TerminalSnapshot {
            inventory: Default::default(),
            binding: None,
            checked_at_epoch_ms: 0,
        };
        let view = terminal_snapshot(snapshot, "wezterm_observer");
        assert!(view.binding_id.is_none());
        assert!(!view.status.contains("succeeded"));
    }
}
