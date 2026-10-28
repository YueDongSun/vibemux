//! Offline command construction for the fixed Rust MXC sandbox probe.
//!
//! This adapter validates its inputs and constructs a [`Command`]; it never
//! spawns one. It does not certify a harness, enable a workflow, or provide a
//! general containment boundary. Canonicalization is launch-time validation,
//! not a TOCTOU guarantee. The caller must own the validated paths and prevent
//! their replacement, reparse retargeting, or permission changes until launch.
//! The caller also supplies a fresh profile id and owns stdio, deadlines, the
//! launch trampoline, and the complete child lifecycle.
//!
//! Threat model: caller-supplied paths, profile text, arguments, and ambient
//! environment are untrusted. Local drive-only canonical checks, component-wise
//! containment, explicit permission conflicts, TOML serialization, a fixed argv
//! prefix, and an exact environment allowlist fail closed before construction.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use thiserror::Error;

/// Filesystem inputs for one fresh, offline MXC permission profile.
#[derive(Clone, Eq, PartialEq)]
pub struct MxcOfflinePolicy {
    pub workspace: PathBuf,
    pub scratch: PathBuf,
    pub read_only_paths: Vec<PathBuf>,
    pub denied_paths: Vec<PathBuf>,
    pub profile_id: String,
}

/// Content-free failures produced before an offline command is returned.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MxcError {
    #[error("mxc offline command construction is supported only on Windows")]
    UnsupportedOs,
    #[error("mxc profile id does not match the required offline profile format")]
    InvalidProfileId,
    #[error("mxc path is not an approved canonical local drive path")]
    InvalidPath,
    #[error("mxc path could not be resolved")]
    PathResolution,
    #[error("mxc path differs from its canonical identity")]
    PathNotCanonical,
    #[error("mxc path has the wrong file type")]
    InvalidPathType,
    #[error("mxc backend is not a canonical codex.exe file")]
    InvalidBackend,
    #[error("mxc scratch must be strictly below the workspace")]
    ScratchOutsideWorkspace,
    #[error("mxc protected paths must be strictly below the workspace")]
    ProtectedPathOutsideWorkspace,
    #[error("mxc protected path limit exceeded")]
    TooManyProtectedPaths,
    #[error("mxc path permissions contain a duplicate or conflict")]
    ConflictingPaths,
    #[error("mxc argument count limit exceeded")]
    TooManyArguments,
    #[error("mxc aggregate argument size limit exceeded")]
    ArgumentsTooLarge,
    #[error("mxc argument contains a NUL character")]
    ArgumentContainsNul,
    #[error("mxc bootstrap environment is missing or invalid")]
    InvalidEnvironment,
}

impl MxcError {
    /// Stable, content-free machine code for this failure.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedOs => "mxc_unsupported_os",
            Self::InvalidProfileId => "mxc_invalid_profile_id",
            Self::InvalidPath => "mxc_invalid_path",
            Self::PathResolution => "mxc_path_resolution_failed",
            Self::PathNotCanonical => "mxc_path_not_canonical",
            Self::InvalidPathType => "mxc_invalid_path_type",
            Self::InvalidBackend => "mxc_invalid_backend",
            Self::ScratchOutsideWorkspace => "mxc_scratch_outside_workspace",
            Self::ProtectedPathOutsideWorkspace => "mxc_protected_path_outside_workspace",
            Self::TooManyProtectedPaths => "mxc_too_many_protected_paths",
            Self::ConflictingPaths => "mxc_conflicting_paths",
            Self::TooManyArguments => "mxc_too_many_arguments",
            Self::ArgumentsTooLarge => "mxc_arguments_too_large",
            Self::ArgumentContainsNul => "mxc_argument_contains_nul",
            Self::InvalidEnvironment => "mxc_invalid_environment",
        }
    }
}

/// Validates and constructs the fixed offline command without spawning it.
///
/// All caller arguments are appended verbatim after the fixed `--` delimiter.
/// On non-Windows hosts this always returns [`MxcError::UnsupportedOs`].
pub fn offline_command(
    backend: &Path,
    executable: &Path,
    arguments: &[OsString],
    policy: &MxcOfflinePolicy,
) -> Result<Command, MxcError> {
    #[cfg(windows)]
    {
        windows::offline_command(backend, executable, arguments, policy)
    }
    #[cfg(not(windows))]
    {
        let _ = (backend, executable, arguments, policy);
        Err(MxcError::UnsupportedOs)
    }
}

#[cfg(windows)]
mod windows {
    use std::{
        env,
        ffi::{OsStr, OsString},
        fs,
        path::{Component, Path, PathBuf, Prefix},
        process::Command,
    };

