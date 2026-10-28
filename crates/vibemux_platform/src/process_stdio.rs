//! Preconnected, read-only process-output pipes built through safe APIs (ADR 047).
#![forbid(unsafe_code)]

use std::{
    fs::{File, OpenOptions},
    future::Future,
    io,
    os::windows::fs::OpenOptionsExt,
    process::Stdio,
};
use tokio::{
    io::AsyncRead,
    net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions},
    process::{Child, Command},
};

const PIPE_BUFFER_BYTES: u32 = 4096;
const PIPE_NAMESPACE: &str = r"\\.\pipe\vibemux_stdio_";
pub type AsyncChildReader = Box<dyn AsyncRead + Unpin + Send>;
pub struct ChildOutput {
    pub stdout: AsyncChildReader,
    pub stderr: AsyncChildReader,
}
pub struct SpawnedWithOutput {
    pub child: Child,
    pub output: ChildOutput,
}

fn pipe_name() -> String {
    format!("{PIPE_NAMESPACE}{}", uuid::Uuid::new_v4().simple())
}
fn create_server(name: &str) -> io::Result<NamedPipeServer> {
    ServerOptions::new()
        .access_inbound(true)
        .access_outbound(false)
        .pipe_mode(PipeMode::Byte)
        .first_pipe_instance(true)
        .max_instances(1)
        .reject_remote_clients(true)
        .in_buffer_size(PIPE_BUFFER_BYTES)
        .out_buffer_size(PIPE_BUFFER_BYTES)
        .create(name)
}
fn open_writer(name: &str) -> io::Result<File> {
    OpenOptions::new().write(true).share_mode(0).open(name)
}
async fn create_pair_at(name: &str) -> io::Result<(NamedPipeServer, File)> {
    let reader = create_server(name)?;
    // There is no accept/reconnect loop. A preempting connection makes this
    // owner open fail before any child can start.
    let writer = open_writer(name)?;
    reader.connect().await?;
    Ok((reader, writer))
}
async fn create_output_pair() -> io::Result<(NamedPipeServer, File)> {
    create_pair_at(&pipe_name()).await
}

/// Connects both owned pipe pairs before spawning. Must run inside an I/O-enabled
/// Tokio runtime. The caller owns containment/release of its trusted trampoline.
/// The server is inbound-only: clients cannot read captured output. Connections
/// are local, single-instance and permanent; neither names nor handles are caller inputs.
pub async fn spawn_with_async_output(command: &mut Command) -> io::Result<SpawnedWithOutput> {
    spawn_with_pipe_factory(command, create_output_pair).await
}

async fn spawn_with_pipe_factory<F, P>(
    command: &mut Command,
    mut create_pair: F,
) -> io::Result<SpawnedWithOutput>
where
    F: FnMut() -> P,
    P: Future<Output = io::Result<(NamedPipeServer, File)>>,
{
    let (stdout, stdout_writer) = create_pair().await?;
    let (stderr, stderr_writer) = create_pair().await?;
    command
        .stdout(Stdio::from(stdout_writer))
        .stderr(Stdio::from(stderr_writer))
        .kill_on_drop(true);
    let spawned = command.spawn();
    // Command is reusable and retains its configured writer handles. Close those
    // copies on BOTH spawn outcomes before returning or propagating an error.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let child = spawned?;
    Ok(SpawnedWithOutput {
        child,
        output: ChildOutput {
            stdout: Box::new(stdout),
            stderr: Box::new(stderr),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[tokio::test]
    async fn direction_instance_and_preconnection_are_closed() {
        let name = pipe_name();
        let reader = create_server(&name).unwrap();
        assert!(create_server(&name).is_err());
        assert!(OpenOptions::new().read(true).open(&name).is_err());
        let earlier_writer = open_writer(&name).unwrap();
        reader.connect().await.unwrap();
        assert!(
            open_writer(&name).is_err(),
            "a preempting connection must not be adopted"
        );
        drop((earlier_writer, reader));
    }

    async fn endpoint_gone(name: &str) -> bool {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if open_writer(name).is_err_and(|error| error.raw_os_error() == Some(2)) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn second_pair_failure_and_spawn_failure_release_all_pipe_owners() {
        for fail_pair in [true, false] {
            let names = Arc::new(Mutex::new(Vec::new()));
            let mut count = 0;
            let directory = tempfile::TempDir::new().unwrap();
            let mut command = Command::new(directory.path().join("absent_executable.exe"));
            let result = spawn_with_pipe_factory(&mut command, || {
                count += 1;
                let fail = fail_pair && count == 2;
                let name = pipe_name();
                names.lock().unwrap().push(name.clone());
                async move {
                    if fail {
                        Err(io::Error::other("fixture second-pair failure"))
                    } else {
                        create_pair_at(&name).await
                    }
                }
            })
            .await;
            assert!(result.is_err());
            let names = names.lock().unwrap().clone();
            for name in names {
                assert!(endpoint_gone(&name).await);
            }
            // Keep Command alive until after the EOF/ownership checks above.
            drop(command);
        }
    }
}
