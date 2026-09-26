//! Read-only frontend snapshots over the authoritative writer connection.

use rusqlite::{OptionalExtension, params};
use vibemux_events::EventEnvelope;
use vibemux_types::{
    Run, Task, TaskId,
    a2a::A2aRunRecord,
    frontend::{
        FrontendArtifactSummary, FrontendEventSummary, FrontendRunSummary, FrontendTaskDetail,
        FrontendTaskSummary, FrontendTasksPage, FrontendTasksQuery, MAX_FRONTEND_ARTIFACTS,
        MAX_FRONTEND_EVENTS, MAX_FRONTEND_RUNS, MAX_FRONTEND_TASKS,
    },
};

use crate::{SqliteStore, StoreError};

const MAX_FRONTEND_PATH_BYTES: usize = 1024;
const MAX_FRONTEND_TITLE_BYTES: usize = 128;

impl SqliteStore {
    pub fn frontend_tasks(
        &self,
        query: &FrontendTasksQuery,
    ) -> Result<FrontendTasksPage, StoreError> {
        if query.limit == 0 || query.limit > MAX_FRONTEND_TASKS {
            return Err(StoreError::InvalidFrontendLimit);
        }
        let Some(project_id) = self.project_id()? else {
            return Ok(FrontendTasksPage {
                project_id: None,
                tasks: Vec::new(),
                next_cursor: None,
            });
        };
        let mut statement = self.connection.prepare(
            "SELECT entity_id, state_json, updated_sequence FROM projections \
             WHERE entity_kind='task' AND project_id=?1 AND entity_id>?2 \
             ORDER BY entity_id LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![
                project_id.to_string(),
                query.after.map(|id| id.to_string()).unwrap_or_default(),
                i64::from(query.limit) + 1
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )?;
        let mut tasks = Vec::new();
        for row in rows {
            let (entity_id, encoded, sequence) = row?;
            let task: Task = serde_json::from_str(&encoded)?;
            if task.project_id() != project_id || task.task_id().to_string() != entity_id {
                return Err(StoreError::FrontendProjectionMismatch);
            }
            tasks.push(task_summary(&task, sequence)?);
        }
        let has_more = tasks.len() > query.limit as usize;
        tasks.truncate(query.limit as usize);
        let next_cursor = has_more
            .then(|| tasks.last().map(|task| task.task_id))
            .flatten();
        Ok(FrontendTasksPage {
            project_id: Some(project_id),
            tasks,
            next_cursor,
        })
    }