    use super::{MxcError, MxcOfflinePolicy};
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const EXTENDED_DRIVE_PREFIX: &str = r"\\?\";
    const PROFILE_PREFIX: &str = "rd_mxc_";
    const PROFILE_HEX_LENGTH: usize = 32;
    const MAX_PROTECTED_PATHS: usize = 32;
    const MAX_ARGUMENTS: usize = 128;
    const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
    const BOOTSTRAP_ENVIRONMENT_NAMES: [&str; 13] = [
        "SystemRoot",
        "WINDIR",
        "SystemDrive",
        "COMSPEC",
        "PATHEXT",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "LOCALAPPDATA",
        "APPDATA",
        "PROGRAMDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
    ];

    #[derive(Clone, Copy)]
    enum ExpectedPathType {
        Directory,
        File,
        FileOrDirectory,
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Access {
        Read,
        Deny,
    }

    impl Access {
        const fn as_str(self) -> &'static str {
            match self {
                Self::Read => "read",
                Self::Deny => "deny",
            }
        }
    }

    #[derive(Clone)]
    struct CanonicalPath {
        path: PathBuf,
        text: String,
    }

    impl CanonicalPath {
        fn new(path: PathBuf) -> Result<Self, MxcError> {
            let text = path.to_str().ok_or(MxcError::InvalidPath)?.to_owned();
            Ok(Self { path, text })
        }

        fn same_path(&self, other: &Self) -> bool {
            self.path == other.path
        }

        fn is_same_or_below(&self, parent: &Self) -> bool {
            // All paths already have their canonical spelling. Case folding
            // would conflate distinct Windows case-sensitive directories.
            self.path.starts_with(&parent.path)
        }

        fn is_strictly_below(&self, parent: &Self) -> bool {
            !self.same_path(parent) && self.is_same_or_below(parent)
        }

        fn overlaps(&self, other: &Self) -> bool {
            self.is_same_or_below(other) || other.is_same_or_below(self)
        }
    }

    struct ProtectedPath {
        path: CanonicalPath,
        access: Access,
    }

    pub(super) fn offline_command(
        backend: &Path,
        executable: &Path,
        arguments: &[OsString],
        policy: &MxcOfflinePolicy,
    ) -> Result<Command, MxcError> {
        validate_profile_id(&policy.profile_id)?;
        validate_arguments(arguments)?;
        if policy.read_only_paths.len() > MAX_PROTECTED_PATHS
            || policy.denied_paths.len() > MAX_PROTECTED_PATHS
        {
            return Err(MxcError::TooManyProtectedPaths);
        }

        let backend = canonical_existing(backend, ExpectedPathType::File)?;
        let backend_is_codex = backend
            .path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("codex.exe"));
        if !backend_is_codex {
            return Err(MxcError::InvalidBackend);
        }
        let executable = canonical_existing(executable, ExpectedPathType::File)?;
        let workspace = canonical_existing(&policy.workspace, ExpectedPathType::Directory)?;
        let scratch = canonical_existing(&policy.scratch, ExpectedPathType::Directory)?;
        if !scratch.is_strictly_below(&workspace) {
            return Err(MxcError::ScratchOutsideWorkspace);
        }

        let protected = canonicalize_protected_paths(policy)?;
        validate_path_relationships(&workspace, &scratch, &executable, &protected)?;
        let filesystem = filesystem_inline_table(&workspace, &executable, &protected);

        let system_root = env::var_os("SystemRoot").ok_or(MxcError::InvalidEnvironment)?;
        let input = normalize_local_drive_path(&PathBuf::from(system_root))
            .map_err(|_| MxcError::InvalidEnvironment)?;
        let canonical = fs::canonicalize(&input).map_err(|_| MxcError::InvalidEnvironment)?;
        let normalized =
            normalize_local_drive_path(&canonical).map_err(|_| MxcError::InvalidEnvironment)?;
        // Windows commonly exports C:\\WINDOWS while the directory's actual
        // spelling is C:\\Windows. Allow only that ASCII spelling difference
        // for the OS bootstrap path; a reparse alias still fails.
        if !input.to_str().is_some_and(|input| {
            normalized
                .to_str()
                .is_some_and(|canonical| input.eq_ignore_ascii_case(canonical))
        }) {
            return Err(MxcError::InvalidEnvironment);
        }
        let system_root = canonical_existing(&canonical, ExpectedPathType::Directory)
            .map_err(|_| MxcError::InvalidEnvironment)?;
        let system32 = canonical_existing(
            &system_root.path.join("System32"),
            ExpectedPathType::Directory,
        )
        .map_err(|_| MxcError::InvalidEnvironment)?;
        let executable_parent = executable.path.parent().ok_or(MxcError::InvalidPath)?;
        let backend_parent = backend.path.parent().ok_or(MxcError::InvalidPath)?;
        let search_path =
            env::join_paths([executable_parent, backend_parent, system32.path.as_path()])
                .map_err(|_| MxcError::InvalidEnvironment)?;

