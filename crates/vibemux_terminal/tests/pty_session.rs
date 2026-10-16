#![forbid(unsafe_code)]
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use vibemux_terminal::{
    client::TerminalClient,
    wire::{Open, Size, command},
};

fn wait_for(client: &mut TerminalClient, mut predicate: impl FnMut(&TerminalClient) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        client.poll();
        if predicate(client) {
            return;
        }
        assert!(
            !matches!(client.status.phase.as_str(), "failed" | "exited"),
            "{}: {}",
            client.status.phase,
            client.status.code
        );
        assert!(
            Instant::now() < deadline,
            "terminal fixture timed out: {}",
            client.status.phase
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn real_pty_transfers_input_resizes_and_stops_only_its_owned_session() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_BIN_EXE_vibemux_terminal_fixture"));
    let host = Path::new(env!("CARGO_BIN_EXE_vibemux_terminal_host"));
    let open = Open {
        project_root: root.path().to_string_lossy().into_owned(),
        executable: fixture.to_string_lossy().into_owned(),
        git_executable: "unused_fixture_git".into(),
        size: Some(Size {
            columns: 80,
            rows: 24,
        }),
        fixture: true,
    };
    let mut unrelated = TerminalClient::start(host, open.clone(), Arc::new(|| {})).unwrap();
    wait_for(&mut unrelated, |client| client.status.phase == "fixture");
    let mut client = TerminalClient::start(host, open, Arc::new(|| {})).unwrap();
    wait_for(&mut client, |client| {
        client.screen.as_ref().is_some_and(|screen| {
            screen
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .contains("PTY fixture")
        })
    });
    assert_ne!(client.status.process_id, unrelated.status.process_id);
    client
        .send(command::Operation::Input(
            "fixture_input_42 東京".as_bytes().to_vec(),
        ))
        .unwrap();
    wait_for(&mut client, |client| {
        client.screen.as_ref().is_some_and(|screen| {
            screen
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .contains("fixture_input_42")
        })
    });
    client
        .send(command::Operation::Resize(Size {
            columns: 100,
            rows: 30,
        }))
        .unwrap();
    wait_for(&mut client, |client| {
        client.screen.as_ref().is_some_and(|screen| {
            screen.size
                == Some(Size {
                    columns: 100,
                    rows: 30,
                })
        })
    });
    client.stop();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        client.poll();
        if client.status.phase == "exited" {
            break;
        }
        assert!(Instant::now() < deadline, "session stop did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(client);
    unrelated
        .send(command::Operation::Input(b"still_running_73".to_vec()))
        .unwrap();
    wait_for(&mut unrelated, |client| {
        client.screen.as_ref().is_some_and(|screen| {
            screen
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .contains("still_running_73")
        })
    });
    unrelated.send(command::Operation::Input(vec![17])).unwrap();
    wait_for(&mut unrelated, |client| client.status.phase == "exited");
    drop(unrelated);
    assert!(
        !root.path().join(".vibemux").exists(),
        "fixture must not allocate a real worktree"
    );
}

#[test]
fn worktree_scope_is_component_aware() {
    use vibemux_terminal::workspace::is_within;
    assert!(is_within(
        Path::new("/root/worktrees"),
        Path::new("/root/worktrees/owned")
    ));
    assert!(!is_within(
        Path::new("/root/worktrees"),
        Path::new("/root/worktrees_evil/owned")
    ));
}

fn git_output(git: &Path, root: &Path, arguments: &[&std::ffi::OsStr]) -> Vec<u8> {
    let result = std::process::Command::new(git)
        .current_dir(root)
        .args(["-c", "core.hooksPath=", "-c", "core.fsmonitor=false"])
        .args(arguments)
        .output()
        .unwrap();
    assert!(result.status.success(), "fixture Git operation failed");
    result.stdout
}

#[test]
fn real_worktree_preserves_dirty_main_and_records_an_independent_branch() {
    let root = tempfile::tempdir().unwrap();
    let git = vibemux_terminal::client::find_executable("git")
        .expect("Git required for worktree contract tests");
    git_output(&git, root.path(), &["init".as_ref(), "--quiet".as_ref()]);
    std::fs::write(root.path().join(".gitignore"), "/.vibemux/\n").unwrap();
    git_output(
        &git,
        root.path(),
        &[
            "config".as_ref(),
            "core.autocrlf".as_ref(),
            "false".as_ref(),
        ],
    );
    std::fs::write(root.path().join("README.md"), "committed baseline\n").unwrap();
    std::fs::create_dir(root.path().join("nested")).unwrap();
    std::fs::write(root.path().join("nested/keep"), "baseline\n").unwrap();
    git_output(&git, root.path(), &["add".as_ref(), ".".as_ref()]);
    git_output(
        &git,
        root.path(),
        &[
            "-c".as_ref(),
            "user.name=Fixture".as_ref(),
            "-c".as_ref(),
            "user.email=fixture@example.invalid".as_ref(),
            "commit".as_ref(),
            "--quiet".as_ref(),
            "-m".as_ref(),
            "fixture".as_ref(),
        ],
    );
    std::fs::write(
        root.path().join("README.md"),
        "uncommitted operator change\n",
    )
    .unwrap();
    std::fs::write(root.path().join("notes.txt"), "untracked operator change\n").unwrap();
    let before_status = git_output(
        &git,
        root.path(),
        &["status".as_ref(), "--porcelain=v1".as_ref()],
    );
    let before_head = git_output(&git, root.path(), &["rev-parse".as_ref(), "HEAD".as_ref()]);
    let before_branch = git_output(
        &git,
        root.path(),
        &["branch".as_ref(), "--show-current".as_ref()],
    );
    let receipt = vibemux_terminal::workspace::create(&root.path().join("nested"), &git).unwrap();
    assert_eq!(
        receipt.project_root,
        std::fs::canonicalize(root.path()).unwrap()
    );
    assert!(receipt.branch.starts_with("codex/claude_tui_"));
    assert_ne!(receipt.worktree, receipt.project_root);
    assert_eq!(
        std::fs::read_to_string(receipt.worktree.join("README.md")).unwrap(),
        "committed baseline\n"
    );
    assert_eq!(
        git_output(
            &git,
            root.path(),
            &["status".as_ref(), "--porcelain=v1".as_ref()]
        ),
        before_status
    );
    assert_eq!(
        git_output(&git, root.path(), &["rev-parse".as_ref(), "HEAD".as_ref()]),
        before_head
    );
    assert_eq!(
        git_output(
            &git,
            root.path(),
            &["branch".as_ref(), "--show-current".as_ref()]
        ),
        before_branch
    );
    assert!(
        git_output(
            &git,
            &receipt.worktree,
            &["status".as_ref(), "--porcelain=v1".as_ref()]
        )
        .is_empty()
    );
    let record = root
        .path()
        .join(".vibemux/native_terminal_sessions")
        .join(format!("{}.json", receipt.session_id));
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(record).unwrap()).unwrap();
    assert_eq!(record["schema_version"], 1);
    assert_eq!(record["branch"], receipt.branch);
    // Test-owned, verified-clean resources only; production never does this automatically.
    git_output(
        &git,
        root.path(),
        &[
            "worktree".as_ref(),
            "remove".as_ref(),
            receipt
                .worktree
                .strip_prefix(&receipt.project_root)
                .unwrap()
                .as_os_str(),
        ],
    );
}

#[test]
fn unignored_runtime_directory_is_rejected_before_resource_creation() {
    let root = tempfile::tempdir().unwrap();
    let git = vibemux_terminal::client::find_executable("git")
        .expect("Git required for worktree contract tests");
    git_output(&git, root.path(), &["init".as_ref(), "--quiet".as_ref()]);
    assert_eq!(
        vibemux_terminal::workspace::create(root.path(), &git)
            .unwrap_err()
            .0,
        "terminal_runtime_directory_not_ignored"
    );
    assert!(!root.path().join(".vibemux").exists());
}
