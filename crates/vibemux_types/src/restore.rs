//! Validated restoration of persisted domain records, preserving their JSON shape.
use crate::{ProjectId, Run, RunId, RunStatus, Task, TaskId, TaskStatus};
use serde::{Deserialize, Deserializer, de};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskRecord {
    task_id: TaskId,
    project_id: ProjectId,
    title: String,
    description: String,
    status: TaskStatus,
}

impl<'de> Deserialize<'de> for Task {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let record = TaskRecord::deserialize(deserializer)?;
        crate::validate_required("title", &record.title, crate::MAX_TITLE_BYTES)
            .map_err(de::Error::custom)?;
        crate::validate_optional(
            "description",
            &record.description,
            crate::MAX_DESCRIPTION_BYTES,
        )
        .map_err(de::Error::custom)?;
        Ok(Self {
            task_id: record.task_id,
            project_id: record.project_id,
            title: record.title,
            description: record.description,
            status: record.status,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRecord {
    run_id: RunId,
    project_id: ProjectId,
    task_id: TaskId,
    harness: String,
    role: String,
    protocol: String,
    base_commit: String,
    status: RunStatus,
}

impl<'de> Deserialize<'de> for Run {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let record = RunRecord::deserialize(deserializer)?;
        crate::validate_required("harness", &record.harness, crate::MAX_NAME_BYTES)
            .map_err(de::Error::custom)?;
        crate::validate_required("role", &record.role, crate::MAX_NAME_BYTES)
            .map_err(de::Error::custom)?;
        crate::validate_required("protocol", &record.protocol, crate::MAX_NAME_BYTES)
            .map_err(de::Error::custom)?;
        crate::validate_base_commit(&record.base_commit).map_err(de::Error::custom)?;
        Ok(Self {
            run_id: record.run_id,
            project_id: record.project_id,
            task_id: record.task_id,
            harness: record.harness,
            role: record.role,
            protocol: record.protocol,
            base_commit: record.base_commit,
            status: record.status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RunSpec, TaskSpec};

    #[test]
    fn canonical_records_restore_without_changing_json_contract() {
        let task = Task::new(TaskSpec {
            project_id: ProjectId::new(),
            title: "restoration".to_string(),
            description: String::new(),
        })
        .expect("task");
        let run = Run::new(RunSpec {
            project_id: task.project_id(),
            task_id: task.task_id(),
            harness: "mock".to_string(),
            role: "worker".to_string(),
            protocol: "a2a".to_string(),
            base_commit: "a".repeat(40),
        })
        .expect("run");
        let task_json = serde_json::to_value(&task).expect("task JSON");
        let run_json = serde_json::to_value(&run).expect("run JSON");
        assert_eq!(
            serde_json::from_value::<Task>(task_json.clone()).expect("restored task"),
            task
        );
        assert_eq!(
            serde_json::from_value::<Run>(run_json.clone()).expect("restored run"),
            run
        );
        let mut invalid = task_json;
        invalid["task_id"] = serde_json::json!("00000000-0000-0000-0000-000000000000");
        assert!(serde_json::from_value::<Task>(invalid).is_err());
        let mut invalid = run_json.clone();
        invalid["base_commit"] = serde_json::json!("not_a_commit");
        assert!(serde_json::from_value::<Run>(invalid).is_err());
        let mut invalid = run_json;
        invalid["unexpected_authority"] = serde_json::json!("user");
        assert!(serde_json::from_value::<Run>(invalid).is_err());
    }
}
