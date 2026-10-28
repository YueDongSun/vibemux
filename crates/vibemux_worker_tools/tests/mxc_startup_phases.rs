#![cfg(windows)]
//! Explicit startup localization with the unchanged five-second control bound.
//! This is not reusable qualification and never modifies native sandbox policy.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinHandle,
    time::{Instant, sleep, timeout_at},
};
use uuid::Uuid;
use vibemux_platform::{
    LAUNCH_GO_BYTE, ProcessTree,
    process_stdio::spawn_with_async_output,
    sandbox_mxc::{MxcOfflinePolicy, offline_command},
};

const STARTUP_LIMIT: Duration = Duration::from_secs(5);
const EXTENDED_OBSERVATION_LIMIT: Duration = Duration::from_secs(30);
const CLEANUP_LIMIT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const OUTPUT_LIMIT: usize = 4096;
const MAX_OWNED_PROCESSES: usize = 64;

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    eof: bool,
    failed: bool,
    overflow: bool,
}

fn capture_output(
    mut stream: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) -> (Arc<Mutex<Capture>>, JoinHandle<()>) {
    let captured = Arc::new(Mutex::new(Capture::default()));
    let state = Arc::clone(&captured);
    let task = tokio::spawn(async move {
        let mut buffer = [0u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await;
            let mut state = state.lock().unwrap();
            match read {
                Ok(0) => {
                    state.eof = true;
                    break;
                }
                Ok(count) if state.bytes.len() + count <= OUTPUT_LIMIT => {
                    state.bytes.extend_from_slice(&buffer[..count])
                }
                Ok(_) => {
                    state.overflow = true;
                    break;
                }
                Err(_) => {
                    state.failed = true;
                    break;
                }
            }
        }
    });
    (captured, task)
}

fn metadata(path: &Path) -> Value {
    let value = fs::metadata(path).unwrap();
    json!({"path":path,"bytes":value.len(),
        "modified_ms":value.modified().ok().and_then(|time|time.duration_since(std::time::UNIX_EPOCH).ok()).map(|duration|duration.as_millis())})
}

fn marker_matches(path: &Path, expected: &str) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut bytes = Vec::new();
    file.take(65).read_to_end(&mut bytes).is_ok() && bytes == expected.as_bytes()
}

fn observed_role(
    path: Option<&Path>,
    backend: &Path,
    node: &Path,
    trampoline: &Path,
) -> &'static str {
    let matches = |expected: &Path| {
        path.and_then(|path| path.canonicalize().ok()).as_deref() == Some(expected)
    };
    if matches(backend) {
        "backend"
    } else if matches(node) {
        "node"
    } else if matches(trampoline) {
        "trampoline"
    } else {
        "other_or_unreadable"
    }
}

fn startup_completed_in_time(
    marker_ms: Option<u128>,
    exit_ms: Option<u128>,
    exit_code: Option<i32>,
    output_eof_ms: Option<u128>,
) -> bool {
    exit_code == Some(0)
        && [marker_ms, exit_ms, output_eof_ms]
            .into_iter()
            .all(|value| value.is_some_and(|value| value < STARTUP_LIMIT.as_millis()))
}

#[test]
fn observations_after_the_startup_deadline_never_qualify() {
    assert!(startup_completed_in_time(
        Some(4500),
        Some(4600),
        Some(0),
        Some(4700)
    ));
    assert!(!startup_completed_in_time(
        Some(5000),
        Some(5000),
        Some(0),
        Some(5000)
    ));
    assert!(!startup_completed_in_time(
        Some(5134),
        Some(5134),
        Some(0),
        Some(5134)
    ));
    assert!(!startup_completed_in_time(
        Some(4500),
        Some(5134),
        Some(0),
        Some(4700)
    ));
    assert!(!startup_completed_in_time(
        Some(4500),
        Some(4600),
        Some(0),
        Some(5134)
    ));
    assert!(!startup_completed_in_time(
        Some(4500),
        Some(4600),
        Some(0),
        None
    ));
}

