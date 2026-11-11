//! Operator-run native UI smoke. It uses the production command/window builders,
//! an incognito WebView, and a new output directory for all native settings.
//!
//! The script is a trusted local test plan, not model output. No script loading
//! or evaluation command is added to the production application's IPC surface.

#![forbid(unsafe_code)]

#[path = "native_ui_smoke/support.rs"]
mod support;

use support::{ExitRequest, Watchdog, write_status};

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{Manager, WebviewWindow};
use vibemux_desktop::app_state::AppState;
use vibemux_desktop::desktop_settings::SETTINGS_FILE_NAME;
use vibemux_desktop::event_sink::TauriEventSink;

const MAX_PLAN_BYTES: u64 = 128 * 1024;
const MAX_RESULT_BYTES: usize = 128 * 1024;
const RUN_DEADLINE: Duration = Duration::from_secs(120);
const STEP_DEADLINE: Duration = Duration::from_secs(15);
const EVAL_DEADLINE: Duration = Duration::from_secs(3);
const CAPTURE_ERRORS: &str = r#"
(() => {
  const state = { errors: [], phase: 'loading' };
  Object.defineProperty(window, '__vibemux_ui_smoke', { value: state });
  const record = (kind, message) => {
    if (state.errors.length < 16) state.errors.push({ kind, message: String(message).slice(0, 512) });
  };
  window.addEventListener('error', event => record('error', event.message));
  window.addEventListener('unhandledrejection', event => record('rejection', event.reason?.message ?? event.reason));
  window.addEventListener('securitypolicyviolation', event => record('csp', event.violatedDirective + ': ' + event.blockedURI));
})();
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema_version: u16,
    steps: Vec<Step>,
    inspect: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    name: String,
    action: Option<String>,
    until: String,
}

#[derive(Serialize)]
struct Report {
    schema_version: u16,
    succeeded: bool,
    completed_steps: Vec<String>,
    failure: Option<String>,
    inspection: Value,
    errors: Value,
}

struct Options {
    plan: PathBuf,
    output: PathBuf,
    hold_seconds: u64,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn options() -> io::Result<Options> {
    let mut arguments = std::env::args_os().skip(1);
    let mut plan = None;
    let mut output = None;
    let mut hold_seconds = 0;
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| invalid("missing argument value"))?;
        match argument.to_str() {
            Some("--script") => plan = Some(PathBuf::from(value)),
            Some("--output") => output = Some(PathBuf::from(value)),
            Some("--hold-seconds") => {
                hold_seconds = value
                    .to_str()
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|value| *value <= 180)
                    .ok_or_else(|| invalid("hold must be 0 to 180 seconds"))?;
            }
            _ => return Err(invalid("expected --script, --output, or --hold-seconds")),
        }
    }
    Ok(Options {
        plan: plan.ok_or_else(|| invalid("--script is required"))?,
        output: output.ok_or_else(|| invalid("--output is required"))?,
        hold_seconds,
    })
}

fn read_plan(path: &std::path::Path) -> io::Result<Plan> {
    if !fs::metadata(path)?.is_file() {
        return Err(invalid("test plan must be a regular file"));
    }
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_PLAN_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PLAN_BYTES {
        return Err(invalid("test plan exceeds 128 KiB"));
    }
    let plan: Plan = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("test plan is not valid JSON for this schema"))?;
    if plan.schema_version != 1
        || plan.steps.is_empty()
        || plan.steps.len() > 32
        || plan.inspect.len() > 16 * 1024
        || plan.steps.iter().any(|step| {
            step.name.is_empty()
                || step.name.len() > 128
                || step.until.len() > 16 * 1024
                || step
                    .action
                    .as_ref()
                    .is_some_and(|action| action.len() > 16 * 1024)
        })
    {
        return Err(invalid("invalid or oversized test plan"));
    }
    Ok(plan)
}

