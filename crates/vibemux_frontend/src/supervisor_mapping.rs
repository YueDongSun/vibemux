//! Explicit conversion from sanitized control responses to display-only state.

use crate::supervisor_model::*;
use vibemux_harness::terminal_text::display_character;
use vibemux_types::frontend::{FrontendTaskDetail, FrontendTaskSummary};
use vibemuxd::terminal_observer::{TerminalBinding, TerminalSnapshot};

/// One display line of the daemon's free text. The domain checks a task title,
/// harness, role, branch, or worktree path only for blankness and size, and an
/// artifact media type only for control characters, so a line break, tab, or
/// bidirectional control would otherwise split a row or reorder what follows
/// it in every view that shows the task.
fn display_line(value: &str) -> String {
    value.chars().map(display_character).collect()
}

pub fn task_summary(source: FrontendTaskSummary) -> TaskView {
    TaskView {
        task_id: source.task_id.to_string(),
        title: format!(
            "{}{}",
            display_line(&source.title),
            if source.title_truncated { "…" } else { "" }
        ),
        state: source.status.as_str().into(),
        latest_sequence: source.latest_sequence,
        // A summary alone carries no runs or events, so the view is explicitly
        // incomplete until a detail read fills it in.
        detail_revision: source.detail_revision,
        needs_detail_refresh: true,
        executor: "No execution details loaded".into(),
        latest_update: "Task observed in the daemon".into(),
        ..Default::default()
    }
}

pub fn task_detail(source: FrontendTaskDetail, previous: Option<&TaskView>) -> TaskView {
    let mut task = task_summary(source.task);
    task.needs_detail_refresh = false;
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
                harness: display_line(&run.harness),
                role: display_line(&run.role),
                state: run.status.as_str().into(),
                worktree: format!(
                    "{}{}",
                    run.worktree_path
                        .as_deref()
                        .map_or_else(|| "Not recorded".into(), display_line),
                    if run.worktree_path_truncated {
                        "… (display truncated)"
                    } else {
                        ""
                    }
                ),
                branch: run.branch.as_deref().map(display_line).unwrap_or_default(),
                terminal,
                artifacts: source
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.run_id.to_string() == run_id)
                    .map(|artifact| {
                        let media_type = display_line(&artifact.media_type);
                        ArtifactView {
                            artifact_id: artifact.artifact_id.to_string(),
                            name: artifact.artifact_id.to_string(),
                            kind: media_type.clone(),
                            state: "Recorded reference".into(),
                            sha256: Some(artifact.sha256.clone()),
                            size_bytes: Some(artifact.size_bytes),
                            media_type: Some(media_type),
                        }
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
            display_name: pane_display_name(&pane.pane_id, &pane.window_id, &pane.workspace),
        })
        .collect();
    if terminal.binding_id.is_none() && terminal.candidates.is_empty() {
        terminal.status = "No native pane matches this Run's worktree".into();
    }
    terminal
}