    pub fn frontend_task(&self, task_id: TaskId) -> Result<Option<FrontendTaskDetail>, StoreError> {
        let Some(project_id) = self.project_id()? else {
            return Ok(None);
        };
        let task_row: Option<(String, i64)> = self
            .connection
            .query_row(
                "SELECT state_json, updated_sequence FROM projections \
             WHERE entity_kind='task' AND entity_id=?1 AND project_id=?2",
                params![task_id.to_string(), project_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((encoded, sequence)) = task_row else {
            return Ok(None);
        };
        let task: Task = serde_json::from_str(&encoded)?;
        if task.task_id() != task_id || task.project_id() != project_id {
            return Err(StoreError::FrontendProjectionMismatch);
        }
        let task = task_summary(&task, sequence)?;

        let mut statement = self.connection.prepare(
            "SELECT p.entity_id, p.state_json, a.record_json FROM projections p \
             LEFT JOIN a2a_runs a ON a.run_id=p.entity_id AND a.project_id=p.project_id \
             WHERE p.entity_kind='run' AND p.project_id=?1 \
             AND json_extract(p.state_json, '$.task_id')=?2 \
             ORDER BY p.updated_sequence DESC, p.entity_id LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![
                project_id.to_string(),
                task_id.to_string(),
                (MAX_FRONTEND_RUNS + 1) as i64
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )?;
        let mut runs = Vec::new();
        let mut artifacts = Vec::new();
        let mut has_more_artifacts = false;
        let mut has_more_runs = false;
        for row in rows {
            let (entity_id, encoded_run, encoded_a2a) = row?;
            let run: Run = serde_json::from_str(&encoded_run)?;
            if run.project_id() != project_id
                || run.task_id() != task_id
                || run.run_id().to_string() != entity_id
            {
                return Err(StoreError::FrontendProjectionMismatch);
            }
            if runs.len() == MAX_FRONTEND_RUNS {
                has_more_runs = true;
                break;
            }
            let a2a: Option<A2aRunRecord> = encoded_a2a
                .map(|encoded| serde_json::from_str(&encoded))
                .transpose()?;
            if let Some(record) = &a2a {
                if record.binding.project_id != project_id || record.run.run_id() != run.run_id() {
                    return Err(StoreError::FrontendProjectionMismatch);
                }
                for artifact in &record.binding.artifacts {
                    if artifacts.len() == MAX_FRONTEND_ARTIFACTS {
                        has_more_artifacts = true;
                        break;
                    }
                    artifacts.push(FrontendArtifactSummary {
                        run_id: run.run_id(),
                        artifact_id: artifact.artifact_id,
                        sha256: artifact.sha256.clone(),
                        size_bytes: artifact.size_bytes,
                        media_type: artifact.media_type.clone(),
                    });
                }
            }
            runs.push(FrontendRunSummary {
                run_id: run.run_id(),
                task_id,
                harness: run.harness().to_string(),
                role: run.role().to_string(),
                status: run.status(),
                worktree_path: a2a
                    .as_ref()
                    .map(|record| bounded_utf8(&record.workspace.path, MAX_FRONTEND_PATH_BYTES)),
                worktree_path_truncated: a2a
                    .as_ref()
                    .is_some_and(|record| record.workspace.path.len() > MAX_FRONTEND_PATH_BYTES),
                branch: a2a.as_ref().map(|record| record.workspace.branch.clone()),
            });
        }
        let mut statement = self.connection.prepare(
            "SELECT envelope_json FROM events WHERE json_extract(envelope_json, '$.project_id')=?1 \
             AND json_extract(envelope_json, '$.task_id')=?2 ORDER BY sequence DESC LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![
                project_id.to_string(),
                task_id.to_string(),
                (MAX_FRONTEND_EVENTS + 1) as i64
            ],
            |row| row.get::<_, String>(0),
        )?;
        let mut events = Vec::new();
        let mut has_more_events = false;
        for row in rows {
            if events.len() == MAX_FRONTEND_EVENTS {
                row?;
                has_more_events = true;
                break;
            }
            let event = EventEnvelope::from_json_slice(row?.as_bytes())?;
            if event.project_id() != project_id || event.task_id() != Some(task_id) {
                return Err(StoreError::FrontendProjectionMismatch);
            }
            events.push(FrontendEventSummary {
                sequence: event.sequence().get(),
                event_type: event.event_type().as_str().to_string(),
                at: event.timestamp().to_string(),
                run_id: event.run_id(),
            });
        }
        Ok(Some(FrontendTaskDetail {
            task,
            runs,
            has_more_runs,
            events,
            has_more_events,
            artifacts,
            has_more_artifacts,
        }))
    }
}

fn task_summary(task: &Task, sequence: i64) -> Result<FrontendTaskSummary, StoreError> {
    let latest_sequence = u64::try_from(sequence)
        .ok()
        .filter(|value| *value > 0)
        .ok_or(StoreError::FrontendProjectionMismatch)?;
    Ok(FrontendTaskSummary {
        task_id: task.task_id(),
        project_id: task.project_id(),
        title: bounded_utf8(task.title(), MAX_FRONTEND_TITLE_BYTES),
        title_truncated: task.title().len() > MAX_FRONTEND_TITLE_BYTES,
        status: task.status(),
        latest_sequence,
    })
}

fn bounded_utf8(value: &str, max_bytes: usize) -> String {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_events::{ActorName, EventDraft, EventPayload, EventType};
    use vibemux_types::{
        ArtifactId, EventId, ProjectId, RunSpec, TaskSpec,
        a2a::{
            A2aRemoteState, A2aRunAction, A2aRunStart, A2aRunUpdate, ArtifactReference,
            RunWorkspace,
        },
    };

    fn commit_task(store: &mut SqliteStore, project_id: ProjectId, title: String) -> Task {
        let task = Task::new(TaskSpec {
            project_id,
            title,
            description: "private full prompt".to_string(),
        })
        .expect("valid task");
        let draft = EventDraft {
            event_id: EventId::new(),
            event_type: EventType::new("task_created").expect("type"),
            project_id,
            task_id: Some(task.task_id()),
            run_id: None,
            causation_id: None,
            actor: ActorName::new("test").expect("actor"),
            timestamp: time::OffsetDateTime::now_utc(),
            idempotency_key: Some(format!("frontend-{}", task.task_id())),
            payload: EventPayload::new(serde_json::json!({"private_note": "hidden event payload"}))
                .expect("payload"),
        };
        store.commit_task(&task, draft).expect("commit task");
        task
    }

    #[test]
    fn task_page_is_bounded_stable_and_project_scoped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
        let project_id = store.project_id().expect("project").expect("seed project");
        let empty = store
            .frontend_tasks(&FrontendTasksQuery::default())
            .expect("empty page");
        assert_eq!(empty.project_id, Some(project_id));
        assert!(empty.tasks.is_empty());
        for index in 0..35 {
            commit_task(&mut store, project_id, format!("任务_{index}"));
        }
        let other = commit_task(&mut store, ProjectId::new(), "other project".to_string());
        let mut after = None;
        let mut ids = Vec::new();
        loop {
            let page = store
                .frontend_tasks(&FrontendTasksQuery { after, limit: 7 })
                .expect("page");
            assert_eq!(page.project_id, Some(project_id));
            assert!(page.tasks.len() <= 7);
            ids.extend(page.tasks.iter().map(|task| task.task_id));
            after = page.next_cursor;
            if after.is_none() {
                break;
            }
        }
        assert_eq!(ids.len(), 35);
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!ids.contains(&other.task_id()));
        assert!(
            store
                .frontend_task(other.task_id())
                .expect("scoped detail")
                .is_none()
        );
        assert!(matches!(
            store.frontend_tasks(&FrontendTasksQuery {
                after: None,
                limit: 0
            }),
            Err(StoreError::InvalidFrontendLimit)
        ));
        assert!(matches!(
            store.frontend_tasks(&FrontendTasksQuery {
                after: None,
                limit: MAX_FRONTEND_TASKS + 1
            }),
            Err(StoreError::InvalidFrontendLimit)
        ));
    }

    #[test]
    fn detail_excludes_description_and_event_payload() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
        let project_id = store.project_id().expect("project").expect("seed project");
        let task = commit_task(&mut store, project_id, "多语言任务".to_string());
        let before = store.events().expect("events before").len();
        let detail = store
            .frontend_task(task.task_id())
            .expect("detail")
            .expect("found");
        let encoded = serde_json::to_string(&detail).expect("serialized detail");
        assert_eq!(detail.task.title, "多语言任务");
        assert_eq!(detail.events.len(), 1);
        assert!(!encoded.contains("private full prompt"));
        assert!(!encoded.contains("hidden event payload"));
        assert!(!encoded.contains("ownership_token"));
        assert_eq!(store.events().expect("events after").len(), before);
        assert_eq!(
            store
                .frontend_tasks(&FrontendTasksQuery::default())
                .expect("page")
                .tasks
                .len(),
            1
        );
    }

    #[test]
    fn utf8_path_bound_preserves_character_boundary() {
        let input = "界".repeat(400);
        let output = bounded_utf8(&input, MAX_FRONTEND_PATH_BYTES);
        assert!(output.len() <= MAX_FRONTEND_PATH_BYTES);
        assert!(input.starts_with(&output));
    }

    #[test]
    fn escaped_titles_stay_within_control_frame_budget() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
        let project_id = store.project_id().expect("project").expect("seed project");
        for _ in 0..MAX_FRONTEND_TASKS {
            commit_task(&mut store, project_id, "\u{0001}".repeat(512));
        }
        let page = store
            .frontend_tasks(&FrontendTasksQuery::default())
            .expect("page");
        assert_eq!(page.tasks.len(), MAX_FRONTEND_TASKS as usize);
        assert!(page.tasks.iter().all(|task| task.title_truncated));
        let encoded = serde_json::to_vec(&page).expect("json");
        assert!(encoded.len() < 64 * 1024, "page is {} bytes", encoded.len());
    }

    #[test]
    fn a2a_detail_exposes_artifact_metadata_without_ownership_token() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
        let project_id = store.project_id().expect("project").expect("seed project");
        let task = Task::new(TaskSpec {
            project_id,
            title: "artifact task".to_string(),
            description: String::new(),
        })
        .expect("task");
        let run = Run::new(RunSpec {
            project_id,
            task_id: task.task_id(),
            harness: "mock".to_string(),
            role: "worker".to_string(),
            protocol: "a2a".to_string(),
            base_commit: "a".repeat(40),
        })
        .expect("run");
        let timestamp = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("time");
        let start = A2aRunStart {
            task: task.clone(),
            run: run.clone(),
            peer_id: "peer".to_string(),
            external_task_id: format!("remote_{}", run.run_id()),
            transport: "http_json".to_string(),
            protocol_version: "1.0".to_string(),
            workspace: RunWorkspace {
                path: "C:\\worktrees\\artifact".to_string(),
                branch: "codex/artifact".to_string(),
                base_commit: "a".repeat(40),
                ownership_token: "private_owner_token".to_string(),
            },
            timestamp,
            idempotency_key: format!("start_{}", run.run_id()),
        };
        let started = store.start_a2a_run(start).expect("start").record;
        let running = store
            .update_a2a_run(A2aRunUpdate {
                run_id: run.run_id(),
                expected_version: started.version,
                timestamp: timestamp + time::Duration::milliseconds(1),
                idempotency_key: format!("run_{}", run.run_id()),
                action: A2aRunAction::Start,
            })
            .expect("run")
            .record;
        let artifact_id = ArtifactId::new();
        store
            .update_a2a_run(A2aRunUpdate {
                run_id: run.run_id(),
                expected_version: running.version,
                timestamp: timestamp + time::Duration::milliseconds(2),
                idempotency_key: format!("observe_{}", run.run_id()),
                action: A2aRunAction::Observe {
                    state: A2aRemoteState::Completed,
                    artifacts: vec![ArtifactReference {
                        artifact_id,
                        sha256: "b".repeat(64),
                        size_bytes: 42,
                        media_type: "application/json".to_string(),
                    }],
                },
            })
            .expect("observe");
        let detail = store
            .frontend_task(task.task_id())
            .expect("detail")
            .expect("found");
        assert_eq!(detail.runs.len(), 1);
        assert_eq!(
            detail.runs[0].worktree_path.as_deref(),
            Some("C:\\worktrees\\artifact")
        );
        assert_eq!(detail.artifacts.len(), 1);
        assert_eq!(detail.artifacts[0].artifact_id, artifact_id);
        assert!(
            !serde_json::to_string(&detail)
                .expect("json")
                .contains("private_owner_token")
        );
    }
}
