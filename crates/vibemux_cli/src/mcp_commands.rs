//! `vibemuxctl mcp`: register the read-only MCP host with a desktop client.
//!
//! These verbs touch the client's JSON configuration only. They never start the
//! daemon, the MCP host, or a model request, and they never read prompt text,
//! environment values, or credentials. Configuration discovery and binary
//! discovery take their inputs from [`Environment`] and [`locate_binary`]'s
//! parameters, so every merge rule is exercisable with temporary directories
//! and injected paths.
//!
//! The client configuration is parsed into [`ConfigValue`], whose objects are
//! `IndexMap`s, so unrelated top-level keys and unrelated `mcpServers` entries
//! keep their original position across an install.

use std::{
    env, error,
    ffi::{OsStr, OsString},
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value, json};
use time::OffsetDateTime;
use vibemuxd::process::DaemonPaths;

use crate::failure::Failure;

/// The single client key this module creates or replaces under `mcpServers`.
pub const SERVER_NAME: &str = "vibemux";
/// The Claude Desktop configuration file name.
pub const CONFIG_FILE_NAME: &str = "claude_desktop_config.json";
/// Bound on the configuration file read. A desktop client configuration is a
/// few KiB; this refuses a runaway or hostile file without rejecting a real one.
pub const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
/// The one descriptor argument the MCP host accepts.
const DESCRIPTOR_ARGUMENT: &str = "--descriptor";
const CLIENT_CLAUDE_DESKTOP: &str = "claude-desktop";
const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Platform {
    Macos,
    Windows,
    #[default]
    Other,
}

impl Platform {
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// Injected discovery inputs; tests supply their own home and application data.
#[derive(Clone, Debug, Default)]
pub struct Environment {
    pub platform: Platform,
    pub home: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
}

impl Environment {
    /// The process environment. Only the two variables the client layout needs
    /// are read; no value is ever reported.
    #[must_use]
    pub fn current() -> Self {
        Self {
            platform: Platform::current(),
            home: env::var_os("HOME").map(PathBuf::from),
            app_data: env::var_os("APPDATA").map(PathBuf::from),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigSource {
    Override,
    Default,
}

impl ConfigSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Default => "default",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Client {
    ClaudeDesktop,
}

impl Client {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        CLIENT_CLAUDE_DESKTOP
    }

    pub fn parse(value: &str) -> Result<Self, ParseError> {
        if value == CLIENT_CLAUDE_DESKTOP {
            Ok(Self::ClaudeDesktop)
        } else {
            Err(ParseError::UnsupportedClient)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpVerb {
    Install,
    Status,
    Doctor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpOptions {
    pub verb: McpVerb,
    pub project_root: Option<PathBuf>,
    pub client: Client,
    pub binary: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub bundle: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    InvalidArguments,
    UnsupportedClient,
}

impl ParseError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidArguments => "cli_invalid_arguments",
            Self::UnsupportedClient => "mcp_client_unsupported",
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl error::Error for ParseError {}

/// Parses `<install|status|doctor> [flags...]` without any I/O.
pub fn parse_arguments(arguments: &[OsString]) -> Result<McpOptions, ParseError> {
    let (verb, tail) = arguments
        .split_first()
        .ok_or(ParseError::InvalidArguments)?;
    let verb = match verb.to_str().ok_or(ParseError::InvalidArguments)? {
        "install" => McpVerb::Install,
        "status" => McpVerb::Status,
        "doctor" => McpVerb::Doctor,
        _ => return Err(ParseError::InvalidArguments),
    };
    let mut project_root = None;
    let mut client = None;
    let mut binary = None;
    let mut config = None;
    let mut bundle = None;
    let mut index = 0;
    while index < tail.len() {
        let flag = tail[index].to_str().ok_or(ParseError::InvalidArguments)?;
        let value = tail
            .get(index + 1)
            .filter(|value| {
                !value.is_empty() && !value.to_str().is_some_and(|text| text.starts_with("--"))
            })
            .ok_or(ParseError::InvalidArguments)?;
        match flag {
            "--project-root" => set_once(&mut project_root, PathBuf::from(value))?,
            "--client" => {
                let value = value.to_str().ok_or(ParseError::InvalidArguments)?;
                set_once(&mut client, Client::parse(value)?)?;
            }
            "--binary" => set_once(&mut binary, PathBuf::from(value))?,
            "--config" => set_once(&mut config, PathBuf::from(value))?,
            "--bundle" => {
                if verb != McpVerb::Doctor || bundle.is_some() {
                    return Err(ParseError::InvalidArguments);
                }
                bundle = Some(PathBuf::from(value));
            }
            _ => return Err(ParseError::InvalidArguments),
        }
        index += 2;
    }
    Ok(McpOptions {
        verb,
        project_root,
        client: client.unwrap_or(Client::ClaudeDesktop),
        binary,
        config,
        bundle,
    })
}

fn set_once<T>(slot: &mut Option<T>, value: T) -> Result<(), ParseError> {
    if slot.is_some() {
        return Err(ParseError::InvalidArguments);
    }
    *slot = Some(value);
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpError {
    PlatformUnsupported,
    ConfigUnresolved,
    ConfigUnreadable,
    ConfigTooLarge,
    ConfigMalformed,
    ConfigWriteFailed,
    BinaryNotFound,
    BinaryNotExecutable,
    EntryMissing,
    EntryMismatch,
    StaleEntryPath,
    BundleWriteFailed,
}

impl McpError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::PlatformUnsupported => "mcp_config_platform_unsupported",
            Self::ConfigUnresolved => "mcp_config_unresolved",
            Self::ConfigUnreadable => "mcp_config_unreadable",
            Self::ConfigTooLarge => "mcp_config_too_large",
            Self::ConfigMalformed => "mcp_config_malformed",
            Self::ConfigWriteFailed => "mcp_config_write_failed",
            Self::BinaryNotFound => "mcp_binary_not_found",
            Self::BinaryNotExecutable => "mcp_binary_not_executable",
            Self::EntryMissing => "mcp_entry_missing",
            Self::EntryMismatch => "mcp_entry_mismatch",
            Self::StaleEntryPath => "mcp_stale_entry_path",
            Self::BundleWriteFailed => "mcp_bundle_write_failed",
        }
    }

    /// One fixed remediation per failure. Messages carry no path, environment
    /// value, or prompt text.
    #[must_use]
    pub const fn remediation(self) -> &'static str {
        match self {
            Self::PlatformUnsupported => {
                "Pass --config with the client's configuration path; automatic discovery supports macOS and Windows."
            }
            Self::ConfigUnresolved => {
                "Pass --config with the client's configuration path or set HOME/APPDATA."
            }
            Self::ConfigUnreadable => {
                "Check that the configuration file exists and is a readable regular file."
            }
            Self::ConfigTooLarge => {
                "Reduce the configuration file below 4 MiB or pass a different --config."
            }
            Self::ConfigMalformed => {
                "Fix the configuration JSON by hand; VibeMux refuses to rewrite a malformed file."
            }
            Self::ConfigWriteFailed => {
                "Check that the configuration directory is writable and retry."
            }
            Self::BinaryNotFound => "Build vibemux_mcp or pass --binary with an absolute path.",
            Self::BinaryNotExecutable => {
                "Make the MCP binary executable or pass --binary with another path."
            }
            Self::EntryMissing => {
                "Run `vibemuxctl mcp install` to register the VibeMux MCP server."
            }
            Self::EntryMismatch => "Run `vibemuxctl mcp install` to refresh the VibeMux MCP entry.",
            Self::StaleEntryPath => {
                "Run `vibemuxctl mcp install` to refresh the VibeMux entry, or remove the stale server entry."
            }
            Self::BundleWriteFailed => "Pass --bundle with a writable directory.",
        }
    }
}

impl fmt::Display for McpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl error::Error for McpError {}

/// An ordered JSON value. `serde_json::Value` sorts object keys because its map
/// is a `BTreeMap`; this representation keeps client configuration entries in
/// document order.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ConfigValue {
    Null(()),
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<ConfigValue>),
    Object(ConfigObject),
}

pub type ConfigObject = IndexMap<String, ConfigValue>;

impl ConfigValue {
    fn object(&self) -> Option<&ConfigObject> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }

