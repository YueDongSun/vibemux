use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use vibemux_harness_native::{Error, contract::Harness};

fn main() {
    let result = arguments().and_then(|(config, harness, digest)| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|source| Error::with_source("harness_runtime_failed", source))?;
        let result = runtime.block_on(vibemux_harness_native::plugin::serve(&config, harness, &digest));
        // Tokio stdin uses a blocking OS read. Native children have already been
        // reaped, and this bound prevents a pending stdin read delaying shutdown.
        runtime.shutdown_timeout(Duration::from_millis(100));
        result
    });
    if let Err(error) = result {
        eprintln!("{}", error.code());
        std::process::exit(1);
    }
}

fn arguments() -> Result<(PathBuf, Harness, String), Error> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() != 6 {
        return Err(Error::new("harness_plugin_usage"));
    }
    let mut options = BTreeMap::new();
    for pair in arguments.chunks_exact(2) {
        if !matches!(
            pair[0].as_str(),
            "--config" | "--harness" | "--config_digest"
        ) || options.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            return Err(Error::new("harness_plugin_usage"));
        }
    }
    let config = options
        .get("--config")
        .ok_or_else(|| Error::new("harness_plugin_usage"))?;
    let harness = options
        .get("--harness")
        .ok_or_else(|| Error::new("harness_plugin_usage"))?;
    let harness = serde_json::from_value(serde_json::Value::String(harness.clone()))
        .map_err(|source| Error::with_source("harness_plugin_usage", source))?;
    let digest = options
        .get("--config_digest")
        .ok_or_else(|| Error::new("harness_plugin_usage"))?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::new("harness_plugin_usage"));
    }
    Ok((PathBuf::from(config), harness, digest.clone()))
}
