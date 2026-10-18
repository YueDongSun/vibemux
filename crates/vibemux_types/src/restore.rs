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
    use crate::{
        MAX_BASE_COMMIT_BYTES, MAX_DESCRIPTION_BYTES, MAX_NAME_BYTES, MAX_TITLE_BYTES, RunSpec,
        TaskSpec,
    };
    use serde_json::json;

    fn task() -> Task {
        Task::new(TaskSpec {
            project_id: ProjectId::new(),
            title: "restoration".to_string(),
            description: String::new(),
        })
        .expect("task")
    }

    fn run() -> Run {
        let task = task();
        Run::new(RunSpec {
            project_id: task.project_id(),
            task_id: task.task_id(),
            harness: "mock".to_string(),
            role: "worker".to_string(),
            protocol: "a2a".to_string(),
            base_commit: "a".repeat(40),
        })
        .expect("run")
    }

    /// The stored JSON of `record` with `key` set to `value`, restored.
    fn restore_with<T: serde::de::DeserializeOwned>(
        record: &impl serde::Serialize,
        key: &str,
        value: serde_json::Value,
    ) -> Result<T, String> {
        let mut encoded = serde_json::to_value(record).expect("encode");
        encoded[key] = value;
        serde_json::from_value(encoded).map_err(|error| error.to_string())
    }

    /// The stored JSON of `record` without `key`, restored.
    fn restore_without<T: serde::de::DeserializeOwned>(
        record: &impl serde::Serialize,
        key: &str,
    ) -> Result<T, String> {
        let mut encoded = serde_json::to_value(record).expect("encode");
        encoded.as_object_mut().expect("object").remove(key);
        serde_json::from_value(encoded).map_err(|error| error.to_string())
    }

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

    #[test]
    fn a_restored_task_keeps_its_stored_status_and_bounded_text() {
        let started = task()
            .transitioned(TaskStatus::InProgress)
            .expect("in progress");
        let encoded = serde_json::to_value(&started).expect("encode");
        let restored: Task = serde_json::from_value(encoded).expect("restore");
        assert_eq!(restored.status(), TaskStatus::InProgress);
        assert_eq!(restored, started);
        // Restoration keeps whatever status was stored; it does not replay transitions.
        let blocked: Task = restore_with(&task(), "status", json!("blocked")).expect("blocked");
        assert_eq!(blocked.status(), TaskStatus::Blocked);

        // The bounds count UTF-8 bytes, not characters.
        let longest_title = "\u{e9}".repeat(MAX_TITLE_BYTES / 2);
        for (index, (key, value)) in [
            ("title", json!(longest_title)),
            ("description", json!("d".repeat(MAX_DESCRIPTION_BYTES))),
            ("description", json!(" ")),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                restore_with::<Task>(&started, key, value).is_ok(),
                "{index}: {key}"
            );
        }
        for (index, (key, value)) in [
            ("title", json!(format!("{longest_title}a"))),
            ("title", json!(" \t")),
            ("title", json!("")),
            ("description", json!("d".repeat(MAX_DESCRIPTION_BYTES + 1))),
            ("status", json!("paused")),
            ("task_id", json!("00000000-0000-0000-0000-000000000000")),
            ("project_id", json!("not_a_uuid")),
            ("unexpected", json!(true)),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                restore_with::<Task>(&started, key, value).is_err(),
                "{index}: {key}"
            );
        }
        for key in ["title", "description", "status"] {
            assert!(restore_without::<Task>(&started, key).is_err(), "{key}");
        }
    }

    #[test]
    fn a_restored_run_keeps_its_stored_status_and_bounded_names() {
        let running = run()
            .transitioned(RunStatus::Running, None)
            .expect("running");
        let encoded = serde_json::to_value(&running).expect("encode");
        let restored: Run = serde_json::from_value(encoded).expect("restore");
        assert_eq!(restored.status(), RunStatus::Running);
        assert_eq!(restored, running);
        let stale: Run = restore_with(&run(), "status", json!("stale")).expect("stale");
        assert_eq!(stale.status(), RunStatus::Stale);

        for key in ["harness", "role", "protocol"] {
            assert!(
                restore_with::<Run>(&running, key, json!("n".repeat(MAX_NAME_BYTES))).is_ok(),
                "{key}"
            );
            for invalid in [json!("n".repeat(MAX_NAME_BYTES + 1)), json!(" "), json!("")] {
                assert!(
                    restore_with::<Run>(&running, key, invalid).is_err(),
                    "{key}"
                );
            }
            assert!(restore_without::<Run>(&running, key).is_err(), "{key}");
        }
        for valid in [json!("abcdef0"), json!("a".repeat(MAX_BASE_COMMIT_BYTES))] {
            assert!(restore_with::<Run>(&running, "base_commit", valid).is_ok());
        }
        for (index, (key, value)) in [
            ("base_commit", json!("abcdef")),
            ("base_commit", json!("a".repeat(MAX_BASE_COMMIT_BYTES + 1))),
            ("base_commit", json!("g".repeat(40))),
            ("status", json!("paused")),
            ("run_id", json!("00000000-0000-0000-0000-000000000000")),
            ("unexpected", json!("user")),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                restore_with::<Run>(&running, key, value).is_err(),
                "{index}: {key}"
            );
        }
    }

    #[test]
    fn restoration_reports_the_same_domain_error_as_construction() {
        let title = "t".repeat(MAX_TITLE_BYTES + 1);
        let constructed = Task::new(TaskSpec {
            project_id: ProjectId::new(),
            title: title.clone(),
            description: String::new(),
        })
        .expect_err("title too long");
        let reported = restore_with::<Task>(&task(), "title", json!(title)).expect_err("too long");
        assert!(reported.starts_with(&constructed.to_string()), "{reported}");

        let description = "d".repeat(MAX_DESCRIPTION_BYTES + 1);
        let constructed = Task::new(TaskSpec {
            project_id: ProjectId::new(),
            title: "restoration".to_string(),
            description: description.clone(),
        })
        .expect_err("description too long");
        let reported =
            restore_with::<Task>(&task(), "description", json!(description)).expect_err("too long");
        assert!(reported.starts_with(&constructed.to_string()), "{reported}");

        let template = run();
        let constructed = Run::new(RunSpec {
            project_id: template.project_id(),
            task_id: template.task_id(),
            harness: "mock".to_string(),
            role: "worker".to_string(),
            protocol: "a2a".to_string(),
            base_commit: "abc".to_string(),
        })
        .expect_err("short commit");
        let reported =
            restore_with::<Run>(&template, "base_commit", json!("abc")).expect_err("short commit");
        assert!(reported.starts_with(&constructed.to_string()), "{reported}");
    }
}
