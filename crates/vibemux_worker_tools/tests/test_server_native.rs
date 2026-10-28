#![cfg(windows)]
use serde_json::{Value, json};
use std::{fs, path::PathBuf, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_worker_tools::{
    contract::{Bootstrap, TextFile},
    test_server::serve_tests,
    test_session::{TestBootstrap, TestPolicy, TestSession, TestSuite},
};
use vibemux_workflow::PathPattern;

fn fixture(code: &str) -> (PathBuf, TestSession) {
    let root = tempfile::TempDir::new().unwrap();
    vibemux_platform::secure_user_directory(root.path()).unwrap();
    let root = root.keep(); // A failed test retains evidence instead of deleting a live tree.
    let session = TestSession::create(
        &root,
        TestBootstrap {
            schema_version: 2,
            worker: Bootstrap {
                schema_version: 1,
                attempt_id: Uuid::new_v4(),
                grants: WritableGrant {
                    read_files: true,
                    edit_files: true,
                    write_files: true,
                    run_dev_tests: true,
                    ..WritableGrant::NONE
                },
                owned_paths: vec![PathPattern::new("src/**").unwrap()],
                read_scopes: vec![],
                forbidden_paths: vec![],
                files: vec![
                    TextFile {
                        path: "src/test.cjs".into(),
                        content: code.into(),
                    },
                    TextFile {
                        path: "src/original.txt".into(),
                        content: "original".into(),
                    },
                ],
            },
            test_policy: TestPolicy {
                backend: std::env::var_os("VIBEMUX_DEV_TEST_BACKEND")
                    .expect("explicit MXC")
                    .into(),
                node: std::env::var_os("VIBEMUX_DEV_TEST_NODE")
                    .expect("explicit Node")
                    .into(),
                suites: vec![TestSuite {
                    suite_id: "unit".into(),
                    test_entries: vec!["src/test.cjs".into()],
                    timeout_ms: 8000,
                    output_limit_bytes: 65536,
                }],
                max_runs: 4,
            },
        },
        &PathBuf::from(env!("CARGO_BIN_EXE_vibemux_worker_tools")),
    )
    .unwrap();
    (root, session)
}
fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}
fn run_request(id: u64) -> Value {
    request(
        id,
        "tools/call",
        json!({"name":"run_dev_tests","arguments":{"suite_id":"unit","expected_revision":0}}),
    )
}
async fn send<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, value: Value) {
    writer
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
}
async fn receive<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Value {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(12), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(line.len() <= 512 * 1024);
    serde_json::from_str(&line).unwrap()
}
async fn initialize<R: tokio::io::AsyncBufRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
) {
    send(writer,request(1,"initialize",json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"native_control","version":"1"}}))).await;
    assert_eq!(receive(reader).await["id"], 1);
    send(
        writer,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
}

