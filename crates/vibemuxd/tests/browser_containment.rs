#![cfg(all(windows, feature = "test_helpers"))]
//! NativeProcess/Job browser lifecycle diagnosis without candidate content.

use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};
use uuid::Uuid;
use vibemux_harness::dispatch::launch_spec::SYSTEM_ENVIRONMENT_NAMES;
use vibemux_platform::ProcessTree;
use vibemuxd::harness_dispatch::native_process::{
    BrowserProbePlan, BrowserProbeResult, observe_browser_probe_output, run_browser_lifecycle_probe,
};

const TRAMPOLINE: &str = env!("CARGO_BIN_EXE_vibemux_launch_trampoline");
const LIFECYCLE_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/taskboard_lite/verifier/browser_lifecycle.mjs"
);

struct Scratch(Option<PathBuf>);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "vibemux_browser_containment_{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&path).expect("create owned browser scratch");
        Self(Some(fs::canonicalize(path).expect("canonical scratch")))
    }

    fn path(&self) -> &Path {
        self.0.as_deref().expect("scratch retained")
    }

    fn cleanup(mut self) {
        let path = self.0.take().expect("owned scratch");
        remove_owned_scratch(&path).expect("remove only owned browser scratch after Job exit");
    }

    fn retain(mut self) {
        self.0.take();
        eprintln!(
            "browser diagnostic: owned scratch retained because process cleanup is unverified"
        );
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = remove_owned_scratch(&path);
        }
    }
}

/// Windows can retain a just-exited browser file handle briefly after Job
/// accounting reaches zero. Retry only that observed sharing violation, with
/// a deadline and only on this test's unique scratch directory.
fn remove_owned_scratch(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) if error.raw_os_error() == Some(32) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error),
        }
    }
}

fn find_node() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH for Node fixture");
    std::env::split_paths(&path)
        .map(|directory| directory.join("node.exe"))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| fs::canonicalize(candidate).ok())
        .expect("Node 22+ must be installed for browser fixture")
}

fn find_browser(path: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(path);
    candidate
        .is_file()
        .then(|| fs::canonicalize(candidate).ok())
        .flatten()
}

fn find_default_browser() -> PathBuf {
    find_browser("C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe")
        .or_else(|| find_browser("C:/Program Files/Google/Chrome/Application/chrome.exe"))
        .expect("native Edge or Chrome is required for browser containment")
}

fn verifier_environment_names() -> Vec<String> {
    let mut names: Vec<String> = SYSTEM_ENVIRONMENT_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    for name in [
        "PATH",
        "LOCALAPPDATA",
        "APPDATA",
        "USERPROFILE",
        "PROGRAMDATA",
        "SYSTEMDRIVE",
        "PROGRAMFILES",
        "HOME",
    ] {
        if std::env::var_os(name).is_some() {
            names.push(name.to_string());
        }
    }
    names
}

fn plan(
    scratch: &Scratch,
    browser: PathBuf,
    node: PathBuf,
    hold_after_attach: bool,
) -> BrowserProbePlan {
    BrowserProbePlan {
        trampoline: fs::canonicalize(TRAMPOLINE).expect("test trampoline"),
        node,
        script: fs::canonicalize(LIFECYCLE_SCRIPT).expect("pinned fixture script"),
        browser,
        scratch: scratch.path().to_path_buf(),
        environment_names: verifier_environment_names(),
        hold_after_attach,
    }
}

#[derive(Debug)]
struct DirectLauncherObservation {
    probe: BrowserProbeResult,
    node_pid: Option<u32>,
    exit_code: Option<i32>,
    job_members_after_finish: Option<u32>,
}