#[tokio::test]
#[ignore = "explicit installed backend/Node; optional audited context helper; diagnostic only"]
async fn fixed_marker_records_native_startup_phases() {
    let backend =
        PathBuf::from(std::env::var_os("VIBEMUX_DEV_TEST_BACKEND").expect("explicit backend"))
            .canonicalize()
            .unwrap();
    let node = PathBuf::from(std::env::var_os("VIBEMUX_DEV_TEST_NODE").expect("explicit Node"))
        .canonicalize()
        .unwrap();
    let trampoline = PathBuf::from(env!("CARGO_BIN_EXE_vibemux_worker_tools"))
        .canonicalize()
        .unwrap();
    let own_image = std::env::current_exe().unwrap().canonicalize().unwrap();
    let context_probe = std::env::var_os("VIBEMUX_MXC_CONTEXT_PROBE")
        .map(PathBuf::from)
        .map(|path| path.canonicalize())
        .transpose()
        .unwrap();
    // Optional host-owned query helper; absent metadata is never a verified
    // process security context. No helper is installed or generated here.
    let context = if let Some(probe) = &context_probe {
        let context_command = tokio::process::Command::new(probe)
            .arg("--emit-bootstrap")
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(Duration::from_secs(3), context_command).await {
            Ok(Ok(output)) if output.status.success() && output.stdout.len() <= OUTPUT_LIMIT => {
                serde_json::from_slice::<Value>(&output.stdout)
                    .unwrap_or(json!({"status":"invalid_output"}))
            }
            _ => json!({"status":"unavailable"}),
        }
    } else {
        json!({"status":"not_collected"})
    };
    let private = tempfile::Builder::new()
        .prefix("mxc_phase_")
        .tempdir()
        .unwrap();
    vibemux_platform::secure_user_directory(private.path()).unwrap();
    let private = private.keep();
    let workspace = private.join("workspace");
    let scratch = workspace.join("scratch");
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(&scratch).unwrap();
    let acl = vibemux_platform::verify_restricted_path_acl(&private)
        .unwrap()
        .restricted;
    let marker = Uuid::new_v4().simple().to_string();
    let marker_path = workspace.join("startup_marker");
    let desired=offline_command(&backend,&node,&[
        OsString::from("-e"),
        OsString::from(format!("require('node:fs').writeFileSync('startup_marker','{marker}');console.log('{marker}');")),
    ],&MxcOfflinePolicy {
        workspace:workspace.canonicalize().unwrap(),scratch:scratch.canonicalize().unwrap(),
        read_only_paths:vec![],denied_paths:vec![],profile_id:format!("rd_mxc_{}",Uuid::new_v4().simple()),
    }).unwrap();
    let environment_names: Vec<_> = desired
        .get_envs()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    let mut command = tokio::process::Command::new(&trampoline);
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
        .kill_on_drop(true)
        .creation_flags(0x0800_0000 | 0x0000_0200);
    ProcessTree::prepare_command(command.as_std_mut());
    let mut spawned = spawn_with_async_output(&mut command).await.unwrap();
    let root_pid = Pid::from_u32(spawned.child.id().unwrap());
    let mut tree = ProcessTree::contain(root_pid.as_u32()).unwrap();
    let (stdout, mut stdout_task) = capture_output(spawned.output.stdout);
    let (stderr, mut stderr_task) = capture_output(spawned.output.stderr);
    let mut input = spawned.child.stdin.take().unwrap();
    let started = Instant::now();
    input.write_all(&[LAUNCH_GO_BYTE]).await.unwrap();
    input.flush().await.unwrap();
    drop(input);
    // Opt-in timing observation only. The acceptance predicate remains five
    // seconds even if this diagnostic watches a late startup for longer.
    let observation_limit = match std::env::var("VIBEMUX_MXC_OBSERVATION_WINDOW")
        .ok()
        .as_deref()
    {
        None => STARTUP_LIMIT,
        Some("30s") => EXTENDED_OBSERVATION_LIMIT,
        _ => panic!("unsupported observation window"),
    };
    let deadline = started + observation_limit;
    let mut system = System::new();
    let mut seen = BTreeMap::<(u32, u64), Value>::new();
    let mut output_eof_ms = None;
    let mut marker_ms = None;
    let mut exit_ms = None;
    let mut exit_code = None;
    let mut max_job_count = 0;
    while Instant::now() < deadline {
        // Enumerate identities/parent relationships only; no command lines,
        // environment values, user identities or unrelated executable paths.
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        // PPID chains are diagnostic candidates, not Job membership proof.
        // Rebuild each sample and discard known older-child edges. Coarse
        // creation times still cannot authenticate every ancestry edge.
        let mut owned = BTreeSet::from([root_pid]);
        for _ in 0..MAX_OWNED_PROCESSES {
            let previous = owned.len();
            for (pid, process) in system.processes() {
                if owned.len() < MAX_OWNED_PROCESSES
                    && process.parent().is_some_and(|parent| {
                        owned.contains(&parent)
                            && system.process(parent).is_some_and(|parent_process| {
                                process.start_time() >= parent_process.start_time()
                            })
                    })
                {
                    owned.insert(*pid);
                }
            }
            if owned.len() == previous {
                break;
            }
        }
        let owned_list: Vec<_> = owned.into_iter().collect();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&owned_list),
            false,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
        );
        for pid in &owned_list {
            if let Some(process) = system.process(*pid) {
                seen.entry((pid.as_u32(), process.start_time())).or_insert_with(|| {
                    json!({"pid":pid.as_u32(),"classification":if *pid == root_pid { "retained_root" } else { "unverified_ancestry_candidate" },
                    "parent":process.parent().map(|parent|parent.as_u32()),
                    "image_name":process.name().to_string_lossy().chars().take(128).collect::<String>(),
                    "creation_epoch_seconds":process.start_time(),
                    "role":observed_role(process.exe(),&backend,&node,&trampoline),
                    "first_seen_ms":started.elapsed().as_millis()})
                });
            }
        }
        if marker_ms.is_none() && marker_matches(&marker_path, &marker) {
            marker_ms = Some(started.elapsed().as_millis());
        }
        if exit_ms.is_none()
            && let Some(status) = spawned.child.try_wait().unwrap()
        {
            exit_ms = Some(started.elapsed().as_millis());
            exit_code = status.code();
        }
        if output_eof_ms.is_none() && stdout.lock().unwrap().eof && stderr.lock().unwrap().eof {
            output_eof_ms = Some(started.elapsed().as_millis());
        }
        let count = tree.active_process_count().unwrap();
        max_job_count = max_job_count.max(count);
        if exit_ms.is_some()
            && count == 0
            && stdout.lock().unwrap().eof
            && stderr.lock().unwrap().eof
        {
            break;
        }
        sleep(POLL_INTERVAL).await;
    }
    let observed_ms = started.elapsed().as_millis();
    let count_before_stop = tree.active_process_count().ok();
    let cleanup_deadline = Instant::now() + CLEANUP_LIMIT;
    let termination_requested = tree.terminate().is_ok();
    let root_reaped = timeout_at(cleanup_deadline, spawned.child.wait())
        .await
        .is_ok_and(|result| result.is_ok());
    let mut tree_empty = false;
    while Instant::now() < cleanup_deadline {
        if tree.active_process_count().ok() == Some(0) {
            tree_empty = true;
            break;
        }
        sleep(POLL_INTERVAL).await;
    }
    if timeout_at(cleanup_deadline, &mut stdout_task)
        .await
        .is_err()
    {
        stdout_task.abort();
        let _ = stdout_task.await;
    }
    if timeout_at(cleanup_deadline, &mut stderr_task)
        .await
        .is_err()
    {
        stderr_task.abort();
        let _ = stderr_task.await;
    }
    drop((spawned.child, tree, system));
    let stdout = stdout.lock().unwrap();
    let stderr = stderr.lock().unwrap();
    let completed = startup_completed_in_time(marker_ms, exit_ms, exit_code, output_eof_ms)
        && root_reaped
        && tree_empty
        && stdout.eof
        && stderr.eof
        && !stdout.failed
        && !stderr.failed
        && !stdout.overflow
        && !stderr.overflow;
    // Preserve the complete failed fixture; only a successful startup may
    // attempt the same disposable-workspace cleanup acceptance check.
    let removal = completed.then(|| fs::remove_dir_all(&workspace));
    let removed = removal.as_ref().is_some_and(|value| value.is_ok());
    let removal_error = removal
        .as_ref()
        .and_then(|value| value.as_ref().err())
        .and_then(|error| error.raw_os_error());
    let success = completed && removed;
    let report = json!({
        "schema_version":1,"reusable_qualification":false,"diagnostic":"fixed_marker_startup_phases",
        "startup_limit_ms":STARTUP_LIMIT.as_millis(),"observation_limit_ms":observation_limit.as_millis(),"cleanup_limit_ms":CLEANUP_LIMIT.as_millis(),
        "backend_preflight_invoked":false,"root_private_acl_verified":acl,"context":context,
        "artifacts":{"test":metadata(&own_image),"trampoline":metadata(&trampoline),"backend":metadata(&backend),"node":metadata(&node),"context_probe":context_probe.as_deref().map(metadata)},
        "environment_names":environment_names,
        "release_written":true,"ancestry_is_job_membership_evidence":false,"sampled_processes":seen.into_values().collect::<Vec<_>>(),
        "max_job_count":max_job_count,"job_count_before_stop":count_before_stop,"observed_ms":observed_ms,
        "marker_ms":marker_ms,"root_exit_ms":exit_ms,"root_exit_code":exit_code,"output_eof_ms":output_eof_ms,
        "termination_requested":termination_requested,"root_reaped":root_reaped,"tree_empty":tree_empty,
        "stdout":{"bytes":stdout.bytes.len(),"eof":stdout.eof,"failed":stdout.failed,"overflow":stdout.overflow},
        "stderr":{"bytes":stderr.bytes.len(),"eof":stderr.eof,"failed":stderr.failed,"overflow":stderr.overflow},
        "workspace_removed":removed,"removal_attempted":removal.is_some(),"removal_os_error":removal_error,
        "passed":success,
    });
    fs::write(
        private.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    fs::write(private.join("stdout.bin"), &stdout.bytes).unwrap();
    fs::write(private.join("stderr.bin"), &stderr.bytes).unwrap();
    eprintln!("phase_report={}", private.join("report.json").display());
    eprintln!(
        "phase_summary={}",
        json!({"passed":success,"marker_ms":marker_ms,"root_exit_ms":exit_ms,
        "root_exit_code":exit_code,"root_reaped":root_reaped,"tree_empty":tree_empty,"max_job_count":max_job_count})
    );
    assert!(
        success,
        "unchanged startup control failed; retained exact phase report"
    );
}
