//! Content-free projection of the daemon reads behind the ADR 042 console
//! views (daemon, sessions, decisions) for the web API. Pure: the
//! `console_poller` does the I/O and hands the results here. Every
//! daemon-supplied string that leaves this module passed through
//! `content_label::web_label`, except the DSH fields, which the bridge's
//! `project_dsh_status` sanitizes; harness, protocol, and mode names are
//! static strings.

use serde::Serialize;
use vibemux_harness::dispatch::route_config::DispatchCatalogEntry;
use vibemuxd::{
    control::{DaemonHealth, DecisionView},
    plugin_registry::PluginStatus,
};

use crate::{content_label::web_label, control_bridge::BridgeDshStatus};

/// Decision rows the web console keeps, newest first.
pub const MAX_CONSOLE_DECISIONS: usize = 64;
/// Plugin, route, and slot rows per list; a longer list is marked truncated.
pub const MAX_CONSOLE_ROWS: usize = 64;

/// One daemon read as the web console sees it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConsoleSection<T> {
    /// Not read yet.
    Waiting,
    Loaded {
        data: T,
    },
    /// The read failed; `hint` tells the operator what to do.
    Unavailable {
        code: String,
        hint: String,
    },
}

/// Whether the web process reached the daemon on its last poll.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConsoleConnection {
    Waiting,
    Connected,
    Unavailable { code: String, hint: String },
}

/// A bounded list and whether the bound cut it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleList<T> {
    pub rows: Vec<T>,
    pub truncated: bool,
}

