use std::{collections::BTreeMap, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::timeout,
};
use vibemux_plugin_protocol::manifest::{ManifestPluginKind, PluginManifest, SupportedPlatform};
use vibemux_plugin_supervisor::ResolvedPluginLaunch;

use crate::{Error, config::Route};

pub(crate) const SYSTEM_ENVIRONMENT: &[&str] = &["SYSTEMROOT", "WINDIR", "TEMP", "TMP"];

pub(crate) fn environment(route: &Route) -> Result<BTreeMap<String, String>, Error> {
    let mut values = BTreeMap::new();
    for name in SYSTEM_ENVIRONMENT {
        if let Ok(value) = std::env::var(name) {
            values.insert((*name).to_string(), value);
        }
    }
    for name in &route.environment_names {
        let value = std::env::var(name)
            .map_err(|source| Error::with_source("harness_environment_unavailable", source))?;
        values.insert(name.clone(), value);
    }
    Ok(values)
}

pub(crate) struct Process {
    pub input: Option<ChildStdin>,
    pub output: Option<BufReader<ChildStdout>>,
    child: Child,
    ownership: Option<Ownership>,
    stderr_task: Option<JoinHandle<Result<u64, Error>>>,
}

pub(crate) struct Exit {
    pub code: Option<i32>,
    pub success: bool,
    pub forced: bool,
    pub stderr_bytes: u64,
}

impl Process {
    pub async fn spawn(route: &Route, arguments: Vec<String>) -> Result<Self, Error> {
        let mut entry_point = vec!["native".to_string()];
        entry_point.extend(arguments);
        let manifest = PluginManifest {
            schema_version: 1,
            plugin_id: "vibemux.native".to_string(),
            plugin_version: "1.0.0".to_string(),
            kind: ManifestPluginKind::Harness,
            entry_point,
            capabilities: vec![],
            requested_permissions: vec![],
            protocol_major: 1,
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
            supported_platforms: vec![
                SupportedPlatform::Windows,
                SupportedPlatform::Linux,
                SupportedPlatform::Macos,
            ],
        };
        let launch = ResolvedPluginLaunch::new(
            &manifest,
            route.executable.clone(),
            route.working_directory.clone(),
            environment(route)?,
        )
        .map_err(|source| Error::with_source("harness_invalid_launch", source))?;
        #[cfg(windows)]
        if !launch
            .executable
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            return Err(Error::new("harness_native_executable_required"));
        }
        let mut command = Command::new(&launch.executable);
        command
            .args(&launch.arguments)
            .current_dir(&launch.working_directory)
            .env_clear()
            .envs(&launch.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000 | 0x0000_0200);
        #[cfg(unix)]
        command.process_group(0);
        let ownership = Ownership::prepare()?;
        let mut child = command
            .spawn()
            .map_err(|source| Error::with_source("harness_spawn_failed", source))?;
        let ownership = match ownership.attach(&child) {
            Ok(ownership) => ownership,
            Err(error) => {
                let _ = child.start_kill();
                let _ = timeout(Duration::from_secs(2), child.wait()).await;
                return Err(error);
            }
        };
        let input = child
            .stdin
            .take()
            .ok_or_else(|| Error::new("harness_missing_pipe"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| Error::new("harness_missing_pipe"))?;
        let mut diagnostics = child
            .stderr
            .take()
            .ok_or_else(|| Error::new("harness_missing_pipe"))?;
        let stderr_task = tokio::spawn(async move {
            let mut count = 0_u64;
            let mut buffer = [0_u8; 8192];
            loop {
                match diagnostics.read(&mut buffer).await {
                    Ok(0) => return Ok(count),
                    Err(source) => {
                        return Err(Error::with_source("harness_stderr_read_failed", source));
                    }
                    Ok(bytes) => count = count.saturating_add(bytes as u64),
                }
            }
        });
        Ok(Self {
            input: Some(input),
            output: Some(BufReader::new(output)),
            child,
            ownership: Some(ownership),
            stderr_task: Some(stderr_task),
        })
    }

    pub async fn observe_exit(&mut self) -> Result<(), Error> {
        self.child
            .wait()
            .await
            .map_err(|source| Error::with_source("harness_wait_failed", source))?;
        // Descendants may inherit stdout. Release the owned group after the
        // parent exits so these handles cannot keep the final drain open.
        self.ownership.take();
        Ok(())
    }

