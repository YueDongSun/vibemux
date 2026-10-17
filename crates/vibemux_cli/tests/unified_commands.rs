//! Command-level compatibility and offline UI checks; no vendor process is run.
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "--initial-branch=main"]);
    git(
        directory.path(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "PRIVATE_COMMIT_SUBJECT",
        ],
    );
    git(directory.path(), &["branch", "python_reference_test"]);
    git(directory.path(), &["branch", "codex/topic_example"]);
    let private_url = format!(
        "https://{}@example.invalid/repository",
        "PRIVATE_USER:PRIVATE_TOKEN"
    );
    git(directory.path(), &["remote", "add", "origin", &private_url]);
    directory
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .unwrap();
    // Fixture arguments are constants, so naming them and Git's own stderr
    // exposes no private data and makes a setup failure diagnosable.
    assert!(
        output.status.success(),
        "fixture git operation failed: git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

fn run(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vibemuxctl"))
        .args(arguments)
        .current_dir(root)
        .output()
        .unwrap()
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vibemux_probe/tests/fixtures/probe_report_v1.json")
        .canonicalize()
        .unwrap()
}

#[test]
fn help_and_usage_errors_do_not_touch_runtime_or_echo_private_arguments() {
    let directory = repository();
    let help = run(directory.path(), &["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for command in ["workspace", "harness", "daemon", "probe", "ui", "--format"] {
        assert!(help.contains(command));
    }
    let error = run(directory.path(), &["--json", "PRIVATE_BAD_COMMAND"]);
    assert_eq!(error.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&error.stderr).contains("PRIVATE_BAD_COMMAND"));
    assert_eq!(
        serde_json::from_slice::<Value>(&error.stderr).unwrap()["error_code"],
        "cli_invalid_arguments"
    );
    assert!(!directory.path().join(".vibemux").exists());
}

#[test]
fn workspace_inventory_reads_local_graph_without_urls_subjects_or_index_changes() {
    let directory = repository();
    std::fs::write(directory.path().join("new_source.txt"), "fixture").unwrap();
    git(directory.path(), &["add", "new_source.txt"]);
    let index_before = std::fs::read(directory.path().join(".git/index")).unwrap();
    let output = run(directory.path(), &["workspace", "status"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(report["source"], "local_git_cached_refs");
    assert_eq!(report["current_branch"], "main");
    assert_eq!(report["baseline"], "main");
    assert_eq!(report["staged"], 1);
    assert_eq!(report["branches"].as_array().unwrap().len(), 3);
    assert_eq!(report["worktrees"][0]["location"], "<primary>");
    assert!(!text.contains("PRIVATE_"));
    assert!(!text.contains(directory.path().to_string_lossy().as_ref()));
    assert_eq!(
        std::fs::read(directory.path().join(".git/index")).unwrap(),
        index_before
    );
    assert!(!directory.path().join(".vibemux").exists());
}

/// The filter fixture's path as a Git filter command. Forward slashes keep
/// the path free of shell metacharacters, so Git executes it directly instead
/// of through `sh -c`.
#[cfg(feature = "test_helpers")]
fn filter_command() -> String {
    env!("CARGO_BIN_EXE_vibemux_git_filter_fixture").replace('\\', "/")
}

#[cfg(feature = "test_helpers")]
#[test]
fn workspace_inventory_disables_external_git_filters_and_labels_the_comparison() {
    let directory = repository();
    let root = directory.path();
    std::fs::write(root.join(".gitattributes"), "tracked.txt filter=fixture\n").unwrap();
    std::fs::write(root.join("tracked.txt"), "fixture content\n").unwrap();
    // This synthetic Git filter demonstrates the side effect under test. Its
    // command is the fixture's build path, with no user input.
    let filter = filter_command();
    git(root, &["config", "filter.fixture.clean", &filter]);
    git(root, &["config", "filter.fixture.required", "true"]);
    git(root, &["add", ".gitattributes", "tracked.txt"]);
    let marker = root.join("filter_marker");
    assert!(marker.is_file(), "the fixture filter must actually execute");
    std::fs::remove_file(&marker).unwrap();
    // Changed stat data with identical bytes makes status rehash the tracked file.
    std::fs::OpenOptions::new()
        .write(true)
        .open(root.join("tracked.txt"))
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000))
        .unwrap();
    let index_before = std::fs::read(root.join(".git/index")).unwrap();
    let output = run(root, &["workspace", "status"]);
    assert!(output.status.success());
    assert!(
        !marker.exists(),
        "inventory must not execute a clean filter"
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["change_detection"], "external_filters_disabled");
    assert_eq!(
        std::fs::read(root.join(".git/index")).unwrap(),
        index_before
    );

    // A long-running process filter takes precedence over `clean`. The same
    // fixture would leave the marker if Git started it.
    git(root, &["config", "filter.fixture.process", &filter]);
    let output = run(root, &["workspace", "status"]);
    assert!(output.status.success());
    assert!(
        !marker.exists(),
        "inventory must not start a process filter"
    );
    let text = run(root, &["--format", "text", "workspace", "status"]);
    assert!(
        String::from_utf8(text.stdout)
            .unwrap()
            .contains("External filters disabled")
    );
}

#[test]
fn status_and_offline_ui_do_not_start_a_daemon_or_harness() {
    let directory = repository();
    let status = run(directory.path(), &["status"]);
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["daemon"]["healthy"], false);
    let fixture = fixture_path();
    let fixture = fixture.to_str().unwrap();
    let ui = run(
        directory.path(),
        &[
            "--format", "text", "ui", "--once", "--report", fixture, "--width", "80", "--height",
            "24",
        ],
    );
    assert!(
        ui.status.success(),
        "offline UI failed: {}",
        String::from_utf8_lossy(&ui.stderr)
    );
    let rendered = String::from_utf8(ui.stdout).unwrap();
    for expected in [
        "VibeMux",
        "Claude",
        "Codex",
        "OpenCode",
        "Copilot",
        "Grok",
        "not_run",
        "Workspace",
        "READ ONLY",
    ] {
        assert!(rendered.contains(expected), "missing {expected}");
    }
    assert!(!rendered.contains("127.0.0.1"));
    assert!(!rendered.contains('\u{1b}'));
    let json_ui = run(directory.path(), &["ui", "--once", "--report", fixture]);
    let json_ui: Value = serde_json::from_slice(&json_ui.stdout).unwrap();
    assert_eq!(json_ui["source"], "saved_report");
    assert_eq!(json_ui["workspace"]["current_branch"], "main");
    assert!(!directory.path().join(".vibemux").exists());
}

#[test]
fn harness_catalog_stays_metadata_only_through_unified_entrypoint() {
    let directory = repository();
    let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/harness/harness_config.json")
        .canonicalize()
        .unwrap();
    let output = run(
        directory.path(),
        &["harness", "catalog", "--config", config.to_str().unwrap()],
    );
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["harnesses"].as_array().unwrap().len(), 5);
    assert!(
        report["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .all(|route| route["allow_execution"] == false)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("native_bin"));
    let disabled = report["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|route| route["enabled"] != true)
        .count();
    let text = run(
        directory.path(),
        &[
            "--format",
            "text",
            "harness",
            "catalog",
            "--config",
            config.to_str().unwrap(),
        ],
    );
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("Actions"));
    assert_eq!(
        text.lines()
            .filter(|line| line.ends_with(" disabled"))
            .count(),
        disabled
    );
    // No example route grants execution, so no row may offer it.
    assert!(!text.contains("execute"));
    assert!(!text.contains("native_bin"));
    assert!(!directory.path().join(".vibemux").exists());
}