// This bypasses NativeProcess's trampoline but deliberately retains the same
// Job ownership boundary. The start gate prevents Node from launching Edge
// until it has been assigned to the test-owned Job.
async fn run_direct_launcher_contained_probe(
    scratch: &Scratch,
    browser: &Path,
    node: &Path,
) -> Result<DirectLauncherObservation, String> {
    const OUTPUT_LIMIT: usize = 64 * 1024;
    const PROBE_TIMEOUT: Duration = Duration::from_secs(75);
    fs::canonicalize(LIFECYCLE_SCRIPT).map_err(|_| "script_missing")?;
    let mut command = Command::new(node);
    command
        // Match NativeProcess's vendor-facing path form. Node's module loader
        // receives the verified entry without a Windows verbatim prefix.
        .arg(LIFECYCLE_SCRIPT)
        .arg("--browser")
        .arg(browser)
        .arg("--temp_root")
        .arg(scratch.path())
        .arg("--wait_for_start")
        .env_clear()
        .envs(resolve_probe_environment(scratch.path()))
        .current_dir(scratch.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    ProcessTree::prepare_command(command.as_std_mut());
    let mut child = command.spawn().map_err(|_| "node_spawn_failed")?;
    let node_pid = child.id();
    let Some(mut tree) = node_pid.and_then(|pid| ProcessTree::contain(pid).ok()) else {
        let _ = child.start_kill();
        if timeout(Duration::from_secs(3), child.wait()).await.is_err() {
            return Err("node_exit_unverified".to_string());
        }
        return Err("job_assignment_failed".to_string());
    };
    let Some(mut stdin) = child.stdin.take() else {
        if !terminate_direct_child(&mut tree, &mut child).await {
            return Err("node_exit_unverified".into());
        }
        return Err("stdin_missing".to_string());
    };
    if stdin.write_all(&[1]).await.is_err() {
        drop(stdin);
        if !terminate_direct_child(&mut tree, &mut child).await {
            return Err("node_exit_unverified".into());
        }
        return Err("start_gate_failed".to_string());
    }
    drop(stdin);
    let Some(stdout) = child.stdout.take() else {
        if !terminate_direct_child(&mut tree, &mut child).await {
            return Err("node_exit_unverified".into());
        }
        return Err("stdout_missing".to_string());
    };
    let mut output = Vec::new();
    let capture = timeout(PROBE_TIMEOUT, async {
        stdout
            .take((OUTPUT_LIMIT + 1) as u64)
            .read_to_end(&mut output)
            .await?;
        if output.len() > OUTPUT_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "probe output limit",
            ));
        }
        child.wait().await
    })
    .await;
    let status = match capture {
        Ok(Ok(status)) => Some(status.code()),
        _ => {
            if !terminate_direct_child(&mut tree, &mut child).await {
                return Err("node_exit_unverified".into());
            }
            None
        }
    };
    let _ = tree.terminate();
    let member_deadline = Instant::now() + Duration::from_secs(3);
    let mut active_members = tree.active_process_count().ok();
    while active_members.is_some_and(|count| count != 0) && Instant::now() < member_deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
        active_members = tree.active_process_count().ok();
    }
    Ok(DirectLauncherObservation {
        probe: observe_browser_probe_output(&output),
        node_pid,
        exit_code: status.flatten(),
        job_members_after_finish: active_members,
    })
}

async fn terminate_direct_child(tree: &mut ProcessTree, child: &mut tokio::process::Child) -> bool {
    let _ = tree.terminate();
    let _ = child.start_kill();
    if matches!(
        timeout(Duration::from_secs(3), child.wait()).await,
        Ok(Ok(_))
    ) {
        return true;
    }
    let _ = tree.terminate();
    matches!(
        timeout(Duration::from_secs(3), child.wait()).await,
        Ok(Ok(_))
    )
}

