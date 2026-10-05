#![cfg(all(windows, feature = "test_helpers"))]
//! NativeProcess/Job browser lifecycle diagnosis without candidate content.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use uuid::Uuid;
use vibemux_harness::dispatch::launch_spec::SYSTEM_ENVIRONMENT_NAMES;
use vibemuxd::harness_dispatch::native_process::{BrowserProbePlan, run_browser_lifecycle_probe};

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

fn find_browser() -> PathBuf {
    [
        "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
        "C:/Program Files/Google/Chrome/Application/chrome.exe",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|candidate| candidate.is_file())
    .and_then(|candidate| fs::canonicalize(candidate).ok())
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

fn plan(scratch: &Scratch, hold_after_attach: bool) -> BrowserProbePlan {
    BrowserProbePlan {
        trampoline: fs::canonicalize(TRAMPOLINE).expect("test trampoline"),
        node: find_node(),
        script: fs::canonicalize(LIFECYCLE_SCRIPT).expect("pinned fixture script"),
        browser: find_browser(),
        scratch: scratch.path().to_path_buf(),
        environment_names: verifier_environment_names(),
        hold_after_attach,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "renderer CDP enable is blocked by Windows browser sandbox access denial inside the contained Job on this host"]
async fn browser_cdp_page_lifecycle_is_usable_inside_native_process() {
    let scratch = Scratch::new();
    let result = run_browser_lifecycle_probe(plan(&scratch, false))
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
    let result = run_browser_lifecycle_probe(plan(&scratch, true))
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
