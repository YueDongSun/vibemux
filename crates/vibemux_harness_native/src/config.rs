use std::{
    collections::BTreeSet,
    fmt,
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use vibemux_harness::dispatch::route_config::MAX_MODEL_BYTES;

use crate::{
    Error,
    contract::{Harness, Protocol, SCHEMA_VERSION},
};

pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
pub const MAX_ROUTES: usize = 16;
pub const MAX_ENVIRONMENT_NAMES: usize = 24;
pub const MAX_ENVIRONMENT_NAME_BYTES: usize = 128;
pub const MAX_ARGUMENTS: usize = 24;
pub const MAX_ARGUMENT_BYTES: usize = 4096;
pub const MAX_TOTAL_ARGUMENT_BYTES: usize = 12 * 1024;
pub const DEFAULT_FRAME_BYTES: usize = 128 * 1024;
pub const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
/// Approval, sandbox, sharing, and debug-log flags an operator prefix may not
/// carry, alone or as `flag=value`. This guards against mistakes; it is not a
/// sandbox, and the shared posture flags still follow the prefix.
pub const UNSAFE_ARGUMENTS: &[&str] = &[
    "--dangerously-bypass-approvals-and-sandbox",
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "--always-approve",
    "--yolo",
    "--auto",
    "--allow-all",
    "--allow-all-tools",
    "--allow-all-paths",
    "--allow-all-urls",
    "--share",
    "--share-gist",
    "--remote",
    "--remote-export",
    "--debug-file",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub frame_bytes: usize,
    pub capture_bytes: usize,
    pub record_count: usize,
    pub queue_capacity: usize,
    pub maximum_in_flight: usize,
    pub request_history: usize,
    pub timeout_ms: u64,
    pub shutdown_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            frame_bytes: DEFAULT_FRAME_BYTES,
            capture_bytes: MAX_CAPTURE_BYTES,
            record_count: 16_384,
            queue_capacity: 16,
            maximum_in_flight: 4,
            request_history: 4096,
            timeout_ms: 60_000,
            shutdown_ms: 2_000,
        }
    }
}

impl Limits {
    pub fn validate(&self) -> Result<(), Error> {
        if !(64..=DEFAULT_FRAME_BYTES).contains(&self.frame_bytes)
            || !(self.frame_bytes..=MAX_CAPTURE_BYTES).contains(&self.capture_bytes)
            || !(1..=65_536).contains(&self.record_count)
            || !(1..=128).contains(&self.queue_capacity)
            || !(1..=16).contains(&self.maximum_in_flight)
            || !(1..=65_536).contains(&self.request_history)
            || !(100..=600_000).contains(&self.timeout_ms)
            || !(100..=10_000).contains(&self.shutdown_ms)
        {
            return Err(Error::new("harness_invalid_limits"));
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub harness: Harness,
    pub protocol: Protocol,
    pub executable: PathBuf,
    #[serde(default)]
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    #[serde(default)]
    pub environment_names: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allow_execution: bool,
    #[serde(default)]
    pub model: Option<String>,
}

impl fmt::Debug for Route {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Route")
            .field("harness", &self.harness)
            .field("protocol", &self.protocol)
            .field("enabled", &self.enabled)
            .field("allow_execution", &self.allow_execution)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default)]
    pub limits: Limits,
    pub harnesses: Vec<Route>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, Error> {
        Self::snapshot(path).map(|(config, _digest)| config)
    }

