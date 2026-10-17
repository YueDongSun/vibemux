use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, time::Duration};

use vibemux_harness_native::{Error, contract::Harness};

/// The one error code for a malformed command line.
const USAGE: &str = "harness_plugin_usage";

fn main() {
    let result =
        parse_arguments(std::env::args_os().skip(1)).and_then(|(config, harness, digest)| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|source| Error::with_source("harness_runtime_failed", source))?;
            let result = runtime.block_on(vibemux_harness_native::plugin::serve(
                &config, harness, &digest,
            ));
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

/// Parse `--config <path> --harness <name> --config_digest <sha256>`, each
/// option exactly once and in any order. The arguments stay `OsString`s, so
/// the config path may be any operating system string, while an option name,
/// harness name, or digest that is not valid Unicode is a usage error rather
/// than a panic.
fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<(PathBuf, Harness, String), Error> {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    if arguments.len() != 6 {
        return Err(Error::new(USAGE));
    }
    let mut options = BTreeMap::new();
    for pair in arguments.chunks_exact(2) {
        let name = pair[0].to_str().ok_or_else(|| Error::new(USAGE))?;
        if !matches!(name, "--config" | "--harness" | "--config_digest")
            || options.insert(name, &pair[1]).is_some()
        {
            return Err(Error::new(USAGE));
        }
    }
    let config = options.get("--config").ok_or_else(|| Error::new(USAGE))?;
    let harness = options
        .get("--harness")
        .and_then(|value| value.to_str())
        .ok_or_else(|| Error::new(USAGE))?;
    let harness = serde_json::from_value(serde_json::Value::String(harness.to_string()))
        .map_err(|source| Error::with_source(USAGE, source))?;
    let digest = options
        .get("--config_digest")
        .and_then(|value| value.to_str())
        .ok_or_else(|| Error::new(USAGE))?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::new(USAGE));
    }
    Ok((PathBuf::from(config), harness, digest.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789abcdef";

    #[cfg(windows)]
    fn non_unicode() -> OsString {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[0x61, 0xD800])
    }

    #[cfg(unix)]
    fn non_unicode() -> OsString {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![b'a', 0xff])
    }

    fn valid() -> Vec<OsString> {
        [
            "--config",
            "routes.json",
            "--harness",
            "codex",
            "--config_digest",
            DIGEST,
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    fn with(index: usize, value: OsString) -> Vec<OsString> {
        let mut arguments = valid();
        arguments[index] = value;
        arguments
    }

    fn error_code(arguments: Vec<OsString>) -> Option<&'static str> {
        parse_arguments(arguments).err().map(|error| error.code())
    }

    #[test]
    fn options_parse_in_any_order() {
        let (config, harness, digest) = parse_arguments(valid()).expect("valid");
        assert_eq!(config, PathBuf::from("routes.json"));
        assert_eq!(harness, Harness::Codex);
        assert_eq!(digest, DIGEST);

        let mut reordered = valid();
        reordered.rotate_left(2);
        let (config, harness, _) = parse_arguments(reordered).expect("reordered");
        assert_eq!(
            (config, harness),
            (PathBuf::from("routes.json"), Harness::Codex)
        );
    }

    #[test]
    fn the_config_path_may_be_any_operating_system_string() {
        let (config, _, _) = parse_arguments(with(1, non_unicode())).expect("path");
        assert_eq!(config.as_os_str(), non_unicode());
    }

    #[test]
    fn a_non_unicode_name_harness_or_digest_is_a_usage_error() {
        for index in [0, 2, 3, 4, 5] {
            assert_eq!(
                error_code(with(index, non_unicode())),
                Some(USAGE),
                "{index}"
            );
        }
    }

    #[test]
    fn a_malformed_command_line_is_a_usage_error() {
        let duplicate = with(2, "--config".into());
        let unknown = with(4, "--digest".into());
        let short_digest = with(5, DIGEST[1..].into());
        let non_hex_digest = with(5, format!("{}g", &DIGEST[1..]).into());
        let unknown_harness = with(3, "gemini".into());
        let mut missing = valid();
        missing.truncate(4);
        for arguments in [
            Vec::new(),
            missing,
            duplicate,
            unknown,
            short_digest,
            non_hex_digest,
            unknown_harness,
        ] {
            assert_eq!(error_code(arguments.clone()), Some(USAGE), "{arguments:?}");
        }
    }
}