fn resolve_probe_environment(scratch: &Path) -> Vec<(String, std::ffi::OsString)> {
    let mut environment: Vec<_> = verifier_environment_names()
        .into_iter()
        .filter_map(|name| std::env::var_os(&name).map(|value| (name, value)))
        .collect();
    let scratch = scratch.as_os_str().to_os_string();
    environment.push(("TEMP".to_string(), scratch.clone()));
    environment.push(("TMP".to_string(), scratch));
    environment
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_lifecycle_diagnostic_compares_native_and_direct_job_contained_launchers() {
    let Some(edge) = find_browser("C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe")
    else {
        eprintln!("browser diagnostic: edge=not_installed; remaining legs=not_run");
        return;
    };
    let node = find_node();
    let chrome = find_browser("C:/Program Files/Google/Chrome/Application/chrome.exe");
    let edge_scratch = Scratch::new();
    match run_browser_lifecycle_probe(plan(&edge_scratch, edge.clone(), node.clone(), false)).await
    {
        Ok(result) => eprintln!("browser diagnostic: edge_native_process_job_contained={result:?}"),
        Err(_) => eprintln!("browser diagnostic: edge_native_process_job_contained=launch_failed"),
    }
    edge_scratch.cleanup();
    let scratch = Scratch::new();
    let retain_scratch = match run_direct_launcher_contained_probe(&scratch, &edge, &node).await {
        Ok(observation) => {
            eprintln!(
                "browser diagnostic: edge_direct_launcher_job_contained={{probe:{:?},node_pid:{:?},exit_code:{:?},job_members_after_finish:{:?}}}",
                observation.probe,
                observation.node_pid,
                observation.exit_code,
                observation.job_members_after_finish
            );
            observation.job_members_after_finish != Some(0)
        }
        Err(class) => {
            eprintln!("browser diagnostic: edge_direct_launcher_job_contained={class}");
            class == "node_exit_unverified"
        }
    };
    if retain_scratch {
        scratch.retain();
    } else {
        scratch.cleanup();
    }
    if let Some(chrome) = chrome {
        let chrome_scratch = Scratch::new();
        match run_browser_lifecycle_probe(plan(&chrome_scratch, chrome, node.clone(), false)).await
        {
            Ok(result) => eprintln!("browser diagnostic: chrome_contained={result:?}"),
            Err(_) => eprintln!("browser diagnostic: chrome_contained=launch_failed"),
        }
        chrome_scratch.cleanup();
    } else {
        eprintln!("browser diagnostic: chrome_contained=not_installed");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "renderer CDP enable is blocked by Windows browser sandbox access denial inside the contained Job on this host"]
async fn browser_cdp_page_lifecycle_is_usable_inside_native_process() {
    let scratch = Scratch::new();
    let result =
        run_browser_lifecycle_probe(plan(&scratch, find_default_browser(), find_node(), false))
            .await
            .expect("contained probe launch");
    eprintln!("browser lifecycle evidence: {result:?}");
    assert_eq!(
        result.active_members_after_finish,
        Some(0),
        "Job members must be gone"
    );
    assert!(result.phases.iter().any(|phase| phase == "browser_ready"));
    assert!(result.phases.iter().any(|phase| phase == "target_created"));
    assert!(result.phases.iter().any(|phase| phase == "target_attached"));
    assert!(result.phases.iter().any(|phase| phase == "runtime_enabled"));
    assert!(
        result
            .phases
            .iter()
            .any(|phase| phase == "runtime_evaluated")
    );
    assert!(
        result.phases.iter().any(|phase| phase == "page_enabled"),
        "contained Page.enable did not finish; check content-free lifecycle evidence above"
    );
    assert_eq!(result.exit.exit_code, Some(0));
    scratch.cleanup();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_contained_browser_removes_every_job_member() {
    let scratch = Scratch::new();
    let result =
        run_browser_lifecycle_probe(plan(&scratch, find_default_browser(), find_node(), true))
            .await
            .expect("contained cancellation probe launch");
    eprintln!("browser cancellation evidence: {result:?}");
    assert!(result.phases.iter().any(|phase| phase == "held_for_cancel"));
    assert!(result.browser_pid.is_some());
    assert!(
        result
            .active_members_before_cancel
            .is_some_and(|count| count >= 2)
    );
    assert!(result.exit.forced_termination);
    assert_eq!(result.active_members_after_finish, Some(0));
    scratch.cleanup();
}
