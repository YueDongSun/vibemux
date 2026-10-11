//! Disposable home, empty launcher path and owned ephemeral loopback producer.

use std::{
    io::{ErrorKind, Write},
    net::TcpListener,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use tokio::{io::AsyncReadExt, process::Command, time::timeout};

const CHILD_DEADLINE: Duration = Duration::from_secs(15);
const OUTPUT_LIMIT: u64 = 256 * 1024;

pub struct CliFixture {
    temp: tempfile::TempDir,
    port: u16,
    stop: Arc<AtomicBool>,
    connections: Arc<AtomicUsize>,
    server: Option<JoinHandle<()>>,
}

impl CliFixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().expect("CLI fixture directory");
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("owned ephemeral loopback fixture");
        let port = listener.local_addr().expect("fixture address").port();
        listener
            .set_nonblocking(true)
            .expect("bounded fixture accept");
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let connections = Arc::new(AtomicUsize::new(0));
        let accepted = Arc::clone(&connections);
        let server = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        accepted.fetch_add(1, Ordering::Release);
                        stream
                            .set_write_timeout(Some(Duration::from_secs(1)))
                            .expect("fixture write deadline");
                        let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            temp,
            port,
            stop,
            connections,
            server: Some(server),
        }
    }

    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::Acquire)
    }

    pub fn run(
        &self,
        executable: &str,
        arguments: &[&str],
        environment: &[(&str, &str)],
    ) -> (i32, String, String) {
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .current_dir(self.temp.path())
            .env_clear();
        for key in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "TEMP",
            "TMP",
            "TMPDIR",
            "PATH",
        ] {
            command.env(key, self.temp.path());
        }
        command.env(
            vibemux_frontend::probe_env::FRONTEND_PROBE_PORT_ENV,
            self.port.to_string(),
        );
        command.envs(environment.iter().copied());
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fixture runtime")
            .block_on(async {
                let mut child = command.spawn().expect("CLI fixture child");
                let mut stdout = child.stdout.take().expect("stdout").take(OUTPUT_LIMIT + 1);
                let mut stderr = child.stderr.take().expect("stderr").take(OUTPUT_LIMIT + 1);
                let output = tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    stdout.read_to_end(&mut bytes).await.expect("stdout read");
                    bytes
                });
                let errors = tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    stderr.read_to_end(&mut bytes).await.expect("stderr read");
                    bytes
                });
                let status = match timeout(CHILD_DEADLINE, child.wait()).await {
                    Ok(status) => status.expect("CLI fixture exit"),
                    Err(_) => {
                        child.kill().await.expect("stop timed out fixture");
                        let _ = output.await;
                        let _ = errors.await;
                        panic!("CLI fixture exceeded its deadline");
                    }
                };
                let output = output.await.expect("stdout join");
                let errors = errors.await.expect("stderr join");
                assert!(
                    output.len() <= OUTPUT_LIMIT as usize && errors.len() <= OUTPUT_LIMIT as usize
                );
                (
                    status.code().unwrap_or(-1),
                    String::from_utf8(output).expect("UTF-8 stdout"),
                    String::from_utf8(errors).expect("UTF-8 stderr"),
                )
            })
    }
}

impl Drop for CliFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(server) = self.server.take() {
            server.join().expect("fixture server join");
        }
    }
}
