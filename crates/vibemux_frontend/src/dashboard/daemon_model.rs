//! Content-free projection of daemon reads for the console. Pure: the feed
//! does the I/O and hands the results here. Every string that leaves this
//! module went through `safe_text`.
use serde::Serialize;
use vibemux_harness::dispatch::route_config::DispatchCatalogEntry;
use vibemux_types::frontend::{FrontendTaskDetail, FrontendTasksPage};
use vibemuxd::{
    control::{DaemonHealth, DecisionView, DshSessionStatus},
    plugin_registry::PluginStatus,
};

use super::safe_text;

/// Decision rows the console keeps.
pub const MAX_DECISIONS: usize = 64;
/// Characters of an id shown in a row.
const SHORT_ID_CHARS: usize = 8;

/// One daemon read as the console sees it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Section<T> {
    /// Not read yet.
    Waiting,
    Loaded(T),
    Unavailable(Unavailable),
}

/// Why a read has no data, and what the operator can do about it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unavailable {
    pub code: String,
    pub hint: String,
}

/// The operator hint for a daemon error code.
#[must_use]
pub fn unavailable(code: &str) -> Unavailable {
    let hint = match code {
        "daemon_not_running" => "start it with vibemuxctl daemon start",
        "control_unsupported_version" => "unsupported by this daemon",
        "harness_dispatch_unconfigured"
        | "dsh_session_unconfigured"
        | "workflow_unconfigured"
        | "decision_not_configured" => "not configured",
        "dsh_session_not_found" => "no DSH session is open",
        // A descriptor whose endpoint is gone: a crashed or hung daemon.
        "control_endpoint_unavailable"
        | "control_deadline_exceeded"
        | "control_server_terminated" => {
            "the daemon is not answering; check vibemuxctl daemon inspect"
        }
        _ => "see vibemuxctl daemon health",
    };
    Unavailable {
        code: safe_text(code),
        hint: hint.to_string(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthView {
    pub status: String,
    pub schema: String,
    pub queue: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginRow {
    pub plugin_id: String,
    pub state: String,
    pub restarts: String,
    pub last_error: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteRow {
    pub harness: String,
    pub protocol: String,
    /// `executes`, `probe only`, or `disabled`.
    pub mode: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskRow {
    pub task_id: String,
    pub title: String,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskList {
    pub rows: Vec<TaskRow>,
    /// Whether the daemon has more tasks than the first page.
    pub more: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskDetailView {
    pub task_id: String,
    pub title: String,
    pub runs: Vec<String>,
    pub events: Vec<String>,
    pub artifacts: Vec<String>,
    /// Whether the daemon cut any of the three lists.
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DshView {
    pub phase: String,
    pub provider: String,
    pub model: String,
    pub turns: String,
    pub transport_open: bool,
}

impl DshView {
    /// `turns` is started, completed, failed, cancelled, unverified.
    #[must_use]
    pub fn new(
        phase: &str,
        provider: &str,
        model: &str,
        turns: [u64; 5],
        transport_open: bool,
    ) -> Self {
        let [started, completed, failed, cancelled, unverified] = turns;
        Self {
            phase: safe_text(phase),
            provider: safe_text(provider),
            model: safe_text(model),
            turns: format!(
                "{started} started, {completed} completed, {failed} failed, {cancelled} cancelled, {unverified} unverified"
            ),
            transport_open,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlotsView {
    pub evidence_class: String,
    pub rows: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionRow {
    pub decision_id: String,
    /// The full id, which merging matches on.
    full_id: String,
    pub status: String,
    pub chosen: String,
    pub source: String,
    pub created: String,
    pub consumed: bool,
    /// The list cursor of the recording event.
    pub created_sequence: u64,
}

impl DecisionRow {
    #[must_use]
    pub fn new(
        decision_id: &str,
        status: &str,
        chosen: Option<&str>,
        source: &str,
        created: &str,
        consumed: bool,
        created_sequence: u64,
    ) -> Self {
        let full_id = safe_text(decision_id);
        Self {
            decision_id: full_id.chars().take(SHORT_ID_CHARS).collect(),
            full_id,
            status: safe_text(status),
            chosen: chosen.map_or_else(|| "-".to_string(), safe_text),
            source: safe_text(source),
            created: safe_text(created),
            consumed,
            created_sequence,
        }
    }
}

/// Everything the console shows about the daemon, at one observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonView {
    /// One line: connected, or why not.
    pub connection: String,
    pub observed_at: Option<String>,
    pub health: Section<HealthView>,
    pub plugins: Section<Vec<PluginRow>>,
    pub routes: Section<Vec<RouteRow>>,
    pub tasks: Section<TaskList>,
    /// The task opened with Enter, if any.
    pub task_detail: Option<Section<TaskDetailView>>,
    pub dsh: Section<DshView>,
    pub slots: Section<SlotsView>,
    /// Newest first.
    pub decisions: Section<Vec<DecisionRow>>,
}

impl DaemonView {
    #[must_use]
    pub fn waiting() -> Self {
        Self {
            connection: "connecting to the daemon".to_string(),
            observed_at: None,
            health: Section::Waiting,
            plugins: Section::Waiting,
            routes: Section::Waiting,
            tasks: Section::Waiting,
            task_detail: None,
            dsh: Section::Waiting,
            slots: Section::Waiting,
            decisions: Section::Waiting,
        }
    }

    /// Every section unavailable for one connection failure.
    #[must_use]
    pub fn not_running(code: &str) -> Self {
        let reason = unavailable(code);

        Self {
            connection: format!("{} - {}", reason.code, reason.hint),
            observed_at: None,
            health: Section::Unavailable(reason.clone()),
            plugins: Section::Unavailable(reason.clone()),
            routes: Section::Unavailable(reason.clone()),
            tasks: Section::Unavailable(reason.clone()),
            task_detail: None,
            dsh: Section::Unavailable(reason.clone()),
            slots: Section::Unavailable(reason.clone()),
            decisions: Section::Unavailable(reason.clone()),
        }
    }
}

/// The snake_case wire name of a serde enum, for enums without `as_str`.
fn wire_name(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(safe_text))
        .unwrap_or_else(|| "unknown".to_string())
}

#[must_use]
pub fn project_health(health: &DaemonHealth) -> HealthView {
    HealthView {
        status: if health.healthy {
            "healthy".to_string()
        } else {
            "unhealthy".to_string()
        },
        schema: health.store_schema_version.to_string(),
        queue: format!(
            "{}/{} (peak {}){}",
            health.queue_depth,
            health.queue_capacity,
            health.queue_high_watermark,
            if health.queue_saturated {
                " saturated"
            } else {
                ""
            }
        ),
    }
}

/// Plugin rows without session ids or process ids.
#[must_use]
pub fn project_plugins(plugins: &[PluginStatus]) -> Vec<PluginRow> {
    plugins
        .iter()
        .map(|plugin| PluginRow {
            plugin_id: safe_text(&plugin.plugin_id),
            state: wire_name(&plugin.state),
            restarts: format!("{}/{}", plugin.restarts, plugin.max_restarts),
            last_error: plugin
                .last_error_code
                .as_deref()
                .map_or_else(|| "-".to_string(), safe_text),
        })
        .collect()
}

#[must_use]
pub fn project_routes(routes: &[DispatchCatalogEntry]) -> Vec<RouteRow> {
    routes
        .iter()
        .map(|route| RouteRow {
            harness: route.harness.display_name().to_string(),
            protocol: route.protocol.as_str().to_string(),
            mode: if !route.enabled {
                "disabled"
            } else if route.allow_execution {
                "executes"
            } else {
                "probe only"
            }
            .to_string(),
        })
        .collect()
}

#[must_use]
pub fn project_tasks(page: &FrontendTasksPage) -> TaskList {
    TaskList {
        rows: page
            .tasks
            .iter()
            .map(|task| TaskRow {
                task_id: task.task_id.to_string(),
                title: safe_text(&task.title),
                status: task.status.as_str().to_string(),
            })
            .collect(),
        more: page.next_cursor.is_some(),
    }
}

/// Runs, events, and artifacts as one line each, without worktree paths,
/// branches, or digests.
#[must_use]
pub fn project_task_detail(detail: &FrontendTaskDetail) -> TaskDetailView {
    TaskDetailView {
        task_id: detail.task.task_id.to_string(),
        title: safe_text(&detail.task.title),
        runs: detail
            .runs
            .iter()
            .map(|run| {
                format!(
                    "{} {} {} {}",
                    short_id(&run.run_id.to_string()),
                    safe_text(&run.harness),
                    safe_text(&run.role),
                    run.status.as_str()
                )
            })
            .collect(),
        events: detail
            .events
            .iter()
            .map(|event| {
                format!(
                    "#{} {} {}",
                    event.sequence,
                    safe_text(&event.event_type),
                    safe_text(&event.at)
                )
            })
            .collect(),
        artifacts: detail
            .artifacts
            .iter()
            .map(|artifact| {
                format!(
                    "{} {} {} bytes",
                    short_id(&artifact.artifact_id.to_string()),
                    safe_text(&artifact.media_type),
                    artifact.size_bytes
                )
            })
            .collect(),
        truncated: detail.has_more_runs || detail.has_more_events || detail.has_more_artifacts,
    }
}

fn short_id(id: &str) -> String {
    // Typed ids carry a prefix (`run_`, `artifact_`); keep it readable.
    let id = safe_text(id);
    let start = id.find('_').map_or(0, |index| index + 1);
    id.chars().skip(start).take(SHORT_ID_CHARS).collect()
}

#[must_use]
pub fn project_dsh(status: &DshSessionStatus) -> DshView {
    DshView::new(
        status.phase.as_str(),
        &status.provider,
        &status.model,
        [
            status.turns_started,
            status.completed_turns,
            status.failed_turns,
            status.cancelled_turns,
            status.unverified_turns,
        ],
        status.transport_open,
    )
}

/// Reads only the evidence class and each route's slot id, route label, and
/// execution flag from the `workflow_slots` payload.
#[must_use]
pub fn project_slots(value: &serde_json::Value) -> SlotsView {
    let text =
        |value: &serde_json::Value| value.as_str().map_or_else(|| "-".to_string(), safe_text);
    SlotsView {
        evidence_class: text(&value["evidence_class"]),
        rows: value["routes"]
            .as_array()
            .map(|routes| {
                routes
                    .iter()
                    .map(|route| {
                        format!(
                            "{} {} {}",
                            text(&route["slot_id"]),
                            text(&route["route_label"]),
                            if route["executes"].as_bool() == Some(true) {
                                "executes"
                            } else {
                                "probe only"
                            }
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[must_use]
pub fn project_decision(view: &DecisionView) -> DecisionRow {
    let created = view
        .created_at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "-".to_string());
    DecisionRow::new(
        &view.decision_id.to_string(),
        &wire_name(&view.status),
        view.chosen_harness.map(|harness| harness.display_name()),
        &wire_name(&view.source),
        &created,
        view.consumed_at.is_some(),
        view.created_sequence,
    )
}

/// Replaces kept rows by id with their re-read versions, adds new ones, and
/// keeps the newest `limit`, newest first. A decision can be consumed after
/// it is recorded, so a re-read row always wins.
pub fn merge_decisions(kept: &mut Vec<DecisionRow>, read: Vec<DecisionRow>, limit: usize) {
    for row in read {
        match kept.iter_mut().find(|old| old.full_id == row.full_id) {
            Some(old) => *old = row,
            None => kept.push(row),
        }
    }
    kept.sort_by(|left, right| right.created_sequence.cmp(&left.created_sequence));
    kept.truncate(limit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_types::{
        ProjectId, RunId, RunStatus, TaskId, TaskStatus,
        frontend::{FrontendEventSummary, FrontendRunSummary, FrontendTaskSummary},
    };
    use vibemuxd::plugin_registry::PluginState;

    #[test]
    fn unavailable_codes_get_operator_hints() {
        for (code, hint) in [
            (
                "daemon_not_running",
                "start it with vibemuxctl daemon start",
            ),
            ("control_unsupported_version", "unsupported by this daemon"),
            ("harness_dispatch_unconfigured", "not configured"),
            ("dsh_session_unconfigured", "not configured"),
            ("workflow_unconfigured", "not configured"),
            ("decision_not_configured", "not configured"),
            ("dsh_session_not_found", "no DSH session is open"),
            (
                "control_endpoint_unavailable",
                "the daemon is not answering; check vibemuxctl daemon inspect",
            ),
            (
                "control_deadline_exceeded",
                "the daemon is not answering; check vibemuxctl daemon inspect",
            ),
            ("control_timeout", "see vibemuxctl daemon health"),
        ] {
            let unavailable = unavailable(code);
            assert_eq!(unavailable.code, code);
            assert_eq!(unavailable.hint, hint, "{code}");
        }
        let hostile = unavailable("bad\u{1b}[2Jcode");
        assert!(!hostile.code.contains('\u{1b}'));
    }

    fn summary(title: &str) -> FrontendTaskSummary {
        FrontendTaskSummary {
            task_id: TaskId::new(),
            project_id: ProjectId::new(),
            title: title.to_string(),
            title_truncated: false,
            status: TaskStatus::InProgress,
            latest_sequence: 1,
            detail_revision: 1,
        }
    }

    #[test]
    fn daemon_strings_are_sanitized() {
        let page = FrontendTasksPage {
            project_id: None,
            tasks: vec![summary("evil\u{1b}[31m\u{202e}title")],
            next_cursor: None,
        };
        let tasks = project_tasks(&page);
        assert!(!tasks.rows[0].title.contains('\u{1b}'));
        assert!(!tasks.rows[0].title.contains('\u{202e}'));
        assert_eq!(tasks.rows[0].status, "in_progress");

        let plugins = project_plugins(&[PluginStatus {
            plugin_id: "observer\u{7}".to_string(),
            state: PluginState::Backoff,
            restarts: 2,
            max_restarts: 5,
            session_id: Some("secret_session".to_string()),
            process_id: Some(42),
            last_error_code: Some("crash\u{1b}]0;title\u{7}".to_string()),
            graceful_shutdown: None,
        }]);
        let text = format!("{plugins:?}");
        assert!(!text.contains('\u{1b}') && !text.contains('\u{7}'));
        assert!(!text.contains("secret_session"));
        assert_eq!(plugins[0].state, "backoff");
        assert_eq!(plugins[0].restarts, "2/5");

        let dsh = DshView::new(
            "ready",
            "deep\u{1b}seek",
            "model\u{202e}",
            [1, 1, 0, 0, 0],
            true,
        );
        assert!(!format!("{dsh:?}").contains('\u{1b}'));
        let decision = DecisionRow::new(
            "0123456789abcdef",
            "resolved",
            Some("Codex"),
            "local_rule\u{1b}",
            "2026-10-21T10:00:00Z",
            false,
            7,
        );
        assert_eq!(decision.decision_id, "01234567");
        assert!(!decision.source.contains('\u{1b}'));
    }

    #[test]
    fn slots_projection_reads_only_route_fields() {
        let value = serde_json::json!({
            "evidence_class": "fixture",
            "slots": [{"slot_id": "a", "prompt": "secret prompt"}],
            "routes": [
                {"slot_id": "track_a", "route_label": "codex", "executes": true, "prompt": "secret"},
                {"slot_id": "track_b", "route_label": "claude", "executes": false}
            ],
            "writable": {"eligible": []}
        });
        let slots = project_slots(&value);
        assert_eq!(slots.evidence_class, "fixture");
        assert_eq!(
            slots.rows,
            ["track_a codex executes", "track_b claude probe only"]
        );
        assert!(!format!("{slots:?}").contains("secret"));
    }

    #[test]
    fn task_detail_marks_truncation() {
        let task = summary("alpha");
        let run_id = RunId::new();
        let detail = FrontendTaskDetail {
            task: task.clone(),
            runs: vec![FrontendRunSummary {
                run_id,
                task_id: task.task_id,
                harness: "codex".to_string(),
                role: "implementer".to_string(),
                status: RunStatus::Running,
                worktree_path: Some("C:/private/path".to_string()),
                worktree_path_truncated: false,
                branch: Some("private_branch".to_string()),
            }],
            has_more_runs: false,
            events: vec![FrontendEventSummary {
                sequence: 3,
                event_type: "run_started".to_string(),
                at: "2026-10-21T10:00:00Z".to_string(),
                run_id: Some(run_id),
            }],
            has_more_events: true,
            artifacts: Vec::new(),
            has_more_artifacts: false,
        };
        let view = project_task_detail(&detail);
        assert!(view.truncated);
        assert_eq!(view.title, "alpha");
        assert!(view.runs[0].contains("codex") && view.runs[0].contains("running"));
        assert!(view.events[0].contains("run_started"));
        let text = format!("{view:?}");
        assert!(!text.contains("C:/private/path") && !text.contains("private_branch"));
    }

    #[test]
    fn health_and_routes_project_to_labels() {
        let health = project_health(&DaemonHealth {
            healthy: true,
            process_id: 99,
            store_schema_version: 9,
            queue_capacity: 64,
            queue_depth: 1,
            queue_high_watermark: 3,
            queue_saturated: false,
        });
        assert_eq!(health.status, "healthy");
        assert_eq!(health.schema, "9");
        assert_eq!(health.queue, "1/64 (peak 3)");
        let routes = project_routes(&[DispatchCatalogEntry {
            harness: vibemux_harness::AgentKind::Codex,
            protocol: vibemux_harness::dispatch::NativeProtocol::CodexExec,
            enabled: true,
            allow_execution: true,
            probe_supported: true,
        }]);
        assert_eq!(routes[0].harness, "Codex");
        assert_eq!(routes[0].protocol, "codex_exec");
        assert_eq!(routes[0].mode, "executes");
    }

    #[test]
    fn decisions_merge_by_id_and_keep_the_newest() {
        let row = |id: &str, sequence: u64, consumed: bool| {
            DecisionRow::new(id, "resolved", None, "local_rule", "t", consumed, sequence)
        };
        let mut kept = vec![row("aaaaaaaa1", 1, false), row("bbbbbbbb2", 2, false)];
        merge_decisions(
            &mut kept,
            vec![row("bbbbbbbb2", 2, true), row("cccccccc3", 3, false)],
            2,
        );
        let ids: Vec<_> = kept.iter().map(|row| row.decision_id.as_str()).collect();
        // Newest first, the consumed update replaced the old row, and the
        // oldest fell out of the bound.
        assert_eq!(ids, ["cccccccc", "bbbbbbbb"]);
        assert!(kept[1].consumed);
    }
}