impl<T> ConsoleList<T> {
    fn bounded(rows: impl Iterator<Item = T>, limit: usize) -> Self {
        let mut rows: Vec<T> = rows.take(limit.saturating_add(1)).collect();
        let truncated = rows.len() > limit;
        rows.truncate(limit);
        Self { rows, truncated }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleHealth {
    pub healthy: bool,
    pub store_schema_version: u32,
    pub queue_depth: usize,
    pub queue_capacity: usize,
    pub queue_high_watermark: usize,
    pub queue_saturated: bool,
}

/// A plugin without its session id or process id.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsolePlugin {
    pub plugin_id: String,
    pub state: String,
    pub restarts: u32,
    pub max_restarts: u32,
    pub last_error_code: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleRoute {
    pub harness: String,
    pub protocol: String,
    /// `executes`, `probe_only`, or `disabled`.
    pub mode: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleSlotRoute {
    pub slot_id: String,
    pub route_label: String,
    pub executes: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleSlots {
    pub evidence_class: String,
    pub routes: ConsoleList<ConsoleSlotRoute>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleDecision {
    pub decision_id: String,
    pub status: String,
    pub chosen_harness: Option<String>,
    pub source: String,
    pub created_at: String,
    pub consumed: bool,
    /// The list cursor of the recording event.
    pub created_sequence: u64,
}

/// Everything the web console shows about the daemon, at one observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConsoleSnapshot {
    pub observed_at: Option<String>,
    pub connection: ConsoleConnection,
    pub health: ConsoleSection<ConsoleHealth>,
    pub plugins: ConsoleSection<ConsoleList<ConsolePlugin>>,
    pub routes: ConsoleSection<ConsoleList<ConsoleRoute>>,
    pub dsh: ConsoleSection<BridgeDshStatus>,
    pub slots: ConsoleSection<ConsoleSlots>,
    /// Newest first.
    pub decisions: ConsoleSection<ConsoleList<ConsoleDecision>>,
}

impl Default for ConsoleSnapshot {
    fn default() -> Self {
        Self::waiting()
    }
}

impl ConsoleSnapshot {
    #[must_use]
    pub fn waiting() -> Self {
        Self {
            observed_at: None,
            connection: ConsoleConnection::Waiting,
            health: ConsoleSection::Waiting,
            plugins: ConsoleSection::Waiting,
            routes: ConsoleSection::Waiting,
            dsh: ConsoleSection::Waiting,
            slots: ConsoleSection::Waiting,
            decisions: ConsoleSection::Waiting,
        }
    }

    /// Every section unavailable for one connection failure, so no earlier
    /// observation is shown as current.
    #[must_use]
    pub fn not_running(code: &str, observed_at: Option<String>) -> Self {
        Self {
            observed_at,
            connection: ConsoleConnection::Unavailable {
                code: web_label(code),
                hint: operator_hint(code).to_owned(),
            },
            health: unavailable(code),
            plugins: unavailable(code),
            routes: unavailable(code),
            dsh: unavailable(code),
            slots: unavailable(code),
            decisions: unavailable(code),
        }
    }
}

/// The operator hint for a daemon or bridge error code; the same wording as
/// the terminal console where the code is shared.
#[must_use]
pub fn operator_hint(code: &str) -> &'static str {
    match code {
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
        | "control_server_terminated"
        | "bridge_deadline_exceeded" => {
            "the daemon is not answering; check vibemuxctl daemon inspect"
        }
        "daemon_unavailable" | "daemon_generation_changed" => {
            "the daemon descriptor could not be verified; check vibemuxctl daemon inspect"
        }
        _ => "see vibemuxctl daemon health",
    }
}

#[must_use]
pub fn unavailable<T>(code: &str) -> ConsoleSection<T> {
    ConsoleSection::Unavailable {
        code: web_label(code),
        hint: operator_hint(code).to_owned(),
    }
}

/// The snake_case wire name of a serde enum, for enums without `as_str`.
fn wire_name(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(web_label))
        .unwrap_or_else(|| "unknown".to_owned())
}

#[must_use]
pub fn project_health(health: &DaemonHealth) -> ConsoleHealth {
    ConsoleHealth {
        healthy: health.healthy,
        store_schema_version: health.store_schema_version,
        queue_depth: health.queue_depth,
        queue_capacity: health.queue_capacity,
        queue_high_watermark: health.queue_high_watermark,
        queue_saturated: health.queue_saturated,
    }
}

#[must_use]
pub fn project_plugins(plugins: &[PluginStatus]) -> ConsoleList<ConsolePlugin> {
    ConsoleList::bounded(
        plugins.iter().map(|plugin| ConsolePlugin {
            plugin_id: web_label(&plugin.plugin_id),
            state: wire_name(&plugin.state),
            restarts: plugin.restarts,
            max_restarts: plugin.max_restarts,
            last_error_code: plugin.last_error_code.as_deref().map(web_label),
        }),
        MAX_CONSOLE_ROWS,
    )
}

#[must_use]
pub fn project_routes(routes: &[DispatchCatalogEntry]) -> ConsoleList<ConsoleRoute> {
    ConsoleList::bounded(
        routes.iter().map(|route| ConsoleRoute {
            harness: route.harness.command_name().to_owned(),
            protocol: route.protocol.as_str().to_owned(),
            mode: if !route.enabled {
                "disabled"
            } else if route.allow_execution {
                "executes"
            } else {
                "probe_only"
            },
        }),
        MAX_CONSOLE_ROWS,
    )
}

/// Reads only the evidence class and each route's slot id, route label, and
/// execution flag from the `workflow_slots` payload.
#[must_use]
pub fn project_slots(value: &serde_json::Value) -> ConsoleSlots {
    let text = |value: &serde_json::Value| value.as_str().map_or_else(String::new, web_label);
    let routes = value["routes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    ConsoleSlots {
        evidence_class: text(&value["evidence_class"]),
        routes: ConsoleList::bounded(
            routes.iter().map(|route| ConsoleSlotRoute {
                slot_id: text(&route["slot_id"]),
                route_label: text(&route["route_label"]),
                executes: route["executes"].as_bool() == Some(true),
            }),
            MAX_CONSOLE_ROWS,
        ),
    }
}

/// A decision without its options, labels, prompt size, thresholds, or the
/// request that consumed it.
#[must_use]
pub fn project_decision(view: &DecisionView) -> ConsoleDecision {
    ConsoleDecision {
        decision_id: view.decision_id.to_string(),
        status: wire_name(&view.status),
        chosen_harness: view
            .chosen_harness
            .map(|harness| harness.command_name().to_owned()),
        source: wire_name(&view.source),
        created_at: view
            .created_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown".to_owned()),
        consumed: view.consumed_at.is_some(),
        created_sequence: view.created_sequence,
    }
}

/// Replaces kept rows by id with their re-read versions, adds new ones, and
/// keeps the newest `limit`, newest first. A decision can be consumed after
/// it is recorded, so a re-read row always wins. Returns whether the limit
/// cut any row.
pub fn merge_decisions(
    kept: &mut Vec<ConsoleDecision>,
    read: Vec<ConsoleDecision>,
    limit: usize,
) -> bool {
    for row in read {
        match kept
            .iter_mut()
            .find(|old| old.decision_id == row.decision_id)
        {
            Some(old) => *old = row,
            None => kept.push(row),
        }
    }
    kept.sort_by(|left, right| right.created_sequence.cmp(&left.created_sequence));
    let cut = kept.len() > limit;
    kept.truncate(limit);
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_harness::{AgentKind, dispatch::NativeProtocol};
    use vibemuxd::plugin_registry::PluginState;

    fn decision(id: &str, sequence: u64, consumed: bool) -> ConsoleDecision {
        ConsoleDecision {
            decision_id: id.to_owned(),
            status: "resolved".into(),
            chosen_harness: Some("codex".into()),
            source: "local_rule".into(),
            created_at: "2026-10-22T00:00:00Z".into(),
            consumed,
            created_sequence: sequence,
        }
    }

    #[test]
    fn sections_serialize_with_a_state_tag_and_operator_hints() {
        let loaded: ConsoleSection<u32> = ConsoleSection::Loaded { data: 7 };
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::json!({"state": "loaded", "data": 7})
        );
        assert_eq!(
            serde_json::to_value(unavailable::<u32>("workflow_unconfigured")).unwrap(),
            serde_json::json!({"state": "unavailable", "code": "workflow_unconfigured", "hint": "not configured"})
        );
        assert_eq!(
            serde_json::to_value(ConsoleSection::<u32>::Waiting).unwrap(),
            serde_json::json!({"state": "waiting"})
        );
        for (code, hint) in [
            (
                "daemon_not_running",
                "start it with vibemuxctl daemon start",
            ),
            ("control_unsupported_version", "unsupported by this daemon"),
            ("dsh_session_not_found", "no DSH session is open"),
            (
                "control_endpoint_unavailable",
                "the daemon is not answering; check vibemuxctl daemon inspect",
            ),
            ("something_else", "see vibemuxctl daemon health"),
        ] {
            assert_eq!(operator_hint(code), hint, "{code}");
        }
    }

    #[test]
    fn not_running_marks_every_section_unavailable() {
        let snapshot = ConsoleSnapshot::not_running("daemon_not_running", None);
        let encoded = serde_json::to_value(&snapshot).unwrap();
        for section in [
            "connection",
            "health",
            "plugins",
            "routes",
            "dsh",
            "slots",
            "decisions",
        ] {
            assert_eq!(encoded[section]["state"], "unavailable", "{section}");
            assert_eq!(encoded[section]["code"], "daemon_not_running", "{section}");
        }
    }

    #[test]
    fn plugins_omit_session_and_process_ids() {
        let plugins = [PluginStatus {
            plugin_id: "terminal_observer".into(),
            state: PluginState::Active,
            restarts: 1,
            max_restarts: 3,
            session_id: Some("SESSION_ID_MARKER".into()),
            process_id: Some(424_242),
            last_error_code: Some("plugin_exit".into()),
            graceful_shutdown: None,
        }];
        let list = project_plugins(&plugins);
        assert!(!list.truncated);
        assert_eq!(list.rows[0].state, "active");
        assert_eq!(list.rows[0].last_error_code.as_deref(), Some("plugin_exit"));
        let encoded = serde_json::to_string(&list).unwrap();
        assert!(!encoded.contains("SESSION_ID_MARKER"));
        assert!(!encoded.contains("424242"));
    }

    #[test]
    fn routes_report_one_mode_each() {
        let route = |enabled, allow_execution| DispatchCatalogEntry {
            harness: AgentKind::Codex,
            protocol: NativeProtocol::CodexAppServer,
            enabled,
            allow_execution,
            probe_supported: true,
        };
        let list = project_routes(&[route(true, true), route(true, false), route(false, true)]);
        let modes: Vec<_> = list.rows.iter().map(|row| row.mode).collect();
        assert_eq!(modes, ["executes", "probe_only", "disabled"]);
        assert_eq!(list.rows[0].harness, "codex");
    }

    #[test]
    fn slots_read_only_the_allowlisted_fields_and_mark_truncation() {
        let routes: Vec<_> = (0..MAX_CONSOLE_ROWS + 1)
            .map(|index| {
                serde_json::json!({
                    "slot_id": format!("slot_{index}"),
                    "route_label": "fixture:worker",
                    "executes": index == 0,
                    "worktree": "/private/worktree/PATH_MARKER",
                })
            })
            .collect();
        let slots = project_slots(&serde_json::json!({
            "evidence_class": "offline_fixture",
            "routes": routes,
            "slots": [{"prompt": "PROMPT_MARKER"}],
        }));
        assert_eq!(slots.evidence_class, "offline_fixture");
        assert_eq!(slots.routes.rows.len(), MAX_CONSOLE_ROWS);
        assert!(slots.routes.truncated);
        assert!(slots.routes.rows[0].executes);
        assert!(!slots.routes.rows[1].executes);
        let encoded = serde_json::to_string(&slots).unwrap();
        assert!(!encoded.contains("PATH_MARKER"));
        assert!(!encoded.contains("PROMPT_MARKER"));
        let empty = project_slots(&serde_json::json!({}));
        assert!(empty.routes.rows.is_empty());
    }

    #[test]
    fn merging_decisions_replaces_by_id_and_keeps_the_newest() {
        let mut kept = vec![decision("a", 1, false), decision("b", 2, false)];
        assert!(merge_decisions(
            &mut kept,
            vec![decision("a", 1, true), decision("c", 3, false)],
            2,
        ));
        let ids: Vec<_> = kept.iter().map(|row| row.decision_id.as_str()).collect();
        assert_eq!(ids, ["c", "b"]);
        let mut kept = vec![decision("a", 1, false)];
        assert!(!merge_decisions(&mut kept, vec![decision("a", 1, true)], 4));
        assert!(kept[0].consumed);
    }
}
