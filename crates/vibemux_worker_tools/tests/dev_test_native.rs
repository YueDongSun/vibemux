//! Explicit installed-runtime controls. Never part of an unattended default run.
#![cfg(windows)]

use std::{
    fs,
    net::{TcpListener, TcpStream},
    path::PathBuf,
    time::Duration,
};
use tempfile::TempDir;
use tokio::sync::watch;
use uuid::Uuid;
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_worker_tools::{
    contract::TextFile,
    dev_tests::{
        DEV_TEST_SCHEMA, DevTestRequest, DevTestRuntime, ExecutionOutcome, prepare_dev_tests,
    },
};
use vibemux_workflow::PathPattern;

fn runtime() -> DevTestRuntime {
    DevTestRuntime {
        backend: PathBuf::from(
            std::env::var_os("VIBEMUX_DEV_TEST_BACKEND")
                .expect("set explicit installed MXC backend"),
        ),
        node: PathBuf::from(
            std::env::var_os("VIBEMUX_DEV_TEST_NODE")
                .expect("set explicit installed Node executable"),
        ),
        trampoline: PathBuf::from(env!("CARGO_BIN_EXE_vibemux_dev_test")),
    }
}

fn root() -> TempDir {
    let root = TempDir::new().unwrap();
    vibemux_platform::secure_user_directory(root.path()).unwrap();
    root
}

fn request(code: String) -> DevTestRequest {
    DevTestRequest {
        schema_version: DEV_TEST_SCHEMA,
        attempt_id: Uuid::new_v4(),
        operation_id: Uuid::new_v4(),
        suite_id: "native_control".into(),
        grants: WritableGrant {
            read_files: true,
            run_dev_tests: true,
            ..WritableGrant::NONE
        },
        read_scopes: vec![
            PathPattern::new("tests/**").unwrap(),
            PathPattern::new("src/**").unwrap(),
        ],
        forbidden_paths: vec![],
        files: vec![
            TextFile {
                path: "tests/control.test.cjs".into(),
                content: code,
            },
            TextFile {
                path: "src/value.txt".into(),
                content: "original".into(),
            },
        ],
        test_entries: vec!["tests/control.test.cjs".into()],
        timeout_ms: 8000,
        output_limit_bytes: 16 * 1024,
    }
}

async fn run(
    root: &TempDir,
    input: DevTestRequest,
) -> vibemux_worker_tools::dev_tests::DevTestReport {
    let prepared = prepare_dev_tests(input, &runtime(), root.path()).unwrap();
    let (_owner, mut cancellation) = watch::channel(false);
    prepared.execute(&mut cancellation).await.finish()
}

fn clean(report: &vibemux_worker_tools::dev_tests::DevTestReport) {
    assert!(
        (!report.process_started || (report.process_exited && report.process_tree_empty))
            && report.snapshot_removed
    );
    assert!(report.retained_directory.is_none());
    assert!(!report.assertions_verified && !report.reusable_qualification);
}

