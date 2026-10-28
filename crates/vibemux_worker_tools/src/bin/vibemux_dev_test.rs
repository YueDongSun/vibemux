//! Explicit owner command; not an MCP tool or daemon admission credential.
#![forbid(unsafe_code)]

use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    process::ExitCode,
};
use vibemux_worker_tools::{
    contract::MAX_BOOTSTRAP_BYTES,
    dev_tests::{DevTestRequest, DevTestRuntime, ExecutionOutcome, prepare_dev_tests},
    workspace::verify_private_parent,
};

const USAGE: &str = "usage: vibemux_dev_test --request <absolute JSON> --scratch-parent <private directory> --backend <absolute codex.exe> --node <absolute node.exe> --report-out <new absolute JSON>";

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|value| value == "__launch_trampoline")
    {
        return ExitCode::from(
            u8::try_from(vibemux_platform::run_launch_trampoline(
                args.into_iter().skip(1),
            ))
            .unwrap_or(123),
        );
    }
    if args == [OsString::from("--help")] {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(code) => {
            eprintln!("{code}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[OsString]) -> Result<u8, &'static str> {
    let keys = [
        "--request",
        "--scratch-parent",
        "--backend",
        "--node",
        "--report-out",
    ];
    if args.len() != keys.len() * 2
        || keys
            .iter()
            .enumerate()
            .any(|(index, key)| args[index * 2] != *key)
    {
        return Err("dev_test_usage");
    }
    let paths: Vec<PathBuf> = args.iter().skip(1).step_by(2).map(PathBuf::from).collect();
    if paths.iter().any(|path| !path.is_absolute()) {
        return Err("dev_test_absolute_paths_required");
    }
    let report_path = &paths[4];
    let report_parent = report_path.parent().ok_or("dev_test_output_unavailable")?;
    verify_private_parent(report_parent).map_err(|error| error.code())?;
    #[cfg(windows)]
    vibemux_platform::verify_restricted_path_acl(&paths[0])
        .map_err(|_| "dev_test_request_not_private")?;
    let mut bytes = vec![];
    File::open(&paths[0])
        .map_err(|_| "dev_test_request_unavailable")?
        .take((MAX_BOOTSTRAP_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "dev_test_request_unavailable")?;
    if bytes.len() > MAX_BOOTSTRAP_BYTES {
        return Err("dev_test_request_limit");
    }
    let request: DevTestRequest =
        serde_json::from_slice(&bytes).map_err(|_| "dev_test_invalid_request")?;
    request.validate().map_err(|error| error.code())?;
    // Refuse an existing result before snapshot construction or execution.
    let reservation = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(report_path)
        .map_err(|_| "dev_test_output_unavailable")?;
    let prepared = prepare_dev_tests(
        request,
        &DevTestRuntime {
            backend: paths[2].clone(),
            node: paths[3].clone(),
            trampoline: std::env::current_exe().map_err(|_| "dev_test_runner_unavailable")?,
        },
        &paths[1],
    )
    .map_err(|error| error.code())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "dev_test_runtime_unavailable")?;
    let (_cancellation_owner, mut cancellation) = tokio::sync::watch::channel(false);
    let executed = runtime.block_on(prepared.execute(&mut cancellation));
    drop(runtime);
    let report = executed.finish();
    let exit = if report.execution == ExecutionOutcome::Completed
        && report.backend_exit_code == Some(0)
        && report.process_exited
        && report.process_tree_empty
        && report.output_complete
        && report.snapshot_removed
    {
        0
    } else {
        2
    };
    let encoded =
        serde_json::to_vec_pretty(&report).map_err(|_| "dev_test_output_encoding_failed")?;
    let mut temporary = tempfile::NamedTempFile::new_in(report_parent)
        .map_err(|_| "dev_test_output_unavailable")?;
    temporary
        .write_all(&encoded)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| "dev_test_output_unavailable")?;
    drop(reservation);
    temporary
        .persist(report_path)
        .map_err(|_| "dev_test_output_unavailable")?;
    // Exit zero means clean execution/cleanup only; see assertions_verified=false.
    Ok(exit)
}