#[tokio::test]
#[ignore = "requires selected installed Node/MXC; exercises real broker test execution"]
async fn test_output_is_bounded_and_copy_mutations_never_reach_candidate_or_replay() {
    let (root, session) = fixture(
        "require('node:fs').writeFileSync('src/original.txt','test-only');process.stdout.write('\\u0001'.repeat(60000));",
    );
    let (server, client) = tokio::io::duplex(1024 * 1024);
    let (mut input, mut output) = tokio::io::split(server);
    let (client_read, mut client_write) = tokio::io::split(client);
    let mut client_read = BufReader::new(client_read);
    let client = async {
        initialize(&mut client_read, &mut client_write).await;
        send(&mut client_write, run_request(2)).await;
        let reply = receive(&mut client_read).await;
        assert_eq!(reply["id"], 2);
        assert_eq!(reply["result"]["isError"], false);
        let result: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(result["assertions_verified"], false);
        assert_eq!(result["workspace_revision"], 0);
        assert!(result.get("retained_directory").is_none());
        send(&mut client_write, run_request(3)).await;
        let repeated = receive(&mut client_read).await;
        assert_eq!(repeated["result"]["isError"], true);
        assert!(repeated.to_string().contains("execution_already_admitted"));
        client_write.shutdown().await.unwrap();
        drop(client_write);
    };
    let (outcome, ()) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(serve_tests(&mut input, &mut output, session), client)
    })
    .await
    .unwrap();
    assert!(outcome.transport.is_ok());
    let mut session = outcome.session.unwrap();
    assert_eq!(session.receipts().len(), 1);
    assert!(
        session.receipts()[0]
            .report
            .as_ref()
            .unwrap()
            .snapshot_removed
    );
    assert!(session.tools.candidate().unwrap().changed_files.is_empty());
    let content = session
        .tools
        .execute(vibemux_worker_tools::contract::Operation::Read {
            path: "src/original.txt".into(),
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(content).unwrap()["content"],
        "original"
    );
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "requires selected installed Node/MXC; exercises real broker test execution"]
async fn busy_operations_are_never_queued_and_cancellation_suppresses_result() {
    let (root, session) = fixture("setInterval(()=>{},1000);");
    let (server, client) = tokio::io::duplex(65536);
    let (mut input, mut output) = tokio::io::split(server);
    let (client_read, mut client_write) = tokio::io::split(client);
    let mut client_read = BufReader::new(client_read);
    let client = async {
        initialize(&mut client_read, &mut client_write).await;
        let messages = [
            run_request(2),
            request(
                3,
                "tools/call",
                json!({"name":"create_file","arguments":{"path":"src/forbidden.txt","content":"must not run later"}}),
            ),
            request(4, "ping", json!({})),
        ];
        client_write
            .write_all(
                messages
                    .iter()
                    .map(|message| format!("{message}\n"))
                    .collect::<String>()
                    .as_bytes(),
            )
            .await
            .unwrap();
        let busy = receive(&mut client_read).await;
        assert_eq!(busy["id"], 3);
        assert_eq!(busy["error"]["message"], "worker_tools_busy");
        assert_eq!(receive(&mut client_read).await["id"], 4);
        tokio::time::sleep(Duration::from_millis(1200)).await;
        // String "2" must not cancel integer 2.
        send(
            &mut client_write,
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"2"}}),
        )
        .await;
        send(
            &mut client_write,
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(1000)).await;
        send(&mut client_write, request(5, "ping", json!({}))).await;
        assert_eq!(receive(&mut client_read).await["id"], 5); // no result for cancelled id2
        client_write.shutdown().await.unwrap();
        drop(client_write);
    };
    let (outcome, ()) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(serve_tests(&mut input, &mut output, session), client)
    })
    .await
    .unwrap();
    assert!(outcome.transport.is_ok());
    let mut session = outcome.session.unwrap();
    let report = session.receipts()[0].report.as_ref().unwrap();
    assert_eq!(
        report.execution,
        vibemux_worker_tools::dev_tests::ExecutionOutcome::Cancelled
    );
    assert!(report.process_tree_empty && report.snapshot_removed);
    assert!(session.tools.candidate().unwrap().changed_files.is_empty());
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "requires selected installed Node/MXC; exercises real broker test execution"]
async fn eof_and_partial_frame_failure_join_execution_before_returning() {
    for partial in [false, true] {
        let (root, session) = fixture("setInterval(()=>{},1000);");
        let (server, client) = tokio::io::duplex(65536);
        let (mut input, mut output) = tokio::io::split(server);
        let (client_read, mut client_write) = tokio::io::split(client);
        let mut client_read = BufReader::new(client_read);
        let client = async {
            initialize(&mut client_read, &mut client_write).await;
            send(&mut client_write, run_request(2)).await;
            tokio::time::sleep(Duration::from_millis(1200)).await;
            if partial {
                client_write.write_all(b"{").await.unwrap();
            }
            client_write.shutdown().await.unwrap();
            drop(client_write);
        };
        let (outcome, ()) = tokio::time::timeout(Duration::from_secs(15), async {
            tokio::join!(serve_tests(&mut input, &mut output, session), client)
        })
        .await
        .unwrap();
        assert_eq!(
            outcome.transport,
            if partial {
                Err("incomplete_frame")
            } else {
                Ok(())
            }
        );
        let session = outcome.session.unwrap();
        let report = session.receipts()[0].report.as_ref().unwrap();
        assert_eq!(
            report.execution,
            vibemux_worker_tools::dev_tests::ExecutionOutcome::Cancelled
        );
        assert!(report.process_tree_empty && report.snapshot_removed);
        if partial {
            assert!(session.tools.recovery_record().is_some());
        }
        drop(session);
        fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
#[ignore = "requires selected installed Node/MXC; exercises real broker test execution"]
async fn blocked_output_does_not_abandon_running_test() {
    let (root, session) = fixture("setInterval(()=>{},1000);");
    let (server, client) = tokio::io::duplex(64);
    let (mut input, mut output) = tokio::io::split(server);
    let (client_read, mut client_write) = tokio::io::split(client);
    let mut client_read = BufReader::new(client_read);
    let client = async {
        initialize(&mut client_read, &mut client_write).await;
        send(&mut client_write, run_request(2)).await;
        send(&mut client_write, request(3, "tools/list", json!({}))).await;
        tokio::time::sleep(Duration::from_millis(5600)).await;
        drop((client_read, client_write));
    };
    let (outcome, ()) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(serve_tests(&mut input, &mut output, session), client)
    })
    .await
    .unwrap();
    assert_eq!(outcome.transport, Err("output_timeout"));
    let session = outcome.session.unwrap();
    let report = session.receipts()[0].report.as_ref().unwrap();
    assert_eq!(
        report.execution,
        vibemux_worker_tools::dev_tests::ExecutionOutcome::Cancelled
    );
    assert!(report.process_tree_empty && report.snapshot_removed);
    assert!(session.tools.recovery_record().is_some());
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "requires selected installed Node/MXC; startup-only control without supervisor or latch"]
async fn baseline_offline_mxc_starts_a_fixed_inline_marker() {
    use std::{ffi::OsString, process::Stdio};
    use tokio::io::AsyncReadExt;
    use vibemux_platform::{
        LAUNCH_GO_BYTE, ProcessTree,
        process_stdio::spawn_with_async_output,
        sandbox_mxc::{MxcOfflinePolicy, offline_command},
    };
    let root = tempfile::TempDir::new().unwrap();
    vibemux_platform::secure_user_directory(root.path()).unwrap();
    let root = root.keep();
    let workspace = root.join("workspace");
    let scratch = workspace.join("scratch");
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(&scratch).unwrap();
    let marker = format!("MXC_STARTUP_{}", Uuid::new_v4().simple());
    let backend = PathBuf::from(std::env::var_os("VIBEMUX_DEV_TEST_BACKEND").unwrap())
        .canonicalize()
        .unwrap();
    let node = PathBuf::from(std::env::var_os("VIBEMUX_DEV_TEST_NODE").unwrap())
        .canonicalize()
        .unwrap();
    let desired = offline_command(
        &backend,
        &node,
        &[
            OsString::from("-e"),
            OsString::from(format!("console.log('{}')", marker)),
        ],
        &MxcOfflinePolicy {
            workspace: workspace.canonicalize().unwrap(),
            scratch: scratch.canonicalize().unwrap(),
            read_only_paths: vec![],
            denied_paths: vec![],
            profile_id: format!("rd_mxc_{}", Uuid::new_v4().simple()),
        },
    )
    .unwrap();
    let trampoline = std::env::var_os("VIBEMUX_TEST_TRAMPOLINE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_vibemux_worker_tools")));
    let mut command = tokio::process::Command::new(trampoline);
    command
        .arg("__launch_trampoline")
        .arg(desired.get_program())
        .args(desired.get_args())
        .env_clear()
        .envs(
            desired
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .current_dir(&workspace)
        .stdin(Stdio::piped())
        .creation_flags(0x0800_0000 | 0x0000_0200);
    ProcessTree::prepare_command(command.as_std_mut());
    let mut spawned = spawn_with_async_output(&mut command).await.unwrap();
    let mut tree = ProcessTree::contain(spawned.child.id().unwrap()).unwrap();
    let mut streams = spawned.output;
    let mut input = spawned.child.stdin.take().unwrap();
    input.write_all(&[LAUNCH_GO_BYTE]).await.unwrap();
    input.flush().await.unwrap();
    drop(input);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut limited_stdout = (&mut streams.stdout).take(4097);
        let mut limited_stderr = (&mut streams.stderr).take(4097);
        tokio::join!(
            spawned.child.wait(),
            limited_stdout.read_to_end(&mut stdout),
            limited_stderr.read_to_end(&mut stderr)
        )
    })
    .await;
    let completed = result.as_ref().is_ok_and(|(status, a, b)| {
        status.as_ref().is_ok_and(|status| status.success()) && a.is_ok() && b.is_ok()
    });
    tree.terminate().unwrap();
    let reaped = tokio::time::timeout(Duration::from_secs(3), spawned.child.wait())
        .await
        .is_ok_and(|value| value.is_ok());
    let empty = tree.active_process_count().unwrap() == 0;
    drop((streams, spawned.child, tree));
    let observed = String::from_utf8_lossy(&stdout).trim() == marker;
    if completed && reaped && empty && observed {
        fs::remove_dir_all(&root).unwrap();
    }
    assert!(
        completed && reaped && empty && observed,
        "startup control failed: complete={completed}, reaped={reaped}, tree_empty={empty}, marker={observed}, stderr={:?}, retained={:?}",
        String::from_utf8_lossy(&stderr),
        root
    );
}