#[tokio::test]
#[ignore = "requires explicitly selected installed Node and MXC; executes synthetic offline controls"]
async fn disposable_copy_and_offline_denials_with_parent_canaries() {
    let root = root();
    let marker = Uuid::new_v4().to_string();
    let outside = root.path().join("outside.txt");
    fs::write(&outside, &marker).unwrap();
    let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
    let v6 = TcpListener::bind("[::1]:0").unwrap();
    for listener in [&v4, &v6] {
        // Demonstrate the exact endpoint is live before the denied candidate call.
        let connection = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        drop(listener.accept().unwrap());
        drop(connection);
        listener.set_nonblocking(true).unwrap();
    }
    let code = format!(
        r#"
const fs=require('node:fs'),net=require('node:net'),assert=require('node:assert/strict');
require('node:test')('offline controls',async()=>{{
  assert.equal(fs.readFileSync('src/value.txt','utf8'),'original');
  fs.writeFileSync('src/value.txt','modified only in test copy');
  fs.writeFileSync('src/created.txt','discard this');
  for(const action of [()=>fs.readFileSync({outside}),()=>fs.writeFileSync({outside},'forbidden')])
    assert.throws(action,e=>['EPERM','EACCES'].includes(e.code));
  for(const [host,port] of [['127.0.0.1',{v4}],['::1',{v6}]]) await new Promise((resolve,reject)=>{{
    const s=net.connect({{host,port}}); const timer=setTimeout(()=>{{s.destroy();reject(Error('timeout'));}},1000);
    s.once('connect',()=>{{clearTimeout(timer);s.destroy();reject(Error('unexpected connection'));}});
    s.once('error',e=>{{clearTimeout(timer);s.destroy();['EPERM','EACCES'].includes(e.code)?resolve():reject(e);}});
  }});
  console.log('CONTROL_'+{marker});
}});
"#,
        outside = serde_json::to_string(&outside).unwrap(),
        v4 = v4.local_addr().unwrap().port(),
        v6 = v6.local_addr().unwrap().port(),
        marker = serde_json::to_string(&marker).unwrap()
    );
    let input = request(code);
    let identity = input.operation_id;
    let report = run(&root, input.clone()).await;
    assert_eq!(
        report.execution,
        ExecutionOutcome::Completed,
        "{}",
        report.untrusted_stderr
    );
    assert_eq!(
        report.backend_exit_code,
        Some(0),
        "{}",
        report.untrusted_stdout
    );
    assert!(report.output_complete && report.untrusted_stdout.contains(&marker));
    assert_eq!(report.operation_id, identity);
    assert_eq!(input.files[1].content, "original");
    assert_eq!(fs::read_to_string(outside).unwrap(), marker);
    for listener in [v4, v6] {
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    clean(&report);
}

#[tokio::test]
#[ignore = "requires explicitly selected installed Node and MXC; executes synthetic offline controls"]
async fn exit_zero_and_failing_assertion_are_execution_evidence_only() {
    let root = root();
    for (code, exit) in [
        ("process.exit(0);", 0),
        (
            "require('node:test')('fails',()=>{require('node:assert/strict').equal(1,2)});",
            1,
        ),
    ] {
        let report = run(&root, request(code.into())).await;
        assert_eq!(report.execution, ExecutionOutcome::Completed);
        assert_eq!(report.backend_exit_code, Some(exit));
        assert!(report.output_complete);
        clean(&report);
    }
}

#[tokio::test]
#[ignore = "requires explicitly selected installed Node and MXC; executes synthetic offline controls"]
async fn timeout_and_combined_output_overflow_stop_and_clean_the_job() {
    let root = root();
    let mut input = request("setInterval(()=>{},1000);".into());
    input.timeout_ms = 1500;
    let report = run(&root, input).await;
    assert_eq!(report.execution, ExecutionOutcome::TimedOut);
    clean(&report);
    let mut input = request("process.stdout.write('x'.repeat(32768));process.stderr.write('y'.repeat(32768));setInterval(()=>{},1000);".into());
    input.output_limit_bytes = 2048;
    let report = run(&root, input).await;
    assert_eq!(report.execution, ExecutionOutcome::OutputLimit);
    assert!(!report.output_complete);
    assert!(report.untrusted_stdout.len() + report.untrusted_stderr.len() <= 2048);
    clean(&report);
}

#[tokio::test]
#[ignore = "requires explicitly selected installed Node and MXC; executes synthetic offline controls"]
async fn cancellation_before_and_after_launch_reconciles_the_owned_job() {
    let root = root();
    for before in [true, false] {
        let prepared = prepare_dev_tests(
            request("setInterval(()=>{},1000);".into()),
            &runtime(),
            root.path(),
        )
        .unwrap();
        let (owner, mut cancellation) = watch::channel(before);
        let execution = prepared.execute(&mut cancellation);
        let cancel = async {
            if !before {
                tokio::time::sleep(Duration::from_millis(1200)).await;
                owner.send(true).unwrap();
            }
        };
        let (executed, ()) = tokio::join!(execution, cancel);
        let report = executed.finish();
        assert_eq!(report.execution, ExecutionOutcome::Cancelled);
        assert!(report.backend_exit_code.is_none());
        clean(&report);
    }
}

#[tokio::test]
#[ignore = "requires explicitly selected installed Node and MXC; executes synthetic offline controls"]
async fn descendant_that_closes_stdio_cannot_outlive_finalization() {
    let root = root();
    let code = "const c=require('node:child_process').spawn(process.execPath,['-e',\"setInterval(()=>require('node:fs').writeFileSync('src/ticking.txt',String(Date.now())),10)\"],{stdio:'ignore',detached:true,windowsHide:true}); c.unref();";
    let report = run(&root, request(code.into())).await;
    assert_eq!(
        report.execution,
        ExecutionOutcome::Completed,
        "{}",
        report.untrusted_stderr
    );
    assert_eq!(
        report.backend_exit_code,
        Some(0),
        "{}",
        report.untrusted_stdout
    );
    clean(&report);
}

#[tokio::test]
#[ignore = "requires explicitly selected installed Node and MXC; executes synthetic offline controls"]
async fn junction_creation_denial_and_inherited_pipe_handles_are_observed() {
    let root = root();
    let outside = root.path().join("outside_directory");
    fs::create_dir(&outside).unwrap();
    let marker = Uuid::new_v4().to_string();
    fs::write(outside.join("sentinel.txt"), &marker).unwrap();
    let code = format!(
        r#"
const fs=require('node:fs'),assert=require('node:assert/strict');
assert.throws(()=>fs.symlinkSync({outside},'src/junction','junction'),e=>['EACCES','EPERM'].includes(e.code));
fs.renameSync('src/value.txt','src/renamed.txt');
console.log('JUNCTION_DENIED');
const child=require('node:child_process').spawn(process.execPath,['-e','setInterval(()=>{{}},1000)'],{{stdio:'inherit',detached:true,windowsHide:true}});
child.unref();
"#,
        outside = serde_json::to_string(&outside).unwrap()
    );
    let report = run(&root, request(code)).await;
    // The inherited test-child pipe remains open. This must time out instead of
    // treating the first test process's exit as completed execution.
    assert_eq!(
        report.execution,
        ExecutionOutcome::TimedOut,
        "{}",
        report.untrusted_stderr
    );
    assert!(
        report.untrusted_stdout.contains("JUNCTION_DENIED"),
        "{}",
        report.untrusted_stdout
    );
    clean(&report);
    assert_eq!(
        fs::read_to_string(outside.join("sentinel.txt")).unwrap(),
        marker
    );
}

#[tokio::test]
#[ignore = "requires explicitly selected installed Node and MXC; executes synthetic offline controls"]
async fn cleanup_failure_retains_attribution_without_changing_execution_outcome() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = root();
    let prepared =
        prepare_dev_tests(request("process.exit(0);".into()), &runtime(), root.path()).unwrap();
    let (_owner, mut cancellation) = watch::channel(false);
    let executed = prepared.execute(&mut cancellation).await;
    let retained = executed
        .report()
        .retained_directory
        .as_ref()
        .unwrap()
        .clone();
    // Allow reading the trusted marker but deny its removal while this test
    // holds the file. No ACL change and no permissions repair are involved.
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(retained.join("owner.json"))
        .unwrap();
    let report = executed.finish();
    assert_eq!(report.execution, ExecutionOutcome::Completed);
    assert_eq!(report.backend_exit_code, Some(0));
    assert!(report.process_tree_empty && report.process_exited);
    assert!(!report.snapshot_removed);
    assert_eq!(report.retained_directory, Some(retained.clone()));
    let owner_record: serde_json::Value =
        serde_json::from_slice(&fs::read(retained.join("owner.json")).unwrap()).unwrap();
    assert_eq!(
        owner_record["operation_id"],
        report.operation_id.to_string()
    );
    drop(lock);
    // The temporary parent owns this known fixture and cleans it after the lock
    // is gone. Production never retries removal or reuses this snapshot.
}