fn evaluate(window: &WebviewWindow, expression: &str) -> Result<Value, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let source = format!(
        "(() => {{ try {{ return {{ok:true,value:({expression})}}; }} catch (error) {{ return {{ok:false,error:String(error).slice(0,512)}}; }} }})()"
    );
    window
        .eval_with_callback(source, move |result| {
            let _ = sender.try_send(result);
        })
        .map_err(|_| "native evaluation could not be scheduled".to_owned())?;
    let text = receiver
        .recv_timeout(EVAL_DEADLINE)
        .map_err(|_| "native evaluation timed out".to_owned())?;
    if text.len() > MAX_RESULT_BYTES {
        return Err("native evaluation result exceeded its limit".to_owned());
    }
    let value: Value = serde_json::from_str(&text)
        .map_err(|_| "native evaluation returned invalid JSON".to_owned())?;
    if value["ok"] != true {
        return Err(format!("JavaScript: {}", value["error"]));
    }
    Ok(value["value"].clone())
}

fn wait_until(
    window: &WebviewWindow,
    expression: &str,
    run_deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let deadline = run_deadline.min(Instant::now() + STEP_DEADLINE);
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("the native window closed before the walkthrough completed".to_owned());
        }
        if Instant::now() >= deadline {
            return Err("the UI condition did not become true before its deadline".to_owned());
        }
        if evaluate(window, expression)? == true {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn walk(window: &WebviewWindow, plan: &Plan, cancelled: &AtomicBool) -> Report {
    let mut report = Report {
        schema_version: 1,
        succeeded: false,
        completed_steps: Vec::new(),
        failure: None,
        inspection: Value::Null,
        errors: Value::Null,
    };
    let deadline = Instant::now() + RUN_DEADLINE;
    for step in &plan.steps {
        if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
            report.failure =
                Some("native walkthrough was cancelled or exceeded its deadline".to_owned());
            break;
        }
        let outcome = step
            .action
            .as_ref()
            .map_or(Ok(Value::Null), |action| evaluate(window, action))
            .and_then(|_| wait_until(window, &step.until, deadline, cancelled));
        if let Err(error) = outcome {
            report.failure = Some(format!("{}: {error}", step.name));
            break;
        }
        report.completed_steps.push(step.name.clone());
    }
    match evaluate(window, &plan.inspect) {
        Ok(value) => report.inspection = value,
        Err(error) if report.failure.is_none() => report.failure = Some(error),
        Err(_) => {}
    }
    match evaluate(window, "window.__vibemux_ui_smoke?.errors ?? []") {
        Ok(value) => {
            if value.as_array().is_none_or(|errors| !errors.is_empty()) && report.failure.is_none()
            {
                report.failure =
                    Some("native page errors or CSP violations were recorded".to_owned());
            }
            report.errors = value;
        }
        Err(error) if report.failure.is_none() => report.failure = Some(error),
        Err(_) => {}
    }
    report.succeeded = report.failure.is_none() && report.completed_steps.len() == plan.steps.len();
    report
}

fn write_new_json(path: &std::path::Path, value: &impl Serialize) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.sync_all()
}

