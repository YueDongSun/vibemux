//! Read-only probe entry point.
//!
//! Default behavior (no flags) is unchanged: run the probe and print the
//! report JSON to stdout. With `--write-cache` the report is additionally
//! persisted atomically to `<project-root>/.vibemux/probe_cache.json` (the
//! trusted cache the daemon reads) before the JSON is printed. Unknown or
//! malformed flags print to stderr and exit with code 4, matching the
//! existing failure convention.

use std::{path::PathBuf, process::exit};

use vibemux_probe::{ProbeConfig, cache, report_json, run_probe};

const USAGE: &str = "usage: vibemux_probe [--write-cache] [--project-root <dir>]";

#[tokio::main]
async fn main() {
    let mut write_cache = false;
    let mut project_root = PathBuf::from(".");
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--write-cache" => write_cache = true,
            "--project-root" => {
                let Some(value) = arguments.next() else {
                    eprintln!("vibemux_probe: --project-root requires a directory argument");
                    eprintln!("{USAGE}");
                    exit(4);
                };
                project_root = PathBuf::from(value);
            }
            _ => {
                eprintln!("vibemux_probe: unknown or malformed argument");
                eprintln!("{USAGE}");
                exit(4);
            }
        }
    }
    let config = ProbeConfig::from_environment();
    let report = run_probe(&config).await;
    let encoded = match report_json(&report) {
        Ok(encoded) => encoded,
        Err(error) => {
            eprintln!("probe failed: {error}");
            exit(4);
        }
    };
    if write_cache {
        let path = cache::default_cache_path(&project_root);
        if let Err(error) = cache::write_cache(&report, &path) {
            eprintln!("probe cache write failed: {error}");
            exit(4);
        }
    }
    println!("{encoded}");
}
