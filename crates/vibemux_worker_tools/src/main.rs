#![forbid(unsafe_code)]
//! Explicit component/qualification entry point; no daemon or global configuration.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
};
use vibemux_worker_tools::{
    contract::{Bootstrap, MAX_BOOTSTRAP_BYTES},
    owner_output::{finalize, finalize_test_session},
    server::serve,
    workspace::{WorkerTools, verify_private_parent},
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "__launch_trampoline") {
        std::process::exit(vibemux_platform::run_launch_trampoline(
            args.into_iter().skip(1),
        ));
    }
    if let Err(code) = run().await {
        eprintln!("{code}");
        std::process::exit(2);
    }
}

async fn run() -> Result<(), &'static str> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 6
        || args[0] != "--bootstrap"
        || args[2] != "--scratch-parent"
        || args[4] != "--candidate-out"
    {
        return Err("worker_tools_usage");
    }
    let bootstrap_path = PathBuf::from(&args[1]);
    let scratch_parent = PathBuf::from(&args[3]);
    let candidate_path = PathBuf::from(&args[5]);
    if !bootstrap_path.is_absolute()
        || !scratch_parent.is_absolute()
        || !candidate_path.is_absolute()
    {
        return Err("worker_tools_absolute_paths_required");
    }
    verify_private_parent(
        candidate_path
            .parent()
            .ok_or("worker_tools_candidate_unavailable")?,
    )
    .map_err(|error| error.code())?;
    #[cfg(windows)]
    vibemux_platform::verify_restricted_path_acl(&bootstrap_path)
        .map_err(|_| "worker_tools_bootstrap_not_private")?;
    let mut bytes = Vec::new();
    File::open(&bootstrap_path)
        .map_err(|_| "worker_tools_bootstrap_unavailable")?
        .take((MAX_BOOTSTRAP_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "worker_tools_bootstrap_unavailable")?;
    if bytes.len() > MAX_BOOTSTRAP_BYTES {
        return Err("worker_tools_bootstrap_limit");
    }
    let bootstrap: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "worker_tools_invalid_bootstrap")?;
    // Reserve owner output before any model-visible effect; never overwrite a prior result.
    let candidate_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&candidate_path)
        .map_err(|_| "worker_tools_candidate_unavailable")?;
    let publish = |output: &[u8]| {
        let mut temporary = tempfile::NamedTempFile::new_in(
            candidate_path
                .parent()
                .ok_or_else(|| std::io::Error::other("missing parent"))?,
        )?;
        temporary.write_all(output)?;
        temporary.as_file().sync_all()?;
        drop(candidate_file);
        temporary
            .persist(&candidate_path)
            .map_err(|error| error.error)?;
        Ok(())
    };
    if bootstrap["schema_version"] == 2 {
        let bootstrap =
            serde_json::from_slice::<vibemux_worker_tools::test_session::TestBootstrap>(&bytes)
                .map_err(|_| "worker_tools_invalid_bootstrap")?;
        let executable = std::env::current_exe().map_err(|_| "worker_tools_runner_unavailable")?;
        // Preparation is owned and joined; it may perform bounded synchronous
        // snapshot and private-directory I/O before serving the protocol.
        let session = tokio::task::spawn_blocking(move || {
            vibemux_worker_tools::test_session::TestSession::create(
                &scratch_parent,
                bootstrap,
                &executable,
            )
        })
        .await
        .map_err(|_| "worker_tools_bootstrap_failed")?
        .map_err(|error| error.code())?;
        let outcome = vibemux_worker_tools::test_server::serve_tests(
            &mut tokio::io::stdin(),
            &mut tokio::io::stdout(),
            session,
        )
        .await;
        finalize_test_session(outcome, publish)
    } else {
        let bootstrap: Bootstrap =
            serde_json::from_slice(&bytes).map_err(|_| "worker_tools_invalid_bootstrap")?;
        let mut tools =
            WorkerTools::create(&scratch_parent, bootstrap).map_err(|error| error.code())?;
        let transport = serve(
            &mut tokio::io::stdin(),
            &mut tokio::io::stdout(),
            &mut tools,
        )
        .await;
        finalize(&mut tools, transport.is_ok(), publish)
    }
}
