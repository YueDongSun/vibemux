//! Explicit, opt-in live inference experiment; never a default test/bench target.

#[path = "fusion_benchmark/cases.rs"]
mod cases;
#[path = "fusion_benchmark/config.rs"]
mod config;
#[path = "fusion_benchmark/evidence.rs"]
mod evidence;
#[path = "fusion_benchmark/workflow.rs"]
mod workflow;

use std::{path::PathBuf, time::Duration};

use serde_json::json;
use tokio::sync::watch;
use vibemux_harness_native::Error;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{}", error.code());
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Error> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.is_empty() || (arguments.len() == 1 && arguments[0] == "--plan") {
        println!("{}", plan());
        return Ok(());
    }
    if arguments.len() != 3 || arguments[0] != "--execute-live" {
        return Err(Error::new(
            "fusion_usage_plan_or_execute_live_config_new_output_dir",
        ));
    }
    let config_path = PathBuf::from(&arguments[1]);
    let output_path = PathBuf::from(&arguments[2]);
    // Every filesystem/configuration operation happens before a vendor launch.
    let prepared = tokio::task::spawn_blocking(move || config::prepare(&config_path))
        .await
        .map_err(|_| Error::new("fusion_setup_worker_failed"))??;
    let duration = Duration::from_secs(prepared.budget_seconds);
    let evidence = evidence::Evidence::create(output_path).await?;
    evidence.record(prepared.header()).await?;
    let (cancel, receiver) = watch::channel(false);
    let experiment = workflow::run(&prepared, &evidence, receiver);
    tokio::pin!(experiment);
    let summary = tokio::select! {
        result = &mut experiment => result,
        () = tokio::time::sleep(duration) => {
            // Signal the existing dispatcher cancellation path, then join it.
            // Dropping a dispatch future here could abandon cleanup/evidence.
            let _ = cancel.send(true);
            experiment.await
        }
    }?;
    evidence.record(summary.clone()).await?;
    println!("{summary}");
    if summary["all_samples_correct"] != true {
        return Err(Error::new("fusion_benchmark_not_all_correct"));
    }
    Ok(())
}

fn plan() -> serde_json::Value {
    json!({
        "schema_version": 1,
        "record_type": "plan",
        "status": "NOT_RUN",
        "workflow_version": workflow::VERSION,
        "suite_version": cases::SUITE_VERSION,
        "suite_sha256": cases::suite_digest(),
        "cases": cases::cases().iter().map(|case| case.id).collect::<Vec<_>>(),
        "modes": workflow::MODES,
        "maximum_dispatches_per_case_repetition": 9,
        "maximum_parallel_dispatches": 2,
        "live_execution_requires": "--execute-live CONFIG NEW_OUTPUT_DIRECTORY",
        "scope": "fixed coding reasoning tasks with deterministic grading; no generated code execution",
        "source": "github/docs@5c9f8941dd6b10b27f350ea7de093aebc5d2a3ef:content/copilot/concepts/models/copilot-fusion.md"
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_plan_is_not_execution_evidence() {
        let value = super::plan();
        assert_eq!(value["status"], "NOT_RUN");
        assert_eq!(value["maximum_parallel_dispatches"], 2);
        assert_eq!(value["modes"].as_array().unwrap().len(), 4);
    }
}