    fn object_mut(&mut self) -> Option<&mut ConfigObject> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    fn strings(&self) -> Option<Vec<&str>> {
        match self {
            Self::Array(values) => values.iter().map(Self::text).collect(),
            _ => None,
        }
    }
}

/// The `mcpServers.vibemux` entry this module owns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerEntry {
    pub command: String,
    pub args: Vec<String>,
}

impl ServerEntry {
    fn to_config_value(&self) -> ConfigValue {
        let mut object = ConfigObject::new();
        object.insert(
            "command".to_string(),
            ConfigValue::String(self.command.clone()),
        );
        object.insert(
            "args".to_string(),
            ConfigValue::Array(self.args.iter().cloned().map(ConfigValue::String).collect()),
        );
        ConfigValue::Object(object)
    }

    fn matches(&self, value: &ConfigValue) -> bool {
        let Some(object) = value.object() else {
            return false;
        };
        object.len() == 2
            && object.get("command").and_then(ConfigValue::text) == Some(self.command.as_str())
            && object
                .get("args")
                .and_then(ConfigValue::strings)
                .is_some_and(|args| {
                    args.len() == self.args.len()
                        && args
                            .iter()
                            .zip(&self.args)
                            .all(|(actual, expected)| actual == expected)
                })
    }
}

