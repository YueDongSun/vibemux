use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn fixture() -> TempDir {
    let root = TempDir::new().unwrap();
    #[cfg(windows)]
    vibemux_platform::secure_user_directory(root.path()).unwrap();
    fs::write(root.path().join("bootstrap.json"),json!({"schema_version":1,"attempt_id":"006aa0dd-6800-4022-8aa6-f65c1d97c161","grants":{"read_files":false,"search_files":false,"edit_files":false,"write_files":true,"run_dev_tests":false},"owned_paths":["src/**"],"read_scopes":[],"forbidden_paths":[],"files":[]}).to_string()).unwrap();
    root
}
fn frames() -> String {
    [json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"}),json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_file","arguments":{"path":"src/new.txt","content":"candidate bytes"}}})].iter().map(|value|format!("{value}\n")).collect()
}
fn run(root: &TempDir, input: &str) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vibemux_worker_tools"));
    command
        .args(["--bootstrap"])
        .arg(root.path().join("bootstrap.json"))
        .arg("--scratch-parent")
        .arg(root.path())
        .arg("--candidate-out")
        .arg(root.path().join("candidate.json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(input.as_bytes());
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("owned fixture deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn executable_returns_candidate_after_clean_eof_and_removes_owned_workspace() {
    let root = fixture();
    let output = run(&root, &frames());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value =
        serde_json::from_slice(&fs::read(root.path().join("candidate.json")).unwrap()).unwrap();
    assert_eq!(result["status"], "candidate");
    assert_eq!(
        result["candidate"]["changed_files"][0]["content"],
        "candidate bytes"
    );
    assert_eq!(
        fs::read_dir(root.path())
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("vibemux_worker_tools_"))
            .count(),
        0
    );
    let replies = String::from_utf8(output.stdout).unwrap();
    assert!(replies.contains("changed"));
    assert!(!replies.contains("candidate bytes"));
}

#[test]
fn broken_transport_after_effect_retains_correlated_recovery() {
    let root = fixture();
    let output = run(&root, &format!("{}{{", frames()));
    assert!(!output.status.success());
    let result: Value =
        serde_json::from_slice(&fs::read(root.path().join("candidate.json")).unwrap()).unwrap();
    assert_eq!(result["status"], "outcome_unknown");
    assert_eq!(result["recovery"]["operations"][0]["status"], "succeeded");
    let retained =
        std::path::PathBuf::from(result["recovery"]["retained_directory"].as_str().unwrap());
    assert!(retained.starts_with(root.path().canonicalize().unwrap()));
    assert_eq!(
        fs::read_to_string(retained.join("src/new.txt")).unwrap(),
        "candidate bytes"
    );
}

#[test]
fn existing_owner_result_prevents_any_session_or_effect() {
    let root = fixture();
    fs::write(root.path().join("candidate.json"), "prior owner result").unwrap();
    let output = run(&root, &frames());
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(root.path().join("candidate.json")).unwrap(),
        "prior owner result"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn duplicate_bootstrap_authority_fields_are_refused_before_a_session() {
    for version in [1, 2] {
        let root = fixture();
        let bootstrap_path = root.path().join("bootstrap.json");
        let worker: Value = serde_json::from_slice(&fs::read(&bootstrap_path).unwrap()).unwrap();
        let encoded = if version == 1 {
            let encoded = worker.to_string();
            assert!(encoded.contains("\"read_files\":false"));
            encoded.replace(
                "\"read_files\":false",
                "\"read_files\":true,\"read_files\":false",
            )
        } else {
            let mut worker = worker;
            worker["files"] = json!([{"path":"src/test.cjs","content":"process.exit(0);"}]);
            let encoded = json!({"schema_version":2,"worker":worker,"test_policy":{
                "backend":root.path().join("codex.exe"),"node":root.path().join("node.exe"),
                "max_runs":1,"suites":[{"suite_id":"unit","test_entries":["src/test.cjs"],"timeout_ms":1000,"output_limit_bytes":1024}]
            }}).to_string();
            assert!(encoded.contains("\"max_runs\":1"));
            encoded.replace("\"max_runs\":1", "\"max_runs\":2,\"max_runs\":1")
        };
        fs::write(&bootstrap_path, encoded).unwrap();
        let output = run(&root, "");
        assert!(
            !output.status.success(),
            "duplicate authority accepted in version {version}"
        );
        assert_eq!(
            String::from_utf8(output.stderr).unwrap().trim(),
            "worker_tools_invalid_bootstrap"
        );
        assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("vibemux_worker_tools_")
        }));
    }
}