    pub async fn finish(&mut self, grace: Duration, force: bool) -> Result<Exit, Error> {
        self.input.take();
        let mut forced = force;
        if force {
            self.ownership.take();
            self.child
                .start_kill()
                .map_err(|source| Error::with_source("harness_kill_failed", source))?;
        }
        let status = match timeout(grace, self.child.wait()).await {
            Ok(result) => {
                result.map_err(|source| Error::with_source("harness_wait_failed", source))?
            }
            Err(_) => {
                forced = true;
                self.ownership.take();
                self.child
                    .start_kill()
                    .map_err(|source| Error::with_source("harness_kill_failed", source))?;
                timeout(grace, self.child.wait())
                    .await
                    .map_err(|_| Error::new("harness_reap_timeout"))?
                    .map_err(|source| Error::with_source("harness_wait_failed", source))?
            }
        };
        self.ownership.take();
        let mut stderr_task = self
            .stderr_task
            .take()
            .ok_or_else(|| Error::new("harness_missing_stderr"))?;
        let stderr_bytes = match timeout(grace, &mut stderr_task).await {
            Ok(result) => {
                result.map_err(|source| Error::with_source("harness_stderr_failed", source))??
            }
            Err(_) => {
                stderr_task.abort();
                return Err(Error::new("harness_stderr_timeout"));
            }
        };
        Ok(Exit {
            code: status.code(),
            success: status.success(),
            forced,
            stderr_bytes,
        })
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.ownership.take();
        if let Some(task) = &self.stderr_task {
            task.abort();
        }
    }
}

#[cfg(windows)]
struct Ownership {
    handle: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
impl Ownership {
    fn prepare() -> Result<Self, Error> {
        use std::{
            mem::size_of,
            os::windows::io::{AsRawHandle, FromRawHandle},
            ptr,
        };
        // SAFETY: null attributes/name request an unnamed job; the returned handle is checked below.
        let handle = unsafe { windows::CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(Error::with_source(
                "harness_job_create_failed",
                std::io::Error::last_os_error(),
            ));
        }
        // SAFETY: CreateJobObjectW returned one valid owned handle, transferred exactly once.
        let handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle) };
        let mut limits = windows::Extended::default();
        limits.basic.limit_flags = 0x0000_2000;
        // SAFETY: the owned job is live, and the repr(C) buffer/size match the synchronous Win32 call.
        let result = unsafe {
            windows::SetInformationJobObject(
                handle.as_raw_handle(),
                9,
                &limits as *const _ as *const std::ffi::c_void,
                size_of::<windows::Extended>() as u32,
            )
        };
        if result == 0 {
            return Err(Error::with_source(
                "harness_job_configure_failed",
                std::io::Error::last_os_error(),
            ));
        }
        Ok(Self { handle })
    }

    fn attach(self, child: &Child) -> Result<Self, Error> {
        use std::os::windows::io::AsRawHandle;
        let child_handle = child
            .raw_handle()
            .ok_or_else(|| Error::new("harness_child_exited"))?;
        // SAFETY: both handles remain owned and live throughout this call; no user PID is accepted.
        let assigned =
            unsafe { windows::AssignProcessToJobObject(self.handle.as_raw_handle(), child_handle) };
        if assigned == 0 {
            return Err(Error::with_source(
                "harness_job_assign_failed",
                std::io::Error::last_os_error(),
            ));
        }
        Ok(self)
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    pub struct Basic {
        pub process_user_time: i64,
        pub job_user_time: i64,
        pub limit_flags: u32,
        pub minimum_working_set: usize,
        pub maximum_working_set: usize,
        pub active_process_limit: u32,
        pub affinity: usize,
        pub priority_class: u32,
        pub scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct Counters {
        pub read_operations: u64,
        pub write_operations: u64,
        pub other_operations: u64,
        pub read_bytes: u64,
        pub write_bytes: u64,
        pub other_bytes: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct Extended {
        pub basic: Basic,
        pub io: Counters,
        pub process_memory_limit: usize,
        pub job_memory_limit: usize,
        pub peak_process_memory: usize,
        pub peak_job_memory: usize,
    }

    // SAFETY: these declarations mirror the kernel32 ABI; call sites validate handles and buffers.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        pub fn SetInformationJobObject(
            job: *mut c_void,
            class: u32,
            data: *const c_void,
            size: u32,
        ) -> i32;
        pub fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
    }
}

#[cfg(unix)]
struct Ownership {
    process_group: Option<i32>,
}

#[cfg(unix)]
impl Ownership {
    fn prepare() -> Result<Self, Error> {
        Ok(Self {
            process_group: None,
        })
    }
    fn attach(mut self, child: &Child) -> Result<Self, Error> {
        self.process_group = Some(
            i32::try_from(
                child
                    .id()
                    .ok_or_else(|| Error::new("harness_child_exited"))?,
            )
            .map_err(|_| Error::new("harness_invalid_process"))?,
        );
        Ok(self)
    }
}

#[cfg(unix)]
impl Drop for Ownership {
    fn drop(&mut self) {
        // SAFETY: POSIX kill has this C ABI and receives no borrowed pointers.
        unsafe extern "C" {
            fn kill(process: i32, signal: i32) -> i32;
        }
        if let Some(process_group) = self.process_group {
            // SAFETY: this positive group ID came from our child; negation targets only its group.
            unsafe {
                kill(-process_group, 9);
            }
        }
    }
}