/// `vibemux_mcp`'s file name for this platform.
#[must_use]
pub const fn binary_name() -> &'static str {
    if cfg!(windows) {
        "vibemux_mcp.exe"
    } else {
        "vibemux_mcp"
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinarySource {
    Explicit,
    NextToExecutable,
    PathEntry,
    NotFound,
}

impl BinarySource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::NextToExecutable => "next_to_executable",
            Self::PathEntry => "path",
            Self::NotFound => "not_found",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocatedBinary {
    pub path: PathBuf,
    pub source: BinarySource,
    pub exists: bool,
}

/// Resolves `vibemux_mcp`: the explicit path, then the directory of the running
/// executable, then `PATH`. A failed lookup keeps the sibling candidate (or the
/// bare name) so status and doctor can report the intended path.
#[must_use]
pub fn locate_binary(
    explicit: Option<&Path>,
    current_exe: Option<&Path>,
    path_var: Option<&OsStr>,
) -> LocatedBinary {
    if let Some(explicit) = explicit {
        let path = absolute_path(explicit);
        return LocatedBinary {
            exists: path.is_file(),
            path,
            source: BinarySource::Explicit,
        };
    }
    let sibling = current_exe
        .and_then(Path::parent)
        .map(|directory| directory.join(binary_name()));
    if let Some(candidate) = sibling.as_ref()
        && candidate.is_file()
    {
        return LocatedBinary {
            path: absolute_path(candidate),
            source: BinarySource::NextToExecutable,
            exists: true,
        };
    }
    if let Some(path_var) = path_var {
        for directory in env::split_paths(path_var) {
            if directory.as_os_str().is_empty() {
                continue;
            }
            let candidate = directory.join(binary_name());
            if candidate.is_file() {
                return LocatedBinary {
                    path: absolute_path(&candidate),
                    source: BinarySource::PathEntry,
                    exists: true,
                };
            }
        }
    }
    LocatedBinary {
        path: absolute_path(&sibling.unwrap_or_else(|| PathBuf::from(binary_name()))),
        source: BinarySource::NotFound,
        exists: false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DescriptorSource {
    Runtime,
    Placeholder,
}

impl DescriptorSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Placeholder => "placeholder",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DescriptorLocation {
    pub path: PathBuf,
    pub source: DescriptorSource,
}

/// Derives the daemon descriptor path for a project through `DaemonPaths` (the
/// single owner of that layout). When the root cannot be canonicalized the
/// project-local legacy path is used as an explicit placeholder.
#[must_use]
pub fn descriptor_location(project_root: &Path) -> DescriptorLocation {
    match DaemonPaths::from_project_root(project_root) {
        Ok(paths) => DescriptorLocation {
            path: paths.descriptor_path().to_path_buf(),
            source: DescriptorSource::Runtime,
        },
        Err(_) => DescriptorLocation {
            path: absolute_path(project_root)
                .join(".vibemux")
                .join("control.json"),
            source: DescriptorSource::Placeholder,
        },
    }
}

/// The macOS and Windows Claude Desktop configuration locations.
pub fn default_config_path(environment: &Environment) -> Result<PathBuf, McpError> {
    match environment.platform {
        Platform::Macos => environment
            .home
            .as_ref()
            .map(|home| {
                home.join("Library")
                    .join("Application Support")
                    .join("Claude")
                    .join(CONFIG_FILE_NAME)
            })
            .ok_or(McpError::ConfigUnresolved),
        Platform::Windows => environment
            .app_data
            .as_ref()
            .map(|app_data| app_data.join("Claude").join(CONFIG_FILE_NAME))
            .ok_or(McpError::ConfigUnresolved),
        Platform::Other => Err(McpError::PlatformUnsupported),
    }
}

pub fn resolve_config_path(
    environment: &Environment,
    explicit: Option<&Path>,
) -> Result<(PathBuf, ConfigSource), McpError> {
    match explicit {
        Some(path) => Ok((absolute_path(path), ConfigSource::Override)),
        None => Ok((default_config_path(environment)?, ConfigSource::Default)),
    }
}

#[derive(Clone, Debug)]
pub struct InstallRequest {
    pub client: Client,
    pub config_path: PathBuf,
    pub config_source: ConfigSource,
    pub binary_path: PathBuf,
    pub descriptor_path: PathBuf,
}

impl InstallRequest {
    fn server_entry(&self) -> ServerEntry {
        ServerEntry {
            command: absolute_path(&self.binary_path)
                .to_string_lossy()
                .into_owned(),
            args: vec![
                DESCRIPTOR_ARGUMENT.to_string(),
                self.descriptor_path.to_string_lossy().into_owned(),
            ],
        }
    }

    fn report(&self, changed: bool, created: bool, backup: Option<PathBuf>) -> InstallReport {
        InstallReport {
            schema_version: REPORT_SCHEMA_VERSION,
            ok: true,
            client: self.client.as_str(),
            config_path: self.config_path.to_string_lossy().into_owned(),
            config_source: self.config_source.as_str(),
            binary: absolute_path(&self.binary_path)
                .to_string_lossy()
                .into_owned(),
            descriptor: self.descriptor_path.to_string_lossy().into_owned(),
            changed,
            created,
            backup: backup.map(|path| path.to_string_lossy().into_owned()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct InstallReport {
    pub schema_version: u32,
    pub ok: bool,
    pub client: &'static str,
    pub config_path: String,
    pub config_source: &'static str,
    pub binary: String,
    pub descriptor: String,
    pub changed: bool,
    pub created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
}

/// Merges `mcpServers.vibemux` into the client configuration.
///
/// A missing file is created; an existing file is backed up (one timestamped
/// `.bak` containing the original bytes) before the atomic replacement. An
/// entry that already matches is a no-op. Malformed JSON is refused before any
/// write, so the original file is untouched.
pub fn install(request: &InstallRequest, now: OffsetDateTime) -> Result<InstallReport, McpError> {
    let original = load_config(&request.config_path)?;
    let (mut document, created) = match &original {
        Some(bytes) => (parse_config(bytes)?, false),
        None => (ConfigValue::Object(ConfigObject::new()), true),
    };
    // Writing through a symlinked configuration (a common dotfile-manager
    // layout) preserves the link instead of replacing it with a regular file.
    let write_path = write_target(&request.config_path)?;
    let desired = request.server_entry();
    let servers = servers_mut(&mut document)?;
    if servers
        .get(SERVER_NAME)
        .is_some_and(|entry| desired.matches(entry))
    {
        return Ok(request.report(false, created, None));
    }
    servers.insert(SERVER_NAME.to_string(), desired.to_config_value());
    let encoded = encode_config(&document)?;
    let backup = match &original {
        Some(bytes) => Some(write_backup(&write_path, now, bytes)?),
        None => None,
    };
    atomic_write(&write_path, &encoded).map_err(|_| McpError::ConfigWriteFailed)?;
    Ok(request.report(true, created, backup))
}

#[derive(Clone, Debug)]
pub struct StatusRequest {
    pub client: Client,
    pub config_path: PathBuf,
    pub config_source: ConfigSource,
    pub binary: LocatedBinary,
    pub descriptor: DescriptorLocation,
}

#[derive(Clone, Debug, Serialize)]
pub struct StatusReport {
    pub schema_version: u32,
    pub ok: bool,
    pub client: &'static str,
    pub config_path: String,
    pub config_source: &'static str,
    pub config_exists: bool,
    pub config_valid: bool,
    pub entry_present: bool,
    pub entry_matches: bool,
    pub status: &'static str,
    pub binary: String,
    pub binary_source: &'static str,
    pub binary_exists: bool,
    pub descriptor: String,
    pub descriptor_source: &'static str,
}

impl StatusRequest {
    fn server_entry(&self) -> ServerEntry {
        ServerEntry {
            command: absolute_path(&self.binary.path)
                .to_string_lossy()
                .into_owned(),
            args: vec![
                DESCRIPTOR_ARGUMENT.to_string(),
                self.descriptor.path.to_string_lossy().into_owned(),
            ],
        }
    }

    fn report(
        &self,
        config_exists: bool,
        config_valid: bool,
        entry_present: bool,
        entry_matches: bool,
        status: &'static str,
    ) -> StatusReport {
        StatusReport {
            schema_version: REPORT_SCHEMA_VERSION,
            ok: entry_matches,
            client: self.client.as_str(),
            config_path: self.config_path.to_string_lossy().into_owned(),
            config_source: self.config_source.as_str(),
            config_exists,
            config_valid,
            entry_present,
            entry_matches,
            status,
            binary: self.binary.path.to_string_lossy().into_owned(),
            binary_source: self.binary.source.as_str(),
            binary_exists: self.binary.exists,
            descriptor: self.descriptor.path.to_string_lossy().into_owned(),
            descriptor_source: self.descriptor.source.as_str(),
        }
    }
}

/// Reports whether the client configuration matches the desired entry without
/// writing anything.
pub fn status(request: &StatusRequest) -> Result<StatusReport, McpError> {
    match load_config(&request.config_path)? {
        None => Ok(request.report(false, true, false, false, "config_missing")),
        Some(bytes) => match parse_config(&bytes) {
            Ok(document) => {
                let status = match servers(&document) {
                    Ok(servers) => {
                        let desired = request.server_entry();
                        let entry = servers.and_then(|servers| servers.get(SERVER_NAME));
                        let present = entry.is_some();
                        let matches = entry.is_some_and(|entry| desired.matches(entry));
                        request.report(
                            true,
                            true,
                            present,
                            matches,
                            if matches {
                                "match"
                            } else if present {
                                "mismatch"
                            } else {
                                "not_installed"
                            },
                        )
                    }
                    Err(McpError::ConfigMalformed) => {
                        request.report(true, false, false, false, "config_malformed")
                    }
                    Err(error) => return Err(error),
                };
                Ok(status)
            }
            Err(McpError::ConfigMalformed) => {
                Ok(request.report(true, false, false, false, "config_malformed"))
            }
            Err(error) => Err(error),
        },
    }
}

#[derive(Clone, Debug)]
pub struct DoctorRequest {
    pub client: Client,
    pub config_path: PathBuf,
    pub config_source: ConfigSource,
    pub binary: LocatedBinary,
    pub descriptor: DescriptorLocation,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl DoctorCheck {
    fn passed(name: &'static str) -> Self {
        Self {
            name,
            ok: true,
            code: None,
            remediation: None,
            detail: None,
        }
    }

    fn failed(name: &'static str, error: McpError) -> Self {
        Self {
            name,
            ok: false,
            code: Some(error.code()),
            remediation: Some(error.remediation()),
            detail: None,
        }
    }

    fn failed_with_detail(name: &'static str, error: McpError, detail: String) -> Self {
        Self {
            name,
            ok: false,
            code: Some(error.code()),
            remediation: Some(error.remediation()),
            detail: Some(detail),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub ok: bool,
    pub client: &'static str,
    pub config_path: String,
    pub config_source: &'static str,
    pub binary: String,
    pub binary_source: &'static str,
    pub descriptor: String,
    pub descriptor_source: &'static str,
    pub checks: Vec<DoctorCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
}

impl DoctorRequest {
    fn server_entry(&self) -> ServerEntry {
        ServerEntry {
            command: absolute_path(&self.binary.path)
                .to_string_lossy()
                .into_owned(),
            args: vec![
                DESCRIPTOR_ARGUMENT.to_string(),
                self.descriptor.path.to_string_lossy().into_owned(),
            ],
        }
    }

    fn report(&self, checks: Vec<DoctorCheck>) -> DoctorReport {
        DoctorReport {
            schema_version: REPORT_SCHEMA_VERSION,
            ok: checks.iter().all(|check| check.ok),
            client: self.client.as_str(),
            config_path: self.config_path.to_string_lossy().into_owned(),
            config_source: self.config_source.as_str(),
            binary: self.binary.path.to_string_lossy().into_owned(),
            binary_source: self.binary.source.as_str(),
            descriptor: self.descriptor.path.to_string_lossy().into_owned(),
            descriptor_source: self.descriptor.source.as_str(),
            checks,
            bundle: None,
        }
    }
}

/// Checks the binary, the configuration parse, the owned entry, and stale
/// absolute command paths. Every failing check carries exactly one remediation.
pub fn doctor(request: &DoctorRequest) -> Result<DoctorReport, McpError> {
    let mut checks = Vec::new();
    if request.binary.exists && request.binary.path.is_file() {
        checks.push(DoctorCheck::passed("binary_exists"));
        checks.push(if is_executable(&request.binary.path) {
            DoctorCheck::passed("binary_executable")
        } else {
            DoctorCheck::failed("binary_executable", McpError::BinaryNotExecutable)
        });
    } else {
        checks.push(DoctorCheck::failed(
            "binary_exists",
            McpError::BinaryNotFound,
        ));
    }

    let parsed = match load_config(&request.config_path) {
        Ok(None) => {
            checks.push(DoctorCheck::passed("config_parse"));
            checks.push(DoctorCheck::failed("entry_match", McpError::EntryMissing));
            None
        }
        Ok(Some(bytes)) => match parse_config(&bytes) {
            Ok(document) => {
                checks.push(DoctorCheck::passed("config_parse"));
                Some(document)
            }
            Err(error) => {
                checks.push(DoctorCheck::failed("config_parse", error));
                None
            }
        },
        Err(error) => {
            checks.push(DoctorCheck::failed("config_parse", error));
            None
        }
    };

    if let Some(document) = &parsed {
        match servers(document) {
            Ok(servers) => {
                let desired = request.server_entry();
                let entry = servers.and_then(|servers| servers.get(SERVER_NAME));
                checks.push(if entry.is_some_and(|entry| desired.matches(entry)) {
                    DoctorCheck::passed("entry_match")
                } else if entry.is_some() {
                    DoctorCheck::failed("entry_match", McpError::EntryMismatch)
                } else {
                    DoctorCheck::failed("entry_match", McpError::EntryMissing)
                });
                let stale = servers.map(stale_servers).unwrap_or_default();
                checks.push(if stale.is_empty() {
                    DoctorCheck::passed("stale_absolute_paths")
                } else {
                    DoctorCheck::failed_with_detail(
                        "stale_absolute_paths",
                        McpError::StaleEntryPath,
                        stale.join(","),
                    )
                });
            }
            Err(error) => checks.push(DoctorCheck::failed("entry_match", error)),
        }
    }

    Ok(request.report(checks))
}

fn stale_servers(servers: &ConfigObject) -> Vec<String> {
    servers
        .iter()
        .filter_map(|(name, entry)| {
            let command = entry.object()?.get("command")?.text()?;
            let path = Path::new(command);
            (path.is_absolute() && !path.exists()).then(|| name.clone())
        })
        .collect()
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn servers(document: &ConfigValue) -> Result<Option<&ConfigObject>, McpError> {
    let object = document.object().ok_or(McpError::ConfigMalformed)?;
    match object.get("mcpServers") {
        None => Ok(None),
        Some(value) => value.object().map(Some).ok_or(McpError::ConfigMalformed),
    }
}

fn servers_mut(document: &mut ConfigValue) -> Result<&mut ConfigObject, McpError> {
    let object = document.object_mut().ok_or(McpError::ConfigMalformed)?;
    object
        .entry("mcpServers".to_string())
        .or_insert_with(|| ConfigValue::Object(ConfigObject::new()))
        .object_mut()
        .ok_or(McpError::ConfigMalformed)
}

fn write_target(path: &Path) -> Result<PathBuf, McpError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::canonicalize(path).map_err(|_| McpError::ConfigUnreadable)
        }
        _ => Ok(path.to_path_buf()),
    }
}

fn load_config(path: &Path) -> Result<Option<Vec<u8>>, McpError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(McpError::ConfigUnreadable),
    };
    if !metadata.is_file() {
        return Err(McpError::ConfigUnreadable);
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(McpError::ConfigTooLarge);
    }
    let file = File::open(path).map_err(|_| McpError::ConfigUnreadable)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| McpError::ConfigUnreadable)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(McpError::ConfigTooLarge);
    }
    Ok(Some(bytes))
}

fn parse_config(bytes: &[u8]) -> Result<ConfigValue, McpError> {
    let document: ConfigValue =
        serde_json::from_slice(bytes).map_err(|_| McpError::ConfigMalformed)?;
    if document.object().is_none() {
        return Err(McpError::ConfigMalformed);
    }
    Ok(document)
}

fn encode_config(document: &ConfigValue) -> Result<Vec<u8>, McpError> {
    let mut text = serde_json::to_string_pretty(document).map_err(|_| McpError::ConfigMalformed)?;
    text.push('\n');
    Ok(text.into_bytes())
}

fn absolute_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn timestamp(now: OffsetDateTime) -> String {
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn write_backup(path: &Path, now: OffsetDateTime, bytes: &[u8]) -> Result<PathBuf, McpError> {
    let parent = parent_directory(path);
    fs::create_dir_all(parent).map_err(|_| McpError::ConfigWriteFailed)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| CONFIG_FILE_NAME.to_string());
    let stamp = timestamp(now);
    for attempt in 0..1000_u32 {
        let candidate_name = if attempt == 0 {
            format!("{name}.{stamp}.bak")
        } else {
            format!("{name}.{stamp}.{attempt}.bak")
        };
        let candidate = parent.join(candidate_name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                file.write_all(bytes)
                    .and_then(|()| file.sync_all())
                    .map_err(|_| McpError::ConfigWriteFailed)?;
                return Ok(candidate);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(McpError::ConfigWriteFailed),
        }
    }
    Err(McpError::ConfigWriteFailed)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = parent_directory(path);
    fs::create_dir_all(parent)?;
    let temp = temporary_path(parent, path);
    let result = (|| {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn temporary_path(parent: &Path, target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| CONFIG_FILE_NAME.to_string());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    parent.join(format!(".{name}.tmp.{}.{nanos}", process::id()))
}

fn bundle_value(report: &DoctorReport) -> Value {
    let checks = report
        .checks
        .iter()
        .map(|check| {
            let mut object = serde_json::Map::new();
            object.insert("name".to_string(), Value::String(check.name.to_string()));
            object.insert("ok".to_string(), Value::Bool(check.ok));
            if let Some(code) = check.code {
                object.insert("code".to_string(), Value::String(code.to_string()));
            }
            if let Some(remediation) = check.remediation {
                object.insert(
                    "remediation".to_string(),
                    Value::String(remediation.to_string()),
                );
            }
            Value::Object(object)
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": REPORT_SCHEMA_VERSION,
        "kind": "mcp_doctor",
        "redacted": true,
        "ok": report.ok,
        "client": report.client,
        "config_path": report.config_path,
        "binary": report.binary,
        "checks": checks,
    })
}

fn write_bundle(directory: &Path, report: &DoctorReport) -> Result<PathBuf, McpError> {
    let path = directory.join("mcp_doctor.json");
    let mut text = serde_json::to_string_pretty(&bundle_value(report))
        .map_err(|_| McpError::BundleWriteFailed)?;
    text.push('\n');
    atomic_write(&path, text.as_bytes()).map_err(|_| McpError::BundleWriteFailed)?;
    Ok(path)
}

fn to_failure(error: McpError) -> Failure {
    Failure::actionable(error.code(), error.remediation())
}

/// Runs one verb against the process environment. Only `install` writes.
pub fn run(options: &McpOptions) -> Result<Value, Failure> {
    let environment = Environment::current();
    let current_exe = env::current_exe().ok();
    let path_var = env::var_os("PATH");
    let cwd = env::current_dir().map_err(|_| to_failure(McpError::ConfigUnresolved))?;
    let project_root = options.project_root.clone().unwrap_or_else(|| cwd.clone());
    let (config_path, config_source) =
        resolve_config_path(&environment, options.config.as_deref()).map_err(to_failure)?;
    let binary = locate_binary(
        options.binary.as_deref(),
        current_exe.as_deref(),
        path_var.as_deref(),
    );
    let descriptor = descriptor_location(&project_root);
    let body = match options.verb {
        McpVerb::Install => {
            if !binary.exists {
                return Err(to_failure(McpError::BinaryNotFound));
            }
            let report = install(
                &InstallRequest {
                    client: options.client,
                    config_path,
                    config_source,
                    binary_path: binary.path,
                    descriptor_path: descriptor.path,
                },
                OffsetDateTime::now_utc(),
            )
            .map_err(to_failure)?;
            encode_report(&report)?
        }
        McpVerb::Status => {
            let report = status(&StatusRequest {
                client: options.client,
                config_path,
                config_source,
                binary,
                descriptor,
            })
            .map_err(to_failure)?;
            encode_report(&report)?
        }
        McpVerb::Doctor => {
            let mut report = doctor(&DoctorRequest {
                client: options.client,
                config_path,
                config_source,
                binary,
                descriptor,
            })
            .map_err(to_failure)?;
            if let Some(directory) = &options.bundle {
                let path = write_bundle(directory, &report).map_err(to_failure)?;
                report.bundle = Some(path.to_string_lossy().into_owned());
            }
            encode_report(&report)?
        }
    };
    Ok(body)
}

fn encode_report(report: &impl Serialize) -> Result<Value, Failure> {
    serde_json::to_value(report)
        .map_err(|source| Failure::with_source("mcp_report_encoding_failed", source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixed_time() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_763_456_789).expect("fixed timestamp")
    }

    fn write(path: &Path, contents: &str) {
        fs::write(path, contents).expect("fixture write");
    }

    fn config_with(dir: &Path, contents: &str) -> PathBuf {
        let path = dir.join(CONFIG_FILE_NAME);
        write(&path, contents);
        path
    }

    fn binary_in(dir: &Path) -> PathBuf {
        let path = dir.join(binary_name());
        write(&path, "");
        path
    }

    fn install_request(
        config_path: PathBuf,
        binary_path: PathBuf,
        descriptor_path: PathBuf,
    ) -> InstallRequest {
        InstallRequest {
            client: Client::ClaudeDesktop,
            config_path,
            config_source: ConfigSource::Override,
            binary_path,
            descriptor_path,
        }
    }

    fn parse(config_path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(config_path).expect("config read"))
            .expect("config JSON")
    }

    fn backups(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .expect("directory read")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "bak"))
            .collect()
    }

    #[test]
    fn default_paths_follow_each_supported_platform() {
        let home = PathBuf::from("home-example");
        let macos = Environment {
            platform: Platform::Macos,
            home: Some(home.clone()),
            app_data: None,
        };
        assert_eq!(
            default_config_path(&macos).expect("macOS path"),
            home.join("Library")
                .join("Application Support")
                .join("Claude")
                .join(CONFIG_FILE_NAME)
        );

        let app_data = PathBuf::from("roaming-example");
        let windows = Environment {
            platform: Platform::Windows,
            home: None,
            app_data: Some(app_data.clone()),
        };
        assert_eq!(
            default_config_path(&windows).expect("Windows path"),
            app_data.join("Claude").join(CONFIG_FILE_NAME)
        );

        let unresolved = Environment {
            platform: Platform::Macos,
            home: None,
            app_data: None,
        };
        assert_eq!(
            default_config_path(&unresolved).unwrap_err(),
            McpError::ConfigUnresolved
        );
        assert_eq!(
            default_config_path(&Environment {
                platform: Platform::Other,
                ..Environment::default()
            })
            .unwrap_err(),
            McpError::PlatformUnsupported
        );
    }

    #[test]
    fn an_explicit_config_path_overrides_discovery() {
        let environment = Environment {
            platform: Platform::Other,
            ..Environment::default()
        };
        let (path, source) =
            resolve_config_path(&environment, Some(Path::new("custom.json"))).expect("override");
        assert_eq!(source, ConfigSource::Override);
        assert!(path.is_absolute(), "{path:?}");
        assert_eq!(path.file_name(), Some(OsStr::new("custom.json")));
    }

    #[test]
    fn binary_discovery_prefers_explicit_then_sibling_then_path() {
        let dir = tempdir().expect("temp dir");
        let explicit = dir.path().join("explicit_mcp");
        write(&explicit, "");
        let located = locate_binary(Some(&explicit), None, None);
        assert_eq!(located.source, BinarySource::Explicit);
        assert!(located.exists);

        let bin = dir.path().join("bin");
        fs::create_dir(&bin).expect("bin directory");
        let sibling = bin.join(binary_name());
        write(&sibling, "");
        let located = locate_binary(None, Some(&bin.join("vibemuxctl")), None);
        assert_eq!(located.source, BinarySource::NextToExecutable);
        assert_eq!(located.path, absolute_path(&sibling));

        let path_dir = dir.path().join("path-entry");
        fs::create_dir(&path_dir).expect("path directory");
        let on_path = path_dir.join(binary_name());
        write(&on_path, "");
        let path_var = env::join_paths([dir.path().join("absent"), path_dir]).expect("join paths");
        let located = locate_binary(None, None, Some(&path_var));
        assert_eq!(located.source, BinarySource::PathEntry);
        assert_eq!(located.path, absolute_path(&on_path));

        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir(&elsewhere).expect("elsewhere directory");
        let missing = locate_binary(None, Some(&elsewhere.join("vibemuxctl")), None);
        assert_eq!(missing.source, BinarySource::NotFound);
        assert!(!missing.exists);
        assert_eq!(missing.path, absolute_path(&elsewhere.join(binary_name())));
    }

    #[test]
    fn install_creates_the_entry_and_preserves_unrelated_order() {
        let dir = tempdir().expect("temp dir");
        let original = r#"{
  "globalShortcut": "Cmd+Space",
  "mcpServers": {
    "zeta": {"command": "z", "args": []},
    "alpha": {"command": "a", "args": []}
  },
  "other": {"keep": true}
}"#;
        let config = config_with(dir.path(), original);
        let binary = binary_in(dir.path());
        let descriptor = dir.path().join("control.json");
        let report = install(
            &install_request(config.clone(), binary.clone(), descriptor.clone()),
            fixed_time(),
        )
        .expect("install");
        assert!(report.ok && report.changed);
        assert!(!report.created);
        let backup = report.backup.expect("backup path");
        assert_eq!(fs::read_to_string(backup).expect("backup"), original);

        let text = fs::read_to_string(&config).expect("config");
        let position = |needle: &str| text.find(needle).expect(needle);
        assert!(position("\"globalShortcut\"") < position("\"mcpServers\""));
        assert!(position("\"mcpServers\"") < position("\"other\""));
        assert!(position("\"zeta\"") < position("\"alpha\""));
        assert!(position("\"alpha\"") < position("\"vibemux\""));

        let document = parse(&config);
        assert_eq!(
            document["mcpServers"]["vibemux"]["command"],
            Value::String(absolute_path(&binary).to_string_lossy().into_owned())
        );
        assert_eq!(
            document["mcpServers"]["vibemux"]["args"],
            json!([DESCRIPTOR_ARGUMENT, descriptor.to_string_lossy()])
        );
        assert_eq!(document["other"]["keep"], Value::Bool(true));
        assert_eq!(backups(dir.path()).len(), 1);
    }

    #[test]
    fn install_is_idempotent_and_replaces_an_existing_entry_in_place() {
        let dir = tempdir().expect("temp dir");
        let config = config_with(
            dir.path(),
            r#"{"mcpServers":{"a":{"command":"a"},"vibemux":{"command":"old","args":[]},"b":{"command":"b"}}}"#,
        );
        let binary = binary_in(dir.path());
        let descriptor = dir.path().join("control.json");
        let request = install_request(config.clone(), binary, descriptor);
        assert!(install(&request, fixed_time()).expect("first").changed);
        let first_backups = backups(dir.path());
        assert_eq!(first_backups.len(), 1);

        let second = install(&request, fixed_time()).expect("second");
        assert!(!second.changed);
        assert!(second.backup.is_none());
        assert_eq!(backups(dir.path()).len(), 1);

        let text = fs::read_to_string(&config).expect("config");
        let position = |needle: &str| text.find(needle).expect(needle);
        assert!(position("\"a\"") < position("\"vibemux\""));
        assert!(position("\"vibemux\"") < position("\"b\""));
    }

    #[test]
    #[cfg(unix)]
    fn install_writes_through_a_symlinked_configuration() {
        let dir = tempdir().expect("temp dir");
        let target = dir.path().join("managed_config.json");
        write(&target, r#"{"mcpServers":{}}"#);
        let link = dir.path().join(CONFIG_FILE_NAME);
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let binary = binary_in(dir.path());
        install(
            &install_request(link.clone(), binary, dir.path().join("control.json")),
            fixed_time(),
        )
        .expect("install");
        let metadata = fs::symlink_metadata(&link).expect("link metadata");
        assert!(metadata.file_type().is_symlink());
        let document = parse(&target);
        assert_eq!(
            document["mcpServers"]["vibemux"]["args"][0],
            json!(DESCRIPTOR_ARGUMENT)
        );
    }

    #[test]
    fn install_creates_a_missing_config_without_a_backup() {
        let dir = tempdir().expect("temp dir");
        let config = dir.path().join("nested").join(CONFIG_FILE_NAME);
        let binary = binary_in(dir.path());
        let report = install(
            &install_request(config.clone(), binary, dir.path().join("control.json")),
            fixed_time(),
        )
        .expect("install");
        assert!(report.changed && report.created);
        assert!(report.backup.is_none());
        assert_eq!(
            parse(&config)["mcpServers"]["vibemux"]["args"][0],
            json!(DESCRIPTOR_ARGUMENT)
        );
    }

    #[test]
    fn malformed_json_is_refused_without_touching_the_file() {
        let dir = tempdir().expect("temp dir");
        let original = "{ this is not json";
        let config = config_with(dir.path(), original);
        let binary = binary_in(dir.path());
        let error = install(
            &install_request(config.clone(), binary, dir.path().join("control.json")),
            fixed_time(),
        )
        .unwrap_err();
        assert_eq!(error, McpError::ConfigMalformed);
        assert_eq!(fs::read_to_string(&config).expect("config"), original);
        assert!(backups(dir.path()).is_empty());
    }

    #[test]
    fn a_non_object_server_map_is_refused_without_touching_the_file() {
        let dir = tempdir().expect("temp dir");
        let original = r#"{"mcpServers": []}"#;
        let config = config_with(dir.path(), original);
        let binary = binary_in(dir.path());
        let error = install(
            &install_request(config.clone(), binary, dir.path().join("control.json")),
            fixed_time(),
        )
        .unwrap_err();
        assert_eq!(error, McpError::ConfigMalformed);
        assert_eq!(fs::read_to_string(&config).expect("config"), original);
    }

    fn status_request(config_path: PathBuf, binary: PathBuf, descriptor: PathBuf) -> StatusRequest {
        StatusRequest {
            client: Client::ClaudeDesktop,
            config_path,
            config_source: ConfigSource::Override,
            binary: LocatedBinary {
                exists: binary.is_file(),
                path: absolute_path(&binary),
                source: BinarySource::Explicit,
            },
            descriptor: DescriptorLocation {
                path: descriptor,
                source: DescriptorSource::Runtime,
            },
        }
    }

    #[test]
    fn status_reports_every_state_without_writing() {
        let dir = tempdir().expect("temp dir");
        let binary = binary_in(dir.path());
        let descriptor = dir.path().join("control.json");

        let missing = dir.path().join("missing.json");
        let report = status(&status_request(
            missing.clone(),
            binary.clone(),
            descriptor.clone(),
        ))
        .expect("status");
        assert_eq!(report.status, "config_missing");
        assert!(!report.config_exists && !report.ok);
        assert!(!missing.exists());

        let config = config_with(
            dir.path(),
            r#"{"mcpServers":{"vibemux":{"command":"elsewhere","args":[]}}}"#,
        );
        let before = fs::read_to_string(&config).expect("config");
        let report = status(&status_request(
            config.clone(),
            binary.clone(),
            descriptor.clone(),
        ))
        .expect("status");
        assert_eq!(report.status, "mismatch");
        assert!(report.entry_present && !report.entry_matches && !report.ok);
        assert_eq!(fs::read_to_string(&config).expect("config"), before);
        assert!(backups(dir.path()).is_empty());

        let config = config_with(dir.path(), r#"{"mcpServers":{}}"#);
        let report =
            status(&status_request(config, binary.clone(), descriptor.clone())).expect("status");
        assert_eq!(report.status, "not_installed");
        assert!(!report.entry_present);

        let config = config_with(dir.path(), "{ broken");
        let report =
            status(&status_request(config, binary.clone(), descriptor.clone())).expect("status");
        assert_eq!(report.status, "config_malformed");
        assert!(report.config_exists && !report.config_valid && !report.ok);

        let config = config_with(dir.path(), r#"{"mcpServers": []}"#);
        let report =
            status(&status_request(config, binary.clone(), descriptor.clone())).expect("status");
        assert_eq!(report.status, "config_malformed");
        assert!(!report.config_valid);

        let config = config_with(dir.path(), "{}");
        let install_request = install_request(config.clone(), binary, descriptor);
        install(&install_request, fixed_time()).expect("install");
        let request = status_request(
            config,
            install_request.binary_path,
            install_request.descriptor_path,
        );
        let report = status(&request).expect("status");
        assert_eq!(report.status, "match");
        assert!(report.ok && report.entry_matches);
    }

    fn doctor_request(config_path: PathBuf, binary: PathBuf, descriptor: PathBuf) -> DoctorRequest {
        DoctorRequest {
            client: Client::ClaudeDesktop,
            config_path,
            config_source: ConfigSource::Override,
            binary: LocatedBinary {
                exists: binary.is_file(),
                path: absolute_path(&binary),
                source: BinarySource::Explicit,
            },
            descriptor: DescriptorLocation {
                path: descriptor,
                source: DescriptorSource::Runtime,
            },
        }
    }

    #[test]
    fn doctor_names_one_remediation_for_each_failure() {
        let dir = tempdir().expect("temp dir");
        let missing_binary = dir.path().join(binary_name());
        let config = config_with(dir.path(), "{ broken");
        let report = doctor(&doctor_request(
            config,
            missing_binary,
            dir.path().join("control.json"),
        ))
        .expect("doctor");
        assert!(!report.ok);
        let binary = report
            .checks
            .iter()
            .find(|check| check.name == "binary_exists")
            .expect("binary check");
        assert!(!binary.ok);
        assert_eq!(binary.code, Some("mcp_binary_not_found"));
        assert_eq!(
            binary.remediation,
            Some(McpError::BinaryNotFound.remediation())
        );
        let config = report
            .checks
            .iter()
            .find(|check| check.name == "config_parse")
            .expect("parse check");
        assert!(!config.ok);
        assert_eq!(config.code, Some("mcp_config_malformed"));
        for check in report.checks.iter().filter(|check| !check.ok) {
            assert!(check.remediation.is_some(), "{}", check.name);
        }
    }

    #[test]
    fn doctor_flags_entry_mismatch_and_stale_absolute_paths() {
        let dir = tempdir().expect("temp dir");
        let binary = binary_in(dir.path());
        let config = config_with(
            dir.path(),
            r#"{"mcpServers":{"stale":{"command":"/definitely/not/here/vibemux_mcp","args":[]},"vibemux":{"command":"elsewhere","args":[]}}}"#,
        );
        let report = doctor(&doctor_request(
            config,
            binary,
            dir.path().join("control.json"),
        ))
        .expect("doctor");
        let mismatch = report
            .checks
            .iter()
            .find(|check| check.name == "entry_match")
            .expect("entry check");
        assert_eq!(mismatch.code, Some("mcp_entry_mismatch"));
        let stale = report
            .checks
            .iter()
            .find(|check| check.name == "stale_absolute_paths")
            .expect("stale check");
        assert_eq!(stale.code, Some("mcp_stale_entry_path"));
        assert!(
            stale
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("stale"))
        );
    }

    #[test]
    #[cfg(unix)]
    fn doctor_rejects_a_binary_without_an_execute_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().expect("temp dir");
        let binary = binary_in(dir.path());
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o644)).expect("permissions");
        let report = doctor(&doctor_request(
            config_with(dir.path(), "{}"),
            binary.clone(),
            dir.path().join("control.json"),
        ))
        .expect("doctor");
        let check = report
            .checks
            .iter()
            .find(|check| check.name == "binary_executable")
            .expect("execute check");
        assert_eq!(check.code, Some("mcp_binary_not_executable"));

        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("permissions");
        let report = doctor(&doctor_request(
            dir.path().join(CONFIG_FILE_NAME),
            binary,
            dir.path().join("control.json"),
        ))
        .expect("doctor");
        assert!(
            report
                .checks
                .iter()
                .find(|check| check.name == "binary_executable")
                .is_some_and(|check| check.ok)
        );
    }

    #[test]
    fn the_bundle_is_redacted_and_keeps_only_paths_and_checks() {
        let dir = tempdir().expect("temp dir");
        let binary = binary_in(dir.path());
        let config = config_with(
            dir.path(),
            r#"{"mcpServers":{"vibemux":{"command":"elsewhere","args":[],"env":{"TOKEN":"SUPER-SECRET-TOKEN"}}}}"#,
        );
        let report = doctor(&doctor_request(
            config.clone(),
            binary.clone(),
            dir.path().join("control.json"),
        ))
        .expect("doctor");
        let bundle_dir = dir.path().join("bundle");
        let path = write_bundle(&bundle_dir, &report).expect("bundle");
        let text = fs::read_to_string(&path).expect("bundle text");
        assert!(!text.contains("SUPER-SECRET-TOKEN"));
        assert!(!text.contains("descriptor"));
        let value: Value = serde_json::from_str(&text).expect("bundle JSON");
        assert_eq!(value["redacted"], Value::Bool(true));
        assert_eq!(
            value["config_path"],
            Value::String(config.to_string_lossy().into_owned())
        );
        assert_eq!(
            value["binary"],
            Value::String(absolute_path(&binary).to_string_lossy().into_owned())
        );
        assert!(
            value["checks"]
                .as_array()
                .is_some_and(|checks| !checks.is_empty())
        );
    }

    #[test]
    fn timestamps_are_second_precision_and_file_name_safe() {
        let epoch = OffsetDateTime::from_unix_timestamp(0).expect("epoch");
        assert_eq!(timestamp(epoch), "19700101T000000Z");
    }

    #[test]
    fn the_descriptor_is_derived_from_the_project_runtime() {
        let dir = tempdir().expect("temp dir");
        let location = descriptor_location(dir.path());
        assert!(location.path.is_absolute());
        assert_eq!(location.path.file_name(), Some(OsStr::new("control.json")));
        assert_eq!(location.source, DescriptorSource::Runtime);
    }

    fn os_arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn arguments_are_strict_and_client_limited() {
        let options = parse_arguments(&os_arguments(&[
            "install",
            "--project-root",
            "project",
            "--client",
            "claude-desktop",
            "--binary",
            "bin/vibemux_mcp",
            "--config",
            "config.json",
        ]))
        .expect("install options");
        assert_eq!(options.verb, McpVerb::Install);
        assert_eq!(options.project_root, Some(PathBuf::from("project")));
        assert_eq!(options.binary, Some(PathBuf::from("bin/vibemux_mcp")));
        assert_eq!(options.config, Some(PathBuf::from("config.json")));
        assert_eq!(options.bundle, None);

        let doctor =
            parse_arguments(&os_arguments(&["doctor", "--bundle", "out"])).expect("doctor options");
        assert_eq!(doctor.verb, McpVerb::Doctor);
        assert_eq!(doctor.bundle, Some(PathBuf::from("out")));

        assert_eq!(
            parse_arguments(&os_arguments(&["install"]))
                .expect("defaults")
                .client,
            Client::ClaudeDesktop
        );
        assert_eq!(
            parse_arguments(&os_arguments(&["status", "--client", "other"])).unwrap_err(),
            ParseError::UnsupportedClient
        );
        for arguments in [
            &["install", "--bundle", "out"][..],
            &["status", "--bundle", "out"][..],
            &["install", "--config"][..],
            &["install", "--config", "--binary"][..],
            &["install", "--config", "a", "--config", "b"][..],
            &["install", "positional"][..],
            &["unknown"][..],
            &[][..],
        ] {
            assert_eq!(
                parse_arguments(&os_arguments(arguments)),
                Err(ParseError::InvalidArguments),
                "{arguments:?}"
            );
        }
    }
}