fn require_webview_runtime(available: bool, output: &std::path::Path) -> io::Result<()> {
    if available {
        return Ok(());
    }
    let report = Report {
        schema_version: 1,
        succeeded: false,
        completed_steps: Vec::new(),
        failure: Some(
            "The native WebView runtime is unavailable; no window was opened.".to_owned(),
        ),
        inspection: Value::Null,
        errors: Value::Array(Vec::new()),
    };
    write_new_json(&output.join("report.json"), &report)?;
    Err(io::Error::other("native WebView runtime unavailable"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = options()?;
    let plan = read_plan(&options.plan)?;
    fs::create_dir(&options.output)?;
    let output = fs::canonicalize(&options.output)?;
    let timeout_output = output.clone();
    let watchdog = Watchdog::start(
        Duration::from_secs(30),
        RUN_DEADLINE + Duration::from_secs(options.hold_seconds + 15) + EVAL_DEADLINE * 2,
        move |phase| {
            let report = serde_json::json!({
                "schema_version": 1,
                "succeeded": false,
                "phase": format!("{phase:?}"),
                "failure": "The native UI harness exceeded its owned deadline.",
                "clean_shutdown_verified": false,
            });
            let _ = write_new_json(&timeout_output.join("watchdog.json"), &report);
            let _ = write_new_json(&timeout_output.join("report.json"), &report);
            std::process::exit(124);
        },
    )?;
    let startup_ready = watchdog.ready_signal();
    require_webview_runtime(tauri::webview_version().is_ok(), &output)?;
    let title = format!("VibeMux UI smoke {}", uuid::Uuid::new_v4().simple());
    write_new_json(
        &output.join("process.json"),
        &serde_json::json!({"pid":std::process::id(), "window_title":title}),
    )?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker = Arc::new(Mutex::new(None));
    let setup_worker = Arc::clone(&worker);
    let worker_cancelled = Arc::clone(&cancelled);
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = "io.github.yuedongsun.vibemux.ui-smoke".to_owned();
    let app = vibemux_desktop::app_builder()
        .setup(move |app| {
            let handle = app.handle().clone();
            let sink = Arc::new(TauriEventSink::new(handle.clone()));
            app.manage(Arc::new(AppState::new(
                output.join(SETTINGS_FILE_NAME),
                sink,
            )));
            let (page_sender, page_loaded) = mpsc::sync_channel(1);
            let window = vibemux_desktop::windows::main_window_builder(&handle)
                .title(title)
                .data_directory(output.join("webview"))
                .incognito(true)
                .initialization_script(CAPTURE_ERRORS)
                .on_page_load(move |_, payload| {
                    if payload.event() == tauri::webview::PageLoadEvent::Finished
                        && vibemux_desktop::navigation::navigation_allowed(payload.url(), None)
                    {
                        let _ = page_sender.try_send(payload.url().to_string());
                    }
                })
                .build()?;
            let join = thread::spawn(move || {
                let mut exit_request = ExitRequest::new(move |code| handle.exit(code));
                // Wry's pre-navigation queue retains scripts, not callbacks.
                // Do not evaluate until the real bundled document has loaded.
                let report = match page_loaded.recv_timeout(Duration::from_secs(30)) {
                    Ok(url) => {
                        startup_ready();
                        let _ = write_new_json(
                            &output.join("page-load.json"),
                            &serde_json::json!({"url":url}),
                        );
                        walk(&window, &plan, &worker_cancelled)
                    }
                    Err(_) => Report {
                        schema_version: 1,
                        succeeded: false,
                        completed_steps: Vec::new(),
                        failure: Some("The bundled native page did not finish loading.".to_owned()),
                        inspection: Value::Null,
                        errors: Value::Null,
                    },
                };
                let written = write_new_json(&output.join("report.json"), &report);
                let code = if report.succeeded && written.is_ok() {
                    0
                } else {
                    1
                };
                exit_request.code(code);
                let _ = write_status(&mut io::stdout().lock(), code);
                let hold_deadline = Instant::now() + Duration::from_secs(options.hold_seconds);
                while Instant::now() < hold_deadline && !worker_cancelled.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(100));
                }
                code
            });
            *setup_worker
                .lock()
                .map_err(|_| io::Error::other("worker lock poisoned"))? = Some(join);
            Ok(())
        })
        .build(context)?;
    let exit = app.run_return(|_, _| {});
    cancelled.store(true, Ordering::Release);
    let join = worker
        .lock()
        .map_err(|_| io::Error::other("worker lock poisoned"))?
        .take();
    let worker_exit = join.map_or(1, |join| join.join().unwrap_or(1));
    drop(watchdog);
    if exit != 0 || worker_exit != 0 {
        return Err(io::Error::other("native UI walkthrough failed; inspect report.json").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unavailable_webview_fails_without_opening_a_window() {
        let directory = tempfile::tempdir().expect("owned output");
        assert!(require_webview_runtime(false, directory.path()).is_err());
        let report: Value = serde_json::from_slice(
            &fs::read(directory.path().join("report.json")).expect("failure report"),
        )
        .expect("JSON");
        assert_eq!(report["succeeded"], false);
        assert_eq!(report["completed_steps"], serde_json::json!([]));
        assert!(
            report["failure"]
                .as_str()
                .expect("failure")
                .contains("no window")
        );
    }
}