/// A terminal candidate's caption. The plugin protocol refuses control
/// characters in a WezTerm workspace name, but not bidirectional controls or
/// line and paragraph separators, so the name passes through the display rule
/// too. A blank workspace leaves no trailing separator.
fn pane_display_name(pane_id: &str, window_id: &str, workspace: &str) -> String {
    let workspace = display_line(workspace);
    let caption = format!("Pane {pane_id} · window {window_id}");
    if workspace.trim().is_empty() {
        caption
    } else {
        format!("{caption} · {workspace}")
    }
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

    #[test]
    fn a_terminal_candidate_caption_is_one_display_line() {
        let mut snapshot = TerminalSnapshot {
            inventory: Default::default(),
            binding: None,
            checked_at_epoch_ms: 0,
        };
        for (pane_id, workspace) in [
            ("3", "dev\u{202e}txt.exe\u{2028}x"),
            ("4", ""),
            ("5", "\u{2066}\u{2029}"),
            ("6", "default"),
        ] {
            snapshot.inventory.panes.push(Default::default());
            let pane = snapshot.inventory.panes.last_mut().expect("pushed");
            pane.pane_id = pane_id.into();
            pane.window_id = "1".into();
            pane.workspace = workspace.into();
        }
        let view = terminal_snapshot(snapshot, "wezterm_observer");
        let captions: Vec<&str> = view
            .candidates
            .iter()
            .map(|candidate| candidate.display_name.as_str())
            .collect();
        assert_eq!(
            captions,
            [
                "Pane 3 · window 1 · dev txt.exe x",
                "Pane 4 · window 1",
                "Pane 5 · window 1",
                "Pane 6 · window 1 · default",
            ]
        );
        // The identifiers a link request echoes back stay as reported.
        assert_eq!(view.candidates[0].pane_id, "3");
    }

    fn detail(
        title: &str,
        title_truncated: bool,
        harness: &str,
        role: &str,
        worktree_path: Option<&str>,
        branch: Option<&str>,
    ) -> FrontendTaskDetail {
        use vibemux_types::{
            ProjectId, RunId, RunStatus, TaskId, TaskStatus,
            frontend::{FrontendRunSummary, FrontendTaskSummary},
        };
        let task_id = TaskId::new();
        FrontendTaskDetail {
            task: FrontendTaskSummary {
                task_id,
                project_id: ProjectId::new(),
                title: title.into(),
                title_truncated,
                status: TaskStatus::InProgress,
                latest_sequence: 1,
                detail_revision: 1,
            },
            runs: vec![FrontendRunSummary {
                run_id: RunId::new(),
                task_id,
                harness: harness.into(),
                role: role.into(),
                status: RunStatus::Running,
                worktree_path: worktree_path.map(Into::into),
                worktree_path_truncated: false,
                branch: branch.map(Into::into),
            }],
            has_more_runs: false,
            events: Vec::new(),
            has_more_events: false,
            artifacts: Vec::new(),
            has_more_artifacts: false,
        }
    }

    #[test]
    fn task_and_run_text_becomes_one_display_line() {
        let view = task_detail(
            detail(
                "Fix\nthe\tparser\u{202e}gpj.exe",
                true,
                "co\rdex",
                "wor\u{2066}ker",
                Some("C:\\work\u{2028}tree"),
                Some("feat/\u{200f}x\ny"),
            ),
            None,
        );
        assert_eq!(view.title, "Fix the parser gpj.exe…");
        let run = &view.runs[0];
        assert_eq!(
            (run.harness.as_str(), run.role.as_str()),
            ("co dex", "wor ker")
        );
        assert_eq!(run.worktree, "C:\\work tree");
        assert_eq!(run.branch, "feat/ x y");
        assert_eq!(view.executor, "co dex · wor ker");

        // Ordinary text, including non-Latin script, passes through unchanged,
        // and absent fields keep their placeholders.
        let view = task_detail(
            detail("修复解析器", false, "claude", "worker", None, None),
            None,
        );
        assert_eq!(view.title, "修复解析器");
        let run = &view.runs[0];
        assert_eq!(
            (run.harness.as_str(), run.role.as_str()),
            ("claude", "worker")
        );
        assert_eq!(run.worktree, "Not recorded");
        assert_eq!(run.branch, "");
    }

    #[test]
    fn an_artifact_media_type_becomes_one_display_line() {
        use vibemux_types::{ArtifactId, frontend::FrontendArtifactSummary};
        let mut source = detail("Task", false, "codex", "worker", None, None);
        let run_id = source.runs[0].run_id;
        // The A2A artifact rule refuses control characters but not a line
        // separator or a right-to-left override.
        for media_type in ["text/plain\u{2028}\u{202e}lmth", "application/json"] {
            source.artifacts.push(FrontendArtifactSummary {
                run_id,
                artifact_id: ArtifactId::new(),
                sha256: "0".repeat(64),
                size_bytes: 1,
                media_type: media_type.into(),
            });
        }
        let view = task_detail(source, None);
        let types: Vec<(&str, Option<&str>)> = view.runs[0]
            .artifacts
            .iter()
            .map(|artifact| (artifact.kind.as_str(), artifact.media_type.as_deref()))
            .collect();
        assert_eq!(
            types,
            [
                ("text/plain  lmth", Some("text/plain  lmth")),
                ("application/json", Some("application/json")),
            ]
        );
    }
}