#[tokio::test]
#[ignore = "requires explicitly selected installed Node and MXC; executes synthetic offline controls"]
async fn lost_owner_channel_refuses_launch_and_unpolled_drop_retains_snapshot() {
    let root = root();
    let input = request("throw Error('must never execute');".into());
    let prepared = prepare_dev_tests(input.clone(), &runtime(), root.path()).unwrap();
    let (owner, mut cancellation) = watch::channel(false);
    drop(owner);
    let report = prepared.execute(&mut cancellation).await.finish();
    assert_eq!(report.execution, ExecutionOutcome::Cancelled);
    assert!(!report.process_started);
    clean(&report);
    let prepared = prepare_dev_tests(input.clone(), &runtime(), root.path()).unwrap();
    let (_owner, mut cancellation) = watch::channel(false);
    drop(prepared.execute(&mut cancellation));
    let entries: Vec<_> = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1);
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(entries[0].join("owner.json")).unwrap()).unwrap();
    assert_eq!(marker["operation_id"], input.operation_id.to_string());
    assert!(
        entries[0]
            .join("workspace/tests/control.test.cjs")
            .is_file()
    );
}

#[tokio::test]
#[ignore = "requires explicitly selected installed Node and MXC; executes synthetic offline controls"]
async fn actual_command_runs_documented_example_and_refuses_existing_result() {
    use std::process::Stdio;
    let root = root();
    let example = include_str!("../../../examples/worker_tools/development_test.json");
    let mut input: DevTestRequest = serde_json::from_str(example).unwrap();
    input.attempt_id = Uuid::new_v4();
    input.operation_id = Uuid::new_v4();
    let request_file = root.path().join("request.json");
    let report_file = root.path().join("report.json");
    fs::write(&request_file, serde_json::to_vec(&input).unwrap()).unwrap();
    let selected = runtime();
    for repeat in [false, true] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_vibemux_dev_test"));
        command
            .arg("--request")
            .arg(&request_file)
            .arg("--scratch-parent")
            .arg(root.path())
            .arg("--backend")
            .arg(&selected.backend)
            .arg("--node")
            .arg(&selected.node)
            .arg("--report-out")
            .arg(&report_file)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .creation_flags(0x0800_0000);
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .unwrap()
            .unwrap();
        if repeat {
            assert!(!output.status.success());
            assert_eq!(
                String::from_utf8(output.stderr).unwrap().trim(),
                "dev_test_output_unavailable"
            );
        } else {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let report: vibemux_worker_tools::dev_tests::DevTestReport =
            serde_json::from_slice(&fs::read(&report_file).unwrap()).unwrap();
        assert_eq!(report.operation_id, input.operation_id);
        assert_eq!(report.execution, ExecutionOutcome::Completed);
        assert!(report.untrusted_stdout.contains("addition"));
        clean(&report);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }
}
