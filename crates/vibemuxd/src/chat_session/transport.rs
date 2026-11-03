//! NativeProcess is the only production process owner. Tests inject owned pipes.
use super::{RuntimeCleanup, input_writer::Stdin};
use crate::harness_dispatch::native_process::NativeProcess;
use async_trait::async_trait;
use tokio::{io::AsyncReadExt, time::Instant};

#[async_trait]
pub(crate) trait RuntimeTransport: Send {
    fn take_stdin(&mut self) -> Option<Stdin>;
    fn root_exited(&mut self) -> Result<bool, super::RuntimeError> {
        Ok(false)
    }
    async fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize>;
    async fn finish(&mut self, force: bool, deadline: Instant) -> RuntimeCleanup;
}

pub(crate) struct NativeTransport(pub NativeProcess);

#[async_trait]
impl RuntimeTransport for NativeTransport {
    fn take_stdin(&mut self) -> Option<Stdin> {
        self.0.stdin.take().map(|stdin| Box::new(stdin) as Stdin)
    }
    fn root_exited(&mut self) -> Result<bool, super::RuntimeError> {
        self.0
            .root_exited()
            .map_err(|_| super::RuntimeError::Stopped)
    }
    async fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0.stdout.read(buffer).await
    }
    async fn finish(&mut self, force: bool, deadline: Instant) -> RuntimeCleanup {
        let evidence = self.0.finish_session(force, deadline).await;
        RuntimeCleanup {
            exit: evidence.exit,
            root_reaped: evidence.root_reaped,
            tree_empty: evidence.tree_empty,
            cleanup_verified: evidence.cleanup_verified,
            stderr_bytes: evidence.stderr_bytes,
        }
    }
}
