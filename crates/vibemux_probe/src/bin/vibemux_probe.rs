use vibemux_probe::{ProbeConfig, report_json, run_probe};

#[tokio::main]
async fn main() {
    let config = ProbeConfig::from_environment();
    let report = run_probe(&config).await;
    match report_json(&report) {
        Ok(encoded) => println!("{encoded}"),
        Err(error) => {
            eprintln!("probe failed: {error}");
            std::process::exit(4);
        }
    }
}
