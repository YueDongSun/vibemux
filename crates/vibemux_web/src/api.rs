//! Content-free read-only JSON API for the local web surface.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use vibemux_types::TaskId;

use crate::control_bridge::{BridgeDshStatus, BridgeTask, SnapshotState};

const DEFAULT_PAGE_SIZE: usize = 32;
const MAX_PAGE_SIZE: usize = 32;

pub fn api_router(snapshot_state: SnapshotState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/tasks", get(tasks))
        .route("/api/v1/tasks/{task_id}", get(task))
        .with_state(snapshot_state)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPageQuery {
    pub after_task_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HealthResponse {
    pub generation: Option<String>,
    pub observed_at: Option<String>,
    pub stale: bool,
    pub daemon: Option<crate::control_bridge::BridgeHealth>,
    pub dsh_status: Option<BridgeDshStatus>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskPageResponse {
    pub generation: Option<String>,
    pub observed_at: Option<String>,
    pub stale: bool,
    pub tasks: Vec<BridgeTask>,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskDetailResponse {
    pub generation: Option<String>,
    pub observed_at: Option<String>,
    pub stale: bool,
    pub task: BridgeTask,
}

#[derive(Clone, Debug, Serialize)]
pub struct ApiError {
    pub error_code: &'static str,
}

async fn health(State(snapshot_state): State<SnapshotState>) -> Json<HealthResponse> {
    let snapshot = snapshot_state.read().await;
    Json(HealthResponse {
        generation: snapshot.generation.clone(),
        observed_at: snapshot.observed_at.clone(),
        stale: snapshot.stale,
        daemon: snapshot.daemon.clone(),
        dsh_status: snapshot.dsh_status.clone(),
    })
}

async fn tasks(
    State(snapshot_state): State<SnapshotState>,
    Query(query): Query<TaskPageQuery>,
) -> Result<Json<TaskPageResponse>, (StatusCode, Json<ApiError>)> {
    let limit = query.limit.unwrap_or(DEFAULT_PAGE_SIZE);
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_limit"));
    }
    let after_id = query
        .after_task_id
        .as_deref()
        .map(str::parse::<TaskId>)
        .transpose()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_cursor"))?
        .map(|id| id.to_string());
    let snapshot = snapshot_state.read().await;
    let start = match after_id {
        Some(after_id) => snapshot
            .tasks
            .iter()
            .position(|task| task.task_id == after_id)
            .map(|index| index + 1)
            .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_cursor"))?,
        None => 0,
    };
    let end = start.saturating_add(limit).min(snapshot.tasks.len());
    let selected = snapshot.tasks[start..end]
        .iter()
        .cloned()
        .map(|mut task| {
            if snapshot.stale {
                task.title = None;
            }
            task
        })
        .collect::<Vec<_>>();
    let next_cursor = if end < snapshot.tasks.len() {
        selected.last().map(|task| task.task_id.clone())
    } else {
        None
    };
    Ok(Json(TaskPageResponse {
        generation: snapshot.generation.clone(),
        observed_at: snapshot.observed_at.clone(),
        stale: snapshot.stale,
        tasks: selected,
        next_cursor,
        truncated: snapshot.truncated,
    }))
}

async fn task(
    State(snapshot_state): State<SnapshotState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskDetailResponse>, (StatusCode, Json<ApiError>)> {
    let task_id = task_id
        .parse::<TaskId>()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_task_id"))?
        .to_string();
    let snapshot = snapshot_state.read().await;
    let task = snapshot
        .tasks
        .iter()
        .find(|task| task.task_id == task_id)
        .cloned()
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "task_not_found"))?;
    let mut task = task;
    if snapshot.stale {
        task.title = None;
    }
    Ok(Json(TaskDetailResponse {
        generation: snapshot.generation.clone(),
        observed_at: snapshot.observed_at.clone(),
        stale: snapshot.stale,
        task,
    }))
}

fn api_error(status: StatusCode, code: &'static str) -> (StatusCode, Json<ApiError>) {
    (status, Json(ApiError { error_code: code }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_bridge::{BridgeHealth, BridgeRun, BridgeSnapshot};
    use uuid::Uuid;

    fn state() -> SnapshotState {
        let tasks = (0..3)
            .map(|_| BridgeTask {
                task_id: Uuid::new_v4().to_string(),
                title: Some("safe title".into()),
                kind: "task".into(),
                state: "open".into(),
                latest_sequence: 1,
                runs: vec![BridgeRun {
                    run_id: Uuid::new_v4().to_string(),
                    harness: "codex".into(),
                    role: "worker".into(),
                    state: "running".into(),
                    latest_sequence: 1,
                }],
                dispatch: None,
                workflow: None,
            })
            .collect();
        Arc::new(tokio::sync::RwLock::new(BridgeSnapshot {
            generation: Some("generation".into()),
            observed_at: Some("2026-10-08T00:00:00Z".into()),
            stale: false,
            project_id: Some("project".into()),
            daemon: Some(BridgeHealth {
                healthy: true,
                store_schema_version: 5,
            }),
            dsh_status: None,
            tasks,
            truncated: false,
        }))
    }

    #[tokio::test]
    async fn api_router_attaches_state_and_returns_paginated_projection() {
        let state = state();
        let app = api_router(state.clone());
        let response = tasks(
            State(state.clone()),
            Query(TaskPageQuery {
                after_task_id: None,
                limit: Some(2),
            }),
        )
        .await
        .expect("task page")
        .0;
        assert_eq!(response.tasks.len(), 2);
        assert_eq!(
            response.next_cursor.as_deref(),
            Some(response.tasks[1].task_id.as_str())
        );
        let next_page = tasks(
            State(state),
            Query(TaskPageQuery {
                after_task_id: response.next_cursor.clone(),
                limit: Some(2),
            }),
        )
        .await
        .expect("second task page")
        .0;
        assert_eq!(next_page.tasks.len(), 1);
        assert!(next_page.next_cursor.is_none());
        let _ = app;
    }

    #[tokio::test]
    async fn stale_api_pages_never_serialize_task_titles() {
        let state = state();
        state.write().await.stale = true;
        let response = tasks(
            State(state),
            Query(TaskPageQuery {
                after_task_id: None,
                limit: Some(1),
            }),
        )
        .await
        .expect("stale page")
        .0;
        assert!(response.stale);
        assert!(response.tasks[0].title.is_none());
        assert!(
            !serde_json::to_string(&response)
                .expect("serialize stale page")
                .contains("safe title")
        );
    }

    use std::sync::Arc;
}
