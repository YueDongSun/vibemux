//! Explicit bounded metadata projection; never serialize frontend DTOs wholesale.

use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use vibemux_types::frontend::{
    FrontendTaskDetail, FrontendTaskSummary, FrontendTasksPage, MAX_FRONTEND_ARTIFACTS,
    MAX_FRONTEND_EVENTS, MAX_FRONTEND_RUNS, MAX_FRONTEND_TASKS,
};
use vibemuxd::control::{DaemonHealth, HarnessDispatchStatus};

const TITLE_BYTES: usize = 240;
const TOKEN_BYTES: usize = 96;
const TIMESTAMP_BYTES: usize = 40;
const EVIDENCE: &str = "daemon_declared_metadata";

pub fn health(health: DaemonHealth) -> Value {
    json!({"healthy":health.healthy,"store_schema_version":health.store_schema_version,
        "queue_capacity":health.queue_capacity,"queue_depth":health.queue_depth,
        "queue_high_watermark":health.queue_high_watermark,"queue_saturated":health.queue_saturated,
        "evidence":EVIDENCE})
}

pub fn tasks(page: FrontendTasksPage, limit: u32) -> Value {
    let limit = limit.min(MAX_FRONTEND_TASKS) as usize;
    let truncated = page.tasks.len() > limit;
    let next_cursor = if truncated {
        page.tasks
            .get(limit.saturating_sub(1))
            .map(|task| task.task_id)
    } else {
        page.next_cursor
    };
    let tasks: Vec<_> = page.tasks.iter().take(limit).map(task_summary).collect();
    json!({"project_id":page.project_id,"tasks":tasks,"next_cursor":next_cursor,"truncated":truncated,"evidence":EVIDENCE})
}

pub fn task_detail(detail: Option<FrontendTaskDetail>) -> Value {
    let Some(detail) = detail else {
        return json!({"task":null,"evidence":EVIDENCE});
    };
    let runs: Vec<_> = detail
        .runs
        .iter()
        .take(MAX_FRONTEND_RUNS)
        .map(|run| {
            json!({
                "run_id":run.run_id,"task_id":run.task_id,"harness":token(&run.harness),
                "role":token(&run.role),"status":run.status,
            })
        })
        .collect();
    let events: Vec<_> = detail
        .events
        .iter()
        .take(MAX_FRONTEND_EVENTS)
        .map(|event| {
            json!({
                "sequence":event.sequence,"event_type":token(&event.event_type),
                "at":timestamp(&event.at),"run_id":event.run_id,
            })
        })
        .collect();
    let artifacts: Vec<_> = detail.artifacts.iter().take(MAX_FRONTEND_ARTIFACTS).map(|artifact| json!({
        "run_id":artifact.run_id,"artifact_id":artifact.artifact_id,"size_bytes":artifact.size_bytes,
        "sha256":digest(&artifact.sha256),"media_type":media_type(&artifact.media_type),
    })).collect();
    json!({"task":task_summary(&detail.task),"runs":runs,"events":events,"artifacts":artifacts,
        "has_more_runs":detail.has_more_runs || detail.runs.len()>MAX_FRONTEND_RUNS,
        "has_more_events":detail.has_more_events || detail.events.len()>MAX_FRONTEND_EVENTS,
        "has_more_artifacts":detail.has_more_artifacts || detail.artifacts.len()>MAX_FRONTEND_ARTIFACTS,
        "evidence":EVIDENCE})
}

pub fn dispatch(status: HarnessDispatchStatus) -> Value {
    json!({"request_id":status.request_id,"task_id":status.task_id,"run_id":status.run_id,
        "harness":status.harness,"protocol":status.protocol,"phase":status.phase,"version":status.version,
        "outcome":status.outcome,"error_code":status.error_code,
        "created_at":status.created_at.format(&Rfc3339).ok(),"updated_at":status.updated_at.format(&Rfc3339).ok(),
        "evidence":EVIDENCE})
}

fn task_summary(task: &FrontendTaskSummary) -> Value {
    let mut title = String::new();
    for character in task.title.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if title.len() + character.len_utf8() > TITLE_BYTES {
            break;
        }
        title.push(character);
    }
    let truncated = task.title_truncated || title != task.title;
    json!({"task_id":task.task_id,"project_id":task.project_id,"title":title,
        "title_truncated":truncated,"status":task.status,"latest_sequence":task.latest_sequence})
}

fn token(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= TOKEN_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)))
    .then_some(value)
}

fn media_type(value: &str) -> Option<&str> {
    let (kind, subtype) = value.split_once('/')?;
    let valid_component = |component: &str| {
        component
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&byte))
    };
    (value.len() <= TOKEN_BYTES && valid_component(kind) && valid_component(subtype))
        .then_some(value)
}

fn timestamp(value: &str) -> Option<String> {
    if value.len() > TIMESTAMP_BYTES {
        return None;
    }
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()?
        .format(&Rfc3339)
        .ok()
}