        let filesystem_config =
            format!("permissions.{}.filesystem={filesystem}", policy.profile_id);
        let network_config = format!("permissions.{}.network.enabled=false", policy.profile_id);
        let mut command = Command::new(&backend.path);
        command
            .arg("-c")
            .arg("windows.sandbox=mxc")
            .arg("-c")
            .arg("features.network_proxy=false")
            .arg("-c")
            .arg(filesystem_config)
            .arg("-c")
            .arg(network_config)
            .arg("sandbox")
            .arg("--include-managed-config")
            .arg("--permission-profile")
            .arg(&policy.profile_id)
            .arg("--cd")
            .arg(&workspace.path)
            .arg("--")
            .arg(&executable.path)
            .args(arguments)
            .current_dir(&workspace.path)
            .env_clear();
        for name in BOOTSTRAP_ENVIRONMENT_NAMES {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("PATH", search_path)
            .env("TEMP", &scratch.path)
            .env("TMP", &scratch.path);
        command.creation_flags(CREATE_NO_WINDOW);
        Ok(command)
    }

    fn validate_profile_id(profile_id: &str) -> Result<(), MxcError> {
        let suffix = profile_id
            .strip_prefix(PROFILE_PREFIX)
            .ok_or(MxcError::InvalidProfileId)?;
        if suffix.len() != PROFILE_HEX_LENGTH
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(MxcError::InvalidProfileId);
        }
        Ok(())
    }

    fn validate_arguments(arguments: &[OsString]) -> Result<(), MxcError> {
        if arguments.len() > MAX_ARGUMENTS {
            return Err(MxcError::TooManyArguments);
        }
        let mut aggregate_bytes = 0usize;
        for argument in arguments {
            let encoded = argument.as_os_str().as_encoded_bytes();
            if encoded.contains(&0) {
                return Err(MxcError::ArgumentContainsNul);
            }
            aggregate_bytes = aggregate_bytes
                .checked_add(encoded.len())
                .ok_or(MxcError::ArgumentsTooLarge)?;
            if aggregate_bytes > MAX_ARGUMENT_BYTES {
                return Err(MxcError::ArgumentsTooLarge);
            }
        }
        Ok(())
    }

    fn canonical_existing(
        path: &Path,
        expected_type: ExpectedPathType,
    ) -> Result<CanonicalPath, MxcError> {
        let normalized_input = normalize_local_drive_path(path)?;
        let canonical = fs::canonicalize(path).map_err(|_| MxcError::PathResolution)?;
        let normalized_canonical = normalize_local_drive_path(&canonical)?;
        if normalized_input.as_os_str() != normalized_canonical.as_os_str() {
            return Err(MxcError::PathNotCanonical);
        }
        let metadata = fs::metadata(&canonical).map_err(|_| MxcError::PathResolution)?;
        let correct_type = match expected_type {
            ExpectedPathType::Directory => metadata.is_dir(),
            ExpectedPathType::File => metadata.is_file(),
            ExpectedPathType::FileOrDirectory => metadata.is_file() || metadata.is_dir(),
        };
        if !correct_type {
            return Err(MxcError::InvalidPathType);
        }
        CanonicalPath::new(normalized_canonical)
    }

    fn normalize_local_drive_path(path: &Path) -> Result<PathBuf, MxcError> {
        let text = path.to_str().ok_or(MxcError::InvalidPath)?;
        if text.chars().any(char::is_control) {
            return Err(MxcError::InvalidPath);
        }
        let prefix = match path.components().next() {
            Some(Component::Prefix(prefix)) => prefix.kind(),
            _ => return Err(MxcError::InvalidPath),
        };
        let normalized = match prefix {
            Prefix::Disk(_) => path.to_path_buf(),
            Prefix::VerbatimDisk(_) => text
                .strip_prefix(EXTENDED_DRIVE_PREFIX)
                .map(PathBuf::from)
                .ok_or(MxcError::InvalidPath)?,
            _ => return Err(MxcError::InvalidPath),
        };
        if !normalized.is_absolute() {
            return Err(MxcError::InvalidPath);
        }
        let mut components = normalized.components();
        if !matches!(components.next(), Some(Component::Prefix(_)))
            || !matches!(components.next(), Some(Component::RootDir))
        {
            return Err(MxcError::InvalidPath);
        }
        if !matches!(components.next(), Some(Component::Normal(_))) {
            return Err(MxcError::InvalidPath);
        }
        if components.any(|component| !matches!(component, Component::Normal(_))) {
            return Err(MxcError::InvalidPath);
        }
        Ok(normalized)
    }

    fn canonicalize_protected_paths(
        policy: &MxcOfflinePolicy,
    ) -> Result<Vec<ProtectedPath>, MxcError> {
        let mut protected =
            Vec::with_capacity(policy.read_only_paths.len() + policy.denied_paths.len());
        for (paths, access) in [
            (&policy.read_only_paths, Access::Read),
            (&policy.denied_paths, Access::Deny),
        ] {
            for path in paths {
                protected.push(ProtectedPath {
                    path: canonical_existing(path, ExpectedPathType::FileOrDirectory)?,
                    access,
                });
            }
        }
        Ok(protected)
    }

    fn validate_path_relationships(
        workspace: &CanonicalPath,
        scratch: &CanonicalPath,
        executable: &CanonicalPath,
        protected: &[ProtectedPath],
    ) -> Result<(), MxcError> {
        for (index, current) in protected.iter().enumerate() {
            if !current.path.is_strictly_below(workspace) {
                return Err(MxcError::ProtectedPathOutsideWorkspace);
            }
            if current.path.overlaps(scratch) {
                return Err(MxcError::ConflictingPaths);
            }
            for previous in &protected[..index] {
                if current.path.same_path(&previous.path)
                    || (current.access != previous.access && current.path.overlaps(&previous.path))
                {
                    return Err(MxcError::ConflictingPaths);
                }
            }
            match current.access {
                Access::Deny if executable.is_same_or_below(&current.path) => {
                    return Err(MxcError::ConflictingPaths);
                }
                Access::Read if executable.same_path(&current.path) => {
                    return Err(MxcError::ConflictingPaths);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn filesystem_inline_table(
        workspace: &CanonicalPath,
        executable: &CanonicalPath,
        protected: &[ProtectedPath],
    ) -> String {
        let mut rules = Vec::with_capacity(5 + protected.len());
        rules.extend([
            (":root", "deny"),
            (":minimal", "read"),
            (":slash_tmp", "deny"),
        ]);
        // TEMP/TMP name owned scratch. :tmpdir must stay absent so it cannot
        // resolve back to scratch and override the workspace write grant.
        rules.push((workspace.text.as_str(), "write"));
        rules.push((executable.text.as_str(), "read"));
        rules.extend(
            protected
                .iter()
                .map(|entry| (entry.path.text.as_str(), entry.access.as_str())),
        );
        // Serialize as a table so TOML applies key quoting rules. Serializing a
        // path as a standalone string can choose a multiline literal, which is
        // valid as a value but invalid as an inline-table key.
        let table: toml::Table = rules
            .into_iter()
            .map(|(key, value)| (key.to_owned(), toml::Value::String(value.to_owned())))
            .collect();
        toml::Value::Table(table).to_string()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::{
        collections::BTreeMap,
        env,
        ffi::{OsStr, OsString},
        fs::{self, File},
        path::{Path, PathBuf},
        process::Command,
    };

    use super::{MxcError, MxcOfflinePolicy, offline_command};
    use std::os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::symlink_dir,
    };
    use tempfile::{TempDir, tempdir};

    const PROFILE_ID: &str = "rd_mxc_0123456789abcdef0123456789abcdef";
    const BOOTSTRAP_ENVIRONMENT_NAMES: [&str; 13] = [
        "SystemRoot",
        "WINDIR",
        "SystemDrive",
        "COMSPEC",
        "PATHEXT",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "LOCALAPPDATA",
        "APPDATA",
        "PROGRAMDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
    ];

    struct Fixture {
        _temp: TempDir,
        workspace: PathBuf,
        scratch: PathBuf,
        read_only: PathBuf,
        denied: PathBuf,
        outside: PathBuf,
        runtime: PathBuf,
        backend: PathBuf,
        executable: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempdir().expect("disposable fixture directory");
            let root = temp.path().to_path_buf();
            let workspace = root.join("workspace with 'quoted' #1");
            let scratch = workspace.join("scratch space");
            let read_only = workspace.join("read only 'assets'");
            let denied = workspace.join("denied scope");
            // The sibling shares the workspace's complete string prefix; a
            // string-prefix containment check would wrongly accept it.
            let outside = root.join("workspace with 'quoted' #1-evil");
            let runtime = root.join("runtime binaries");
            let backend = runtime.join("codex.exe");
            let executable = runtime.join("runtime 'host'.exe");
            for directory in [
                &workspace, &scratch, &read_only, &denied, &outside, &runtime,
            ] {
                fs::create_dir_all(directory).expect("fixture directory");
            }
            File::create(&backend).expect("fixture backend");
            File::create(&executable).expect("fixture executable");
            Self {
                _temp: temp,
                workspace,
                scratch,
                read_only,
                denied,
                outside,
                runtime,
                backend,
                executable,
            }
        }

        fn policy(&self) -> MxcOfflinePolicy {
            MxcOfflinePolicy {
                workspace: self.workspace.clone(),
                scratch: self.scratch.clone(),
                read_only_paths: vec![self.read_only.clone()],
                denied_paths: vec![self.denied.clone()],
                profile_id: PROFILE_ID.to_owned(),
            }
        }

        fn command(&self) -> Result<Command, MxcError> {
            self.command_with(&self.policy(), &[])
        }

        fn command_with(
            &self,
            policy: &MxcOfflinePolicy,
            arguments: &[OsString],
        ) -> Result<Command, MxcError> {
            offline_command(&self.backend, &self.executable, arguments, policy)
        }
    }

    fn expect_command(result: Result<Command, MxcError>) -> Command {
        match result {
            Ok(command) => command,
            Err(error) => panic!("unexpected offline command error: {}", error.code()),
        }
    }

    fn assert_error(result: Result<Command, MxcError>, expected: MxcError) {
        match result {
            Ok(_) => panic!("offline command unexpectedly succeeded"),
            Err(error) => assert_eq!(error, expected),
        }
    }

    fn command_arguments(command: &Command) -> Vec<OsString> {
        command.get_args().map(OsStr::to_os_string).collect()
    }

    fn filesystem_table(command: &Command, profile_id: &str) -> toml::Table {
        let arguments = command_arguments(command);
        let configuration = arguments[5].to_str().expect("UTF-8 policy");
        let prefix = format!("permissions.{profile_id}.filesystem=");
        let inline = configuration
            .strip_prefix(&prefix)
            .expect("fixed filesystem configuration key");
        let document = format!("filesystem = {inline}");
        let parsed: toml::Value = toml::from_str(&document).expect("valid TOML policy");
        parsed
            .get("filesystem")
            .and_then(toml::Value::as_table)
            .expect("filesystem inline table")
            .clone()
    }

    fn assert_permission(table: &toml::Table, path: &Path, permission: &str) {
        let key = path.to_str().expect("Unicode fixture path");
        assert_eq!(
            table.get(key),
            Some(&toml::Value::String(permission.to_owned()))
        );
    }

    #[test]
    fn constructs_exact_fixed_argv_and_canonical_paths() {
        let fixture = Fixture::new();
        let command = expect_command(fixture.command());
        let arguments = command_arguments(&command);
        assert_eq!(arguments.len(), 16);
        let expected: Vec<OsString> = [
            "-c",
            "windows.sandbox=mxc",
            "-c",
            "features.network_proxy=false",
            "-c",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(&arguments[..5], expected.as_slice());
        assert!(
            arguments[5]
                .to_string_lossy()
                .starts_with(&format!("permissions.{PROFILE_ID}.filesystem={{ "))
        );
        let expected: Vec<OsString> = [
            "-c",
            &format!("permissions.{PROFILE_ID}.network.enabled=false"),
            "sandbox",
            "--include-managed-config",
            "--permission-profile",
            PROFILE_ID,
            "--cd",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(&arguments[6..13], expected.as_slice());
        assert_eq!(arguments[13], fixture.workspace.as_os_str());
        assert_eq!(arguments[14], "--");
        assert_eq!(arguments[15], fixture.executable.as_os_str());
        assert_eq!(command.get_program(), fixture.backend.as_os_str());
        assert_eq!(command.get_current_dir(), Some(fixture.workspace.as_path()));
    }

    #[test]
    fn serializes_permissions_as_a_closed_toml_inline_table() {
        let fixture = Fixture::new();
        let command = expect_command(fixture.command());
        let table = filesystem_table(&command, PROFILE_ID);
        assert_eq!(table.len(), 7);
        assert_eq!(
            table.get(":root"),
            Some(&toml::Value::String("deny".to_owned()))
        );
        assert_eq!(
            table.get(":minimal"),
            Some(&toml::Value::String("read".to_owned()))
        );
        assert_eq!(
            table.get(":slash_tmp"),
            Some(&toml::Value::String("deny".to_owned()))
        );
        assert!(!table.contains_key(":tmpdir"));
        assert!(!table.contains_key(":workspace_roots"));
        assert_permission(&table, &fixture.workspace, "write");
        assert_permission(&table, &fixture.executable, "read");
        assert_permission(&table, &fixture.read_only, "read");
        assert_permission(&table, &fixture.denied, "deny");
    }

    #[test]
    fn arguments_cannot_inject_configuration_before_the_delimiter() {
        let fixture = Fixture::new();
        let injected = vec![
            OsString::from("-c"),
            OsString::from("features.network_proxy=true"),
            OsString::from("--permission-profile"),
            OsString::from("inherited_profile"),
            OsString::from("spaces and \"double\" plus 'single' quotes"),
            OsString::new(),
            OsString::from("--"),
        ];
        let command = expect_command(fixture.command_with(&fixture.policy(), &injected));
        let arguments = command_arguments(&command);
        assert_eq!(arguments.len(), 16 + injected.len());
        assert_eq!(arguments[14], "--");
        assert_eq!(arguments[15], fixture.executable.as_os_str());
        assert_eq!(&arguments[16..], injected.as_slice());
        assert_eq!(arguments[0], "-c");
        assert_eq!(arguments[1], "windows.sandbox=mxc");
        assert_eq!(arguments[3], "features.network_proxy=false");
    }

    #[test]
    fn environment_contains_only_exact_bootstrap_and_derived_values() {
        let fixture = Fixture::new();
        let command = expect_command(fixture.command());
        let actual: BTreeMap<OsString, Option<OsString>> = command
            .get_envs()
            .map(|(name, value)| (name.to_os_string(), value.map(OsStr::to_os_string)))
            .collect();
        let mut expected = BTreeMap::new();
        for name in BOOTSTRAP_ENVIRONMENT_NAMES {
            if let Some(value) = env::var_os(name) {
                expected.insert(OsString::from(name), Some(value));
            }
        }
        let system_root = PathBuf::from(env::var_os("SystemRoot").expect("SystemRoot"));
        let system32 = system_root
            .join("System32")
            .canonicalize()
            .expect("canonical System32");
        let system32 = PathBuf::from(system32.to_str().unwrap().strip_prefix(r"\\?\").unwrap());
        let search_path = env::join_paths([
            fixture.executable.parent().expect("executable parent"),
            fixture.backend.parent().expect("backend parent"),
            system32.as_path(),
        ])
        .expect("fixture search path");
        expected.insert(OsString::from("PATH"), Some(search_path));
        expected.insert(
            OsString::from("TEMP"),
            Some(fixture.scratch.as_os_str().to_owned()),
        );
        expected.insert(
            OsString::from("TMP"),
            Some(fixture.scratch.as_os_str().to_owned()),
        );
        assert_eq!(actual, expected);
        for forbidden in [
            "CODEX_HOME",
            "NODE_OPTIONS",
            "PYTHONPATH",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "MXC_TEST_SECRET",
            "MXC_TEST_PROFILE",
        ] {
            assert!(!actual.contains_key(OsStr::new(forbidden)));
        }
    }

    #[test]
    fn accepts_standard_extended_drive_prefixes_without_returning_them() {
        fn extended(path: &Path) -> PathBuf {
            PathBuf::from(format!(
                r"\\?\{}",
                path.to_str().expect("Unicode fixture path")
            ))
        }

        let fixture = Fixture::new();
        let mut policy = fixture.policy();
        policy.workspace = extended(&policy.workspace);
        policy.scratch = extended(&policy.scratch);
        policy.read_only_paths = policy.read_only_paths.iter().map(|p| extended(p)).collect();
        policy.denied_paths = policy.denied_paths.iter().map(|p| extended(p)).collect();
        let backend = extended(&fixture.backend);
        let executable = extended(&fixture.executable);
        let command = expect_command(offline_command(&backend, &executable, &[], &policy));
        assert_eq!(command.get_program(), fixture.backend.as_os_str());
        assert_eq!(command.get_current_dir(), Some(fixture.workspace.as_path()));
        let arguments = command_arguments(&command);
        assert_eq!(arguments[15], fixture.executable.as_os_str());
    }

    #[test]
    fn rejects_invalid_profile_ids_before_using_them_as_configuration() {
        let fixture = Fixture::new();
        for invalid in [
            "",
            "rd_mxc_",
            "rd_mxc_0123456789abcdef0123456789abcde",
            "rd_mxc_0123456789abcdef0123456789abcdeff",
            "rd_mxc_0123456789ABCDEF0123456789ABCDEF",
            "RD_MXC_0123456789abcdef0123456789abcdef",
            "rd_mxc_0123456789abcdef0123456789abcdeg",
            "rd_mxc_0123456789abcdef0123456789abcdef.extra",
        ] {
            let mut policy = fixture.policy();
            policy.profile_id = invalid.to_owned();
            assert_error(
                fixture.command_with(&policy, &[]),
                MxcError::InvalidProfileId,
            );
        }
    }

    #[test]
    fn rejects_roots_relative_paths_traversal_and_non_local_namespaces() {
        let fixture = Fixture::new();
        for invalid in [
            PathBuf::from(r"C:\"),
            PathBuf::from("relative\\workspace"),
            fixture.workspace.join("..").join("escape"),
            PathBuf::from(r"\\server\share\workspace"),
            PathBuf::from(r"\\?\UNC\server\share\workspace"),
            PathBuf::from(r"\\?\Volume{00000000-0000-0000-0000-000000000000}\workspace"),
            PathBuf::from(r"\\.\C:\workspace"),
        ] {
            let mut policy = fixture.policy();
            policy.workspace = invalid;
            assert_error(fixture.command_with(&policy, &[]), MxcError::InvalidPath);
        }
    }

    #[test]
    fn relative_backend_and_executable_never_use_path_search() {
        let fixture = Fixture::new();
        assert_error(
            offline_command(
                Path::new("codex.exe"),
                &fixture.executable,
                &[],
                &fixture.policy(),
            ),
            MxcError::InvalidPath,
        );
        assert_error(
            offline_command(
                &fixture.backend,
                Path::new("runtime.exe"),
                &[],
                &fixture.policy(),
            ),
            MxcError::InvalidPath,
        );
    }

    #[test]
    fn rejects_non_unicode_and_control_character_path_names() {
        let fixture = Fixture::new();
        let mut policy = fixture.policy();
        policy.workspace = fixture.workspace.join("bad\u{7}name");
        assert_error(fixture.command_with(&policy, &[]), MxcError::InvalidPath);

        let mut wide: Vec<u16> = fixture.workspace.as_os_str().encode_wide().collect();
        wide.push(u16::from(b'\\'));
        wide.push(0xd800);
        let mut policy = fixture.policy();
        policy.workspace = PathBuf::from(OsString::from_wide(&wide));
        assert_error(fixture.command_with(&policy, &[]), MxcError::InvalidPath);
    }

    #[test]
    fn rejects_reparse_aliases_when_symlink_creation_is_available() {
        let fixture = Fixture::new();
        let target = fixture.workspace.join("alias target");
        let alias = fixture.workspace.join("alias path");
        fs::create_dir(&target).expect("alias target");
        match symlink_dir(&target, &alias) {
            Ok(()) => {
                let mut policy = fixture.policy();
                policy.scratch = alias.clone();
                assert_error(
                    fixture.command_with(&policy, &[]),
                    MxcError::PathNotCanonical,
                );
                let mut policy = fixture.policy();
                policy.read_only_paths = vec![alias];
                assert_error(
                    fixture.command_with(&policy, &[]),
                    MxcError::PathNotCanonical,
                );
            }
            Err(error) if error.raw_os_error() == Some(1314) => {
                eprintln!("reparse alias assertion skipped: symlink privilege is unavailable");
            }
            Err(error) => panic!(
                "fixture symlink creation failed with OS code {:?}",
                error.raw_os_error()
            ),
        }
    }

    #[test]
    fn rejects_scratch_outside_or_equal_to_workspace() {
        let fixture = Fixture::new();
        for scratch in [fixture.outside.clone(), fixture.workspace.clone()] {
            let mut policy = fixture.policy();
            policy.scratch = scratch;
            assert_error(
                fixture.command_with(&policy, &[]),
                MxcError::ScratchOutsideWorkspace,
            );
        }
    }

    #[test]
    fn rejects_protected_paths_outside_or_equal_to_workspace() {
        let fixture = Fixture::new();
        for protected in [fixture.outside.clone(), fixture.workspace.clone()] {
            let mut policy = fixture.policy();
            policy.read_only_paths = vec![protected.clone()];
            assert_error(
                fixture.command_with(&policy, &[]),
                MxcError::ProtectedPathOutsideWorkspace,
            );
            let mut policy = fixture.policy();
            policy.denied_paths = vec![protected];
            assert_error(
                fixture.command_with(&policy, &[]),
                MxcError::ProtectedPathOutsideWorkspace,
            );
        }
    }

    #[test]
    fn rejects_every_scratch_and_protected_overlap_direction() {
        let fixture = Fixture::new();
        let mut policy = fixture.policy();
        policy.read_only_paths = vec![fixture.scratch.clone()];
        assert_error(
            fixture.command_with(&policy, &[]),
            MxcError::ConflictingPaths,
        );

        let nested = fixture.scratch.join("nested denied");
        fs::create_dir(&nested).expect("nested scratch directory");
        let mut policy = fixture.policy();
        policy.denied_paths = vec![nested];
        assert_error(
            fixture.command_with(&policy, &[]),
            MxcError::ConflictingPaths,
        );

        let scratch_parent = fixture.workspace.join("scratch parent");
        let scratch = scratch_parent.join("scratch");
        fs::create_dir_all(&scratch).expect("nested scratch");
        let mut policy = fixture.policy();
        policy.scratch = scratch;
        policy.denied_paths = vec![scratch_parent];
        assert_error(
            fixture.command_with(&policy, &[]),
            MxcError::ConflictingPaths,
        );
    }

    #[test]
    fn rejects_duplicates_and_read_deny_conflicts_instead_of_widening() {
        let fixture = Fixture::new();
        let mut policy = fixture.policy();
        policy.read_only_paths = vec![fixture.read_only.clone(), fixture.read_only.clone()];
        assert_error(
            fixture.command_with(&policy, &[]),
            MxcError::ConflictingPaths,
        );

        let mut policy = fixture.policy();
        policy.denied_paths = vec![fixture.denied.clone(), fixture.denied.clone()];
        assert_error(
            fixture.command_with(&policy, &[]),
            MxcError::ConflictingPaths,
        );

        let mut policy = fixture.policy();
        policy.read_only_paths = vec![fixture.denied.clone()];
        assert_error(
            fixture.command_with(&policy, &[]),
            MxcError::ConflictingPaths,
        );

        let parent = fixture.workspace.join("protected parent");
        let child = parent.join("child");
        fs::create_dir_all(&child).expect("conflicting directories");
        for (read_only, denied) in [(child.clone(), parent.clone()), (parent, child)] {
            let mut policy = fixture.policy();
            policy.read_only_paths = vec![read_only];
            policy.denied_paths = vec![denied];
            assert_error(
                fixture.command_with(&policy, &[]),
                MxcError::ConflictingPaths,
            );
        }
    }

    #[test]
    fn rejects_executable_grants_conflicting_with_protected_scopes() {
        let fixture = Fixture::new();
        let denied_directory = fixture.workspace.join("denied runtime");
        fs::create_dir(&denied_directory).expect("denied runtime directory");
        let executable = denied_directory.join("runner.exe");
        File::create(&executable).expect("protected executable");
        let mut policy = fixture.policy();
        policy.denied_paths = vec![denied_directory];
        assert_error(
            offline_command(&fixture.backend, &executable, &[], &policy),
            MxcError::ConflictingPaths,
        );

        let mut policy = fixture.policy();
        policy.read_only_paths = vec![executable.clone()];
        policy.denied_paths = Vec::new();
        assert_error(
            offline_command(&fixture.backend, &executable, &[], &policy),
            MxcError::ConflictingPaths,
        );
    }

    #[test]
    fn enforces_each_protected_vector_cap_independently() {
        let fixture = Fixture::new();
        let mut policy = fixture.policy();
        policy.read_only_paths = vec![fixture.read_only.clone(); 33];
        assert_error(
            fixture.command_with(&policy, &[]),
            MxcError::TooManyProtectedPaths,
        );
        let mut policy = fixture.policy();
        policy.denied_paths = vec![fixture.denied.clone(); 33];
        assert_error(
            fixture.command_with(&policy, &[]),
            MxcError::TooManyProtectedPaths,
        );
    }

    #[test]
    fn enforces_argument_count_aggregate_size_and_nul_boundaries() {
        let fixture = Fixture::new();
        let policy = fixture.policy();
        assert_error(
            fixture.command_with(&policy, &vec![OsString::new(); 129]),
            MxcError::TooManyArguments,
        );
        assert_error(
            fixture.command_with(&policy, &[OsString::from("x".repeat(65_537))]),
            MxcError::ArgumentsTooLarge,
        );

        let boundary = vec![
            OsString::from("a".repeat(32_768)),
            OsString::from("b".repeat(32_768)),
        ];
        let command = expect_command(fixture.command_with(&policy, &boundary));
        let arguments = command_arguments(&command);
        assert_eq!(&arguments[16..], boundary.as_slice());

        let nul = OsString::from_wide(&[u16::from(b'a'), 0, u16::from(b'b')]);
        assert_error(
            fixture.command_with(&policy, &[nul]),
            MxcError::ArgumentContainsNul,
        );
    }

    #[test]
    fn rejects_wrong_backend_names_and_file_types() {
        let fixture = Fixture::new();
        let wrong_backend = fixture.runtime.join("other.exe");
        File::create(&wrong_backend).expect("wrong backend fixture");
        assert_error(
            offline_command(&wrong_backend, &fixture.executable, &[], &fixture.policy()),
            MxcError::InvalidBackend,
        );
        assert_error(
            offline_command(
                &fixture.workspace,
                &fixture.executable,
                &[],
                &fixture.policy(),
            ),
            MxcError::InvalidPathType,
        );
        assert_error(
            offline_command(&fixture.backend, &fixture.workspace, &[], &fixture.policy()),
            MxcError::InvalidPathType,
        );
    }

    #[test]
    fn errors_do_not_echo_sensitive_path_profile_or_argument_text() {
        fn assert_content_free(error: MxcError, marker: &str) {
            assert!(!format!("{error:?}").contains(marker));
            assert!(!error.to_string().contains(marker));
            assert!(!error.code().contains(marker));
        }

        let fixture = Fixture::new();
        let marker = "sensitive_mxc_marker";
        let mut policy = fixture.policy();
        policy.profile_id = format!("rd_mxc_{marker}");
        let error = match fixture.command_with(&policy, &[]) {
            Err(error) => error,
            Ok(_) => panic!("invalid profile unexpectedly accepted"),
        };
        assert_content_free(error, marker);

        let mut policy = fixture.policy();
        policy.workspace = PathBuf::from(marker);
        let error = match fixture.command_with(&policy, &[]) {
            Err(error) => error,
            Ok(_) => panic!("invalid path unexpectedly accepted"),
        };
        assert_content_free(error, marker);

        let mut wide: Vec<u16> = marker.encode_utf16().collect();
        wide.push(0);
        let error = match fixture.command_with(&fixture.policy(), &[OsString::from_wide(&wide)]) {
            Err(error) => error,
            Ok(_) => panic!("invalid argument unexpectedly accepted"),
        };
        assert_content_free(error, marker);
    }
}

#[cfg(all(test, not(windows)))]
mod unsupported_os_tests {
    use std::{
        ffi::OsString,
        path::{Path, PathBuf},
    };

    use super::{MxcError, MxcOfflinePolicy, offline_command};

    #[test]
    fn non_windows_hosts_return_an_explicit_typed_error() {
        let policy = MxcOfflinePolicy {
            workspace: PathBuf::new(),
            scratch: PathBuf::new(),
            read_only_paths: Vec::new(),
            denied_paths: Vec::new(),
            profile_id: String::new(),
        };
        let result = offline_command(
            Path::new("unused"),
            Path::new("unused"),
            &[OsString::new()],
            &policy,
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("unsupported platform unexpectedly accepted"),
        };
        assert_eq!(error, MxcError::UnsupportedOs);
        assert_eq!(error.code(), "mxc_unsupported_os");
    }
}