    pub(crate) fn snapshot(path: &Path) -> Result<(Self, String), Error> {
        let path = path
            .canonicalize()
            .map_err(|source| Error::with_source("harness_config_unavailable", source))?;
        let mut file = std::fs::File::open(&path)
            .map_err(|source| Error::with_source("harness_config_unavailable", source))?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_CONFIG_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| Error::with_source("harness_config_unavailable", source))?;
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err(Error::new("harness_config_too_large"));
        }
        let mut digest = String::with_capacity(64);
        for byte in Sha256::digest(&bytes) {
            write!(&mut digest, "{byte:02x}").expect("writing to a String is infallible");
        }
        let mut config: Self = serde_json::from_slice(&bytes)
            .map_err(|source| Error::with_source("harness_invalid_config", source))?;
        config.validate()?;
        let parent = path
            .parent()
            .ok_or_else(|| Error::new("harness_config_unavailable"))?;
        for route in &mut config.harnesses {
            if route.executable.is_relative() {
                route.executable = parent.join(&route.executable);
            }
            if route.working_directory.is_relative() {
                route.working_directory = parent.join(&route.working_directory);
            }
        }
        Ok((config, digest))
    }

    pub fn validate(&self) -> Result<(), Error> {
        self.limits.validate()?;
        if self.schema_version != SCHEMA_VERSION
            || self.harnesses.is_empty()
            || self.harnesses.len() > MAX_ROUTES
        {
            return Err(Error::new("harness_invalid_config"));
        }
        let mut registered = BTreeSet::new();
        for route in &self.harnesses {
            if !registered.insert(route.harness)
                || !route.protocol.accepts(route.harness)
                || route.executable.as_os_str().is_empty()
                || route.working_directory.as_os_str().is_empty()
                || route.arguments.len() > MAX_ARGUMENTS
                || route.arguments.iter().any(|argument| {
                    argument.len() > MAX_ARGUMENT_BYTES || argument.contains('\0')
                })
                || route.arguments.iter().map(String::len).sum::<usize>()
                    > MAX_TOTAL_ARGUMENT_BYTES
                || route.environment_names.len() > MAX_ENVIRONMENT_NAMES
                // The model follows `--model` in argv, so a leading dash could
                // be read as a flag; the daemon's route label check refuses it too.
                || route.model.as_ref().is_some_and(|model| {
                    model.is_empty()
                        || model.len() > MAX_MODEL_BYTES
                        || model.starts_with('-')
                        || model.contains('\0')
                })
            {
                return Err(Error::new("harness_invalid_route"));
            }
            let mut names = BTreeSet::new();
            for name in &route.environment_names {
                if name.is_empty()
                    || name.len() > MAX_ENVIRONMENT_NAME_BYTES
                    || !name.bytes().all(|byte| {
                        byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'
                    })
                    || !names.insert(name.to_ascii_uppercase())
                    || name.eq_ignore_ascii_case("VIBEMUX_PLUGIN_SESSION_ID")
                {
                    return Err(Error::new("harness_invalid_environment"));
                }
            }
            if route.arguments.iter().any(|argument| {
                UNSAFE_ARGUMENTS
                    .iter()
                    .any(|flag| argument == flag || argument.starts_with(&format!("{flag}=")))
            }) {
                return Err(Error::new("harness_unsafe_arguments"));
            }
        }
        Ok(())
    }

    pub fn route(&self, harness: Harness) -> Result<&Route, Error> {
        self.harnesses
            .iter()
            .find(|route| route.harness == harness && route.enabled)
            .ok_or_else(|| Error::new("harness_route_unavailable"))
    }

    /// Both capability flags come from the shared protocol engine: a route
    /// executes when an engine session drives it, and probes when that
    /// engine has an initialize-only handshake.
    pub fn catalog(&self) -> serde_json::Value {
        serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "harnesses": self.harnesses.iter().map(|route| {
                let shared = route.protocol.native_protocol();
                serde_json::json!({
                    "harness": route.harness,
                    "protocol": route.protocol,
                    "enabled": route.enabled,
                    "allow_execution": route.allow_execution,
                    "structured_probe": shared.is_some_and(|protocol| protocol.supports_probe()),
                    "native_execution": shared.is_some()
                })
            }).collect::<Vec<_>>()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        Config, Limits, MAX_ARGUMENT_BYTES, MAX_ARGUMENTS, MAX_ENVIRONMENT_NAME_BYTES,
        MAX_ENVIRONMENT_NAMES, MAX_MODEL_BYTES, MAX_ROUTES, MAX_TOTAL_ARGUMENT_BYTES, Route,
        UNSAFE_ARGUMENTS,
    };
    use crate::contract::{Harness, Protocol, SCHEMA_VERSION};

    type RouteEdit = fn(&mut Route);

    fn config(edit: impl FnOnce(&mut Route)) -> Config {
        let mut route = Route {
            harness: Harness::Codex,
            protocol: Protocol::Appserver,
            executable: PathBuf::from("codex.exe"),
            arguments: vec![],
            working_directory: PathBuf::from("workspace"),
            environment_names: vec![],
            enabled: true,
            allow_execution: false,
            model: None,
        };
        edit(&mut route);
        Config {
            schema_version: SCHEMA_VERSION,
            limits: Limits::default(),
            harnesses: vec![route],
        }
    }

    fn outcome(config: &Config) -> &'static str {
        config
            .validate()
            .map_or_else(|error| error.code(), |()| "ok")
    }

    #[test]
    fn config_shapes_outside_the_bounds_are_refused() {
        let mut other_schema = config(|_| {});
        other_schema.schema_version = SCHEMA_VERSION + 1;
        assert_eq!(outcome(&other_schema), "harness_invalid_config");
        let mut no_routes = config(|_| {});
        no_routes.harnesses.clear();
        assert_eq!(outcome(&no_routes), "harness_invalid_config");
        let mut too_many_routes = config(|_| {});
        too_many_routes.harnesses = vec![too_many_routes.harnesses[0].clone(); MAX_ROUTES + 1];
        assert_eq!(outcome(&too_many_routes), "harness_invalid_config");
        let mut no_timeout = config(|_| {});
        no_timeout.limits.timeout_ms = 0;
        assert_eq!(outcome(&no_timeout), "harness_invalid_limits");
        // One route per harness: a second Codex route is refused even
        // though each route is valid alone.
        let mut duplicate = config(|_| {});
        duplicate.harnesses.push(duplicate.harnesses[0].clone());
        assert_eq!(outcome(&duplicate), "harness_invalid_route");
        let mut distinct = config(|_| {});
        distinct.harnesses.push(Route {
            harness: Harness::Claude,
            protocol: Protocol::Claude,
            ..distinct.harnesses[0].clone()
        });
        assert_eq!(outcome(&distinct), "ok");
    }

    #[test]
    fn route_shapes_outside_the_bounds_are_refused() {
        let per_argument = MAX_TOTAL_ARGUMENT_BYTES / MAX_ARGUMENTS;
        let at_bounds = config(|route| {
            route.arguments = vec!["a".repeat(per_argument); MAX_ARGUMENTS];
        });
        assert_eq!(outcome(&at_bounds), "ok");
        let longest = config(|route| route.arguments = vec!["a".repeat(MAX_ARGUMENT_BYTES)]);
        assert_eq!(outcome(&longest), "ok");
        let refusals: [(&str, RouteEdit); 8] = [
            ("protocol of another harness", |route| {
                route.protocol = Protocol::Claude
            }),
            ("ACP for Codex", |route| route.protocol = Protocol::Acp),
            ("empty executable", |route| {
                route.executable = PathBuf::new()
            }),
            ("empty working directory", |route| {
                route.working_directory = PathBuf::new()
            }),
            ("too many arguments", |route| {
                route.arguments = vec!["a".to_string(); MAX_ARGUMENTS + 1]
            }),
            ("argument too long", |route| {
                route.arguments = vec!["a".repeat(MAX_ARGUMENT_BYTES + 1)]
            }),
            ("argument with NUL", |route| {
                route.arguments = vec!["a\0".to_string()]
            }),
            ("arguments too long together", |route| {
                let full = MAX_TOTAL_ARGUMENT_BYTES / MAX_ARGUMENT_BYTES;
                route.arguments = vec!["a".repeat(MAX_ARGUMENT_BYTES); full];
                route.arguments.push("a".to_string());
            }),
        ];
        for (label, edit) in refusals {
            assert_eq!(outcome(&config(edit)), "harness_invalid_route", "{label}");
        }
    }

    #[test]
    fn approval_bypass_flags_are_refused_alone_and_with_a_value() {
        for flag in [
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-skip-permissions",
            "--yolo",
            "--allow-all-tools",
        ] {
            assert!(UNSAFE_ARGUMENTS.contains(&flag), "{flag}");
        }
        for flag in UNSAFE_ARGUMENTS {
            for argument in [(*flag).to_string(), format!("{flag}=true")] {
                let refused = config(|route| {
                    route.arguments = vec!["launcher.js".to_string(), argument.clone()];
                });
                assert_eq!(outcome(&refused), "harness_unsafe_arguments", "{argument}");
            }
        }
        let launcher = config(|route| {
            route.arguments = vec![
                "--max-old-space-size=4096".to_string(),
                "launcher.js".to_string(),
            ];
        });
        assert_eq!(outcome(&launcher), "ok");
    }

    #[test]
    fn a_model_name_can_never_be_read_as_a_flag() {
        for model in [
            "sonnet",
            "gpt-5.1-codex",
            "claude-sonnet-4-5",
            "openai/gpt-5",
        ] {
            let accepted = config(|route| route.model = Some(model.to_string()));
            assert_eq!(outcome(&accepted), "ok", "{model}");
        }
        for model in [
            String::new(),
            "-m".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "x".repeat(MAX_MODEL_BYTES + 1),
            "model\0".to_string(),
        ] {
            let refused = config(|route| route.model = Some(model.clone()));
            assert_eq!(outcome(&refused), "harness_invalid_route", "{model:?}");
        }
    }

    #[test]
    fn environment_names_must_be_distinct_uppercase_and_unreserved() {
        let accepted = config(|route| {
            route.environment_names = vec![
                "OPENAI_API_KEY".to_string(),
                "HTTPS_PROXY".to_string(),
                "X".repeat(MAX_ENVIRONMENT_NAME_BYTES),
            ];
        });
        assert_eq!(outcome(&accepted), "ok");
        for names in [
            vec![String::new()],
            vec!["path".to_string()],
            vec!["Path".to_string()],
            vec!["MY-NAME".to_string()],
            vec!["NAME=VALUE".to_string()],
            vec!["X".repeat(MAX_ENVIRONMENT_NAME_BYTES + 1)],
            vec!["TOKEN".to_string(), "TOKEN".to_string()],
            // The plugin's session identifier never comes from a route.
            vec!["VIBEMUX_PLUGIN_SESSION_ID".to_string()],
        ] {
            let refused = config(|route| route.environment_names = names.clone());
            assert_eq!(
                outcome(&refused),
                "harness_invalid_environment",
                "{names:?}"
            );
        }
        let at_limit: Vec<String> = (0..MAX_ENVIRONMENT_NAMES)
            .map(|index| format!("NAME_{index}"))
            .collect();
        assert_eq!(
            outcome(&config(|route| route.environment_names = at_limit.clone())),
            "ok"
        );
        let mut over_limit = at_limit;
        over_limit.push("NAME_EXTRA".to_string());
        assert_eq!(
            outcome(&config(|route| route.environment_names = over_limit)),
            "harness_invalid_route"
        );
    }
}