fn digest(value: &str) -> Option<String> {
    (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use vibemux_types::{
        ArtifactId, ProjectId, RunId, RunStatus, TaskId, TaskStatus,
        frontend::{
            FrontendArtifactSummary, FrontendEventSummary, FrontendRunSummary, FrontendTaskSummary,
        },
    };

    #[test]
    fn artifact_media_types_reject_path_shaped_labels() {
        let media_types = [
            "text/plain",
            "application/vnd.api+json",
            "image/svg+xml",
            "/private/file",
            "../private",
            "text//plain",
            "text/",
            "/plain",
        ];
        for (index, media_type) in media_types.iter().enumerate() {
            let detail: FrontendTaskDetail = serde_json::from_value(json!({
                "task": {"task_id":TaskId::new(),"project_id":ProjectId::new(),
                    "title":"artifact metadata","title_truncated":false,"status":"open","latest_sequence":1},
                "runs":[],"has_more_runs":false,"events":[],"has_more_events":false,
                "artifacts":[{"run_id":RunId::new(),"artifact_id":ArtifactId::new(),
                    "sha256":"a".repeat(64),"size_bytes":1,"media_type":media_type}],
                "has_more_artifacts":false
            })).unwrap();
            let projected = task_detail(Some(detail));
            if index < 3 {
                assert_eq!(projected["artifacts"][0]["media_type"], *media_type);
            } else {
                assert!(
                    projected["artifacts"][0]["media_type"].is_null(),
                    "path-shaped media labels must not be disclosed"
                );
            }
        }
    }

    #[test]
    fn task_projection_bounds_metadata_and_omits_private_fields() {
        let private_marker = Uuid::new_v4().to_string();
        let task_id = TaskId::new();
        let run_id = RunId::new();
        let detail = FrontendTaskDetail {
            task: FrontendTaskSummary {
                task_id,
                project_id: ProjectId::new(),
                title: "界".repeat(200),
                title_truncated: false,
                status: TaskStatus::Open,
                latest_sequence: 7,
            },
            runs: vec![
                FrontendRunSummary {
                    run_id,
                    task_id,
                    harness: "fixture".into(),
                    role: "worker".into(),
                    status: RunStatus::Preparing,
                    worktree_path: Some(private_marker.clone()),
                    worktree_path_truncated: false,
                    branch: Some(private_marker.clone())
                };
                13
            ],
            has_more_runs: false,
            events: vec![
                FrontendEventSummary {
                    sequence: 7,
                    event_type: "task.created".into(),
                    at: "2026-10-12T00:00:00Z".into(),
                    run_id: Some(run_id)
                };
                25
            ],
            has_more_events: false,
            artifacts: vec![
                FrontendArtifactSummary {
                    run_id,
                    artifact_id: ArtifactId::new(),
                    sha256: "a".repeat(64),
                    size_bytes: 10,
                    media_type: "text/plain".into()
                };
                25
            ],
            has_more_artifacts: false,
        };
        let projected = task_detail(Some(detail));
        assert_eq!(projected["task"]["title"].as_str().unwrap().len(), 240);
        assert_eq!(projected["task"]["title_truncated"], true);
        assert_eq!(projected["runs"].as_array().unwrap().len(), 12);
        assert_eq!(projected["events"].as_array().unwrap().len(), 24);
        assert_eq!(projected["artifacts"].as_array().unwrap().len(), 24);
        for key in ["has_more_runs", "has_more_events", "has_more_artifacts"] {
            assert_eq!(projected[key], true);
        }
        assert_eq!(projected["task"]["status"], "open");
        assert_eq!(projected["evidence"], "daemon_declared_metadata");
        let encoded = projected.to_string();
        assert!(!encoded.contains(&private_marker));
        assert!(!encoded.contains("worktree_path") && !encoded.contains("branch"));
        assert!(encoded.len() < 32 * 1024);
        assert!(task_detail(None)["task"].is_null());
    }

    #[test]
    fn typed_dispatch_projection_excludes_capture_process_and_prompt_metadata() {
        let private_marker = Uuid::new_v4().to_string();
        let status: HarnessDispatchStatus = serde_json::from_value(json!({
            "request_id":Uuid::new_v4(),"task_id":TaskId::new(),"run_id":RunId::new(),
            "harness":"codex","protocol":"codex_exec","phase":"unverified","version":3,
            "outcome":"unverified","error_code":null,"process":null,"capture":null,"live_capture":null,
            "prompt_bytes":123456,"base_commit":private_marker,"created_at":"2026-10-12T00:00:00Z","updated_at":"2026-10-12T00:01:00Z"
        })).unwrap();
        let projected = dispatch(status);
        assert_eq!(projected["phase"], "unverified");
        assert_eq!(projected["outcome"], "unverified");
        assert_eq!(projected["version"], 3);
        for field in [
            "process",
            "capture",
            "live_capture",
            "prompt_bytes",
            "base_commit",
        ] {
            assert!(projected.get(field).is_none());
        }
        assert!(!projected.to_string().contains(&private_marker));
        assert_eq!(projected["evidence"], "daemon_declared_metadata");
    }

    #[test]
    fn task_pages_honor_requested_limit_and_invalid_text_metadata_is_null() {
        let task_id = TaskId::new();
        let summary = FrontendTaskSummary {
            task_id,
            project_id: ProjectId::new(),
            title: "line\nnext".into(),
            title_truncated: false,
            status: TaskStatus::Open,
            latest_sequence: 1,
        };
        let projected = tasks(
            FrontendTasksPage {
                project_id: Some(summary.project_id),
                tasks: vec![summary; 33],
                next_cursor: None,
            },
            2,
        );
        assert_eq!(projected["tasks"].as_array().unwrap().len(), 2);
        assert_eq!(projected["next_cursor"], task_id.to_string());
        assert_eq!(projected["tasks"][0]["title"], "line next");
        assert_eq!(projected["tasks"][0]["title_truncated"], true);
        assert!(token("C:/private/path").is_none());
        assert!(token(&"x".repeat(97)).is_none());
        assert!(timestamp("not_a_timestamp").is_none());
        assert!(digest("not_a_digest").is_none());
    }
}