#[tokio::test]
#[ignore = "requires selected installed Node/MXC; independent file-marker/stdio comparison"]
async fn baseline_mxc_file_marker_distinguishes_stdio_modes() {
    use std::{ffi::OsString, process::Stdio};
    use vibemux_platform::{
        LAUNCH_GO_BYTE, ProcessTree,
        process_stdio::spawn_with_async_output,
        sandbox_mxc::{MxcOfflinePolicy, offline_command},
    };
    let mut observations = Vec::new();
    for mode in ["null", "idle_piped", "iocp"] {
        let root = tempfile::TempDir::new().unwrap();
        vibemux_platform::secure_user_directory(root.path()).unwrap();
        let root = root.keep();
        let workspace = root.join("workspace");
        let scratch = workspace.join("scratch");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&scratch).unwrap();
        let marker = Uuid::new_v4().to_string();
        let backend = PathBuf::from(std::env::var_os("VIBEMUX_DEV_TEST_BACKEND").unwrap())
            .canonicalize()
            .unwrap();
        let node = PathBuf::from(std::env::var_os("VIBEMUX_DEV_TEST_NODE").unwrap())
            .canonicalize()
            .unwrap();
        let desired = offline_command(
            &backend,
            &node,
            &[
                OsString::from("-e"),
                OsString::from(format!(
                    "require('node:fs').writeFileSync('startup_marker','{}')",
                    marker
                )),
            ],
            &MxcOfflinePolicy {
                workspace: workspace.canonicalize().unwrap(),
                scratch: scratch.canonicalize().unwrap(),
                read_only_paths: vec![],
                denied_paths: vec![],
                profile_id: format!("rd_mxc_{}", Uuid::new_v4().simple()),
            },
        )
        .unwrap();
        let trampoline = std::env::var_os("VIBEMUX_TEST_TRAMPOLINE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_vibemux_worker_tools")));
        let mut command = tokio::process::Command::new(trampoline);
        command
            .arg("__launch_trampoline")
            .arg(desired.get_program())
            .args(desired.get_args())
            .env_clear()
            .envs(
                desired
                    .get_envs()
                    .filter_map(|(key, value)| value.map(|value| (key, value))),
            )
            .current_dir(&workspace)
            .stdin(Stdio::piped())
            .creation_flags(0x0800_0000 | 0x0000_0200)
            .kill_on_drop(true);
        ProcessTree::prepare_command(command.as_std_mut());
        let (mut child, streams) = if mode == "iocp" {
            let spawned = spawn_with_async_output(&mut command).await.unwrap();
            (spawned.child, Some(spawned.output))
        } else {
            command
                .stdout(if mode == "null" {
                    Stdio::null()
                } else {
                    Stdio::piped()
                })
                .stderr(if mode == "null" {
                    Stdio::null()
                } else {
                    Stdio::piped()
                });
            (command.spawn().unwrap(), None)
        };
        let mut tree = ProcessTree::contain(child.id().unwrap()).unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(&[LAUNCH_GO_BYTE]).await.unwrap();
        input.flush().await.unwrap();
        drop(input);
        let result = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        let code = result
            .as_ref()
            .ok()
            .and_then(|result| result.as_ref().ok())
            .and_then(|status| status.code());
        tree.terminate().unwrap();
        let reaped = tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .is_ok_and(|result| result.is_ok());
        let empty = tree.active_process_count().unwrap() == 0;
        drop((streams, child, tree));
        let observed =
            fs::read_to_string(workspace.join("startup_marker")).is_ok_and(|value| value == marker);
        let removed =
            code == Some(0) && reaped && empty && observed && fs::remove_dir_all(&root).is_ok();
        observations.push(json!({"mode":mode,"exit_code":code,"reaped":reaped,"tree_empty":empty,"file_marker":observed,"removed":removed,"root":root}));
    }
    eprintln!("{}", serde_json::to_string_pretty(&observations).unwrap());
    assert!(observations.iter().all(|value| value["exit_code"] == 0
        && value["file_marker"] == true
        && value["removed"] == true));
}

#[tokio::test]
#[ignore = "requires selected Node; fixed trusted trampoline control without a sandbox backend"]
async fn owned_trampoline_starts_a_trusted_file_marker() {
    use std::process::Stdio;
    use vibemux_platform::{LAUNCH_GO_BYTE, ProcessTree};
    let root = tempfile::TempDir::new().unwrap();
    let marker = Uuid::new_v4().to_string();
    let node = PathBuf::from(std::env::var_os("VIBEMUX_DEV_TEST_NODE").unwrap())
        .canonicalize()
        .unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_vibemux_worker_tools"));
    command
        .arg("__launch_trampoline")
        .arg(node)
        .args([
            "-e",
            &format!(
                "require('node:fs').writeFileSync('startup_marker','{}')",
                marker
            ),
        ])
        .current_dir(root.path())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000 | 0x0000_0200)
        .kill_on_drop(true);
    for name in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    ProcessTree::prepare_command(command.as_std_mut());
    let mut child = command.spawn().unwrap();
    let mut tree = ProcessTree::contain(child.id().unwrap()).unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(&[LAUNCH_GO_BYTE]).await.unwrap();
    input.flush().await.unwrap();
    drop(input);
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    tree.terminate().unwrap();
    let reaped = tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .is_ok_and(|value| value.is_ok());
    assert!(status.is_ok_and(|status| status.is_ok_and(|status| status.success())) && reaped);
    assert_eq!(tree.active_process_count().unwrap(), 0);
    assert_eq!(
        fs::read_to_string(root.path().join("startup_marker")).unwrap(),
        marker
    );
}
