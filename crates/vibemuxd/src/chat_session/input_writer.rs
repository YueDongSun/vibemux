//! One bounded stdin owner, independently supervised from output collection.
use super::{RUNTIME_STDIN_CAPACITY, RuntimeError};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

pub(crate) type Stdin = Box<dyn AsyncWrite + Send + Unpin>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputState {
    Open,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputWriteOutcome {
    /// All bytes and flush completed on the owned pipe, without peer attestation.
    Written,
    /// The writer failed or stopped before it could provide that proof.
    Unknown,
}

pub(crate) struct InputReceipt(oneshot::Receiver<()>);

impl Future for InputReceipt {
    type Output = InputWriteOutcome;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0)
            .poll(context)
            .map(|result| match result {
                Ok(()) => InputWriteOutcome::Written,
                Err(_) => InputWriteOutcome::Unknown,
            })
    }
}

struct QueuedInput {
    bytes: Vec<u8>,
    written: oneshot::Sender<()>,
}

pub(crate) struct InputWriter {
    sender: Option<mpsc::Sender<QueuedInput>>,
    pub status: watch::Receiver<InputState>,
    _status_owner: watch::Sender<InputState>,
    task: JoinHandle<()>,
}

impl InputWriter {
    pub fn start(mut stdin: Stdin) -> Self {
        let (sender, mut receiver) = mpsc::channel::<QueuedInput>(RUNTIME_STDIN_CAPACITY);
        let (status_owner, status) = watch::channel(InputState::Open);
        let task_status = status_owner.clone();
        let task = tokio::spawn(async move {
            while let Some(frame) = receiver.recv().await {
                if stdin.write_all(&frame.bytes).await.is_err() || stdin.flush().await.is_err() {
                    task_status.send_replace(InputState::Failed);
                    return;
                }
                // Observer loss does not cancel accepted input or repeat it.
                let _ = frame.written.send(());
            }
            let result = stdin.shutdown().await;
            task_status.send_replace(if result.is_ok() {
                InputState::Closed
            } else {
                InputState::Failed
            });
        });
        Self {
            sender: Some(sender),
            status,
            _status_owner: status_owner,
            task,
        }
    }
    pub fn send(&self, bytes: &[u8]) -> Result<(), RuntimeError> {
        self.send_tracked(bytes).map(drop)
    }
    /// Queue once and retain an optional observer for actual write/flush proof.
    /// Dropping the receipt does not cancel its already accepted frame.
    pub fn send_tracked(&self, bytes: &[u8]) -> Result<InputReceipt, RuntimeError> {
        let (written, receipt) = oneshot::channel();
        self.sender
            .as_ref()
            .ok_or(RuntimeError::InputFailed)?
            .try_send(QueuedInput {
                bytes: bytes.to_vec(),
                written,
            })
            .map_err(|_| RuntimeError::InputFailed)?;
        Ok(InputReceipt(receipt))
    }
    pub fn close(&mut self) {
        self.sender = None;
    }
    pub async fn task_abort_and_join(&mut self) {
        self.close();
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

impl Drop for InputWriter {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll},
        time::Duration,
    };
    use tokio::{io::AsyncReadExt, sync::oneshot, time::timeout};

    const WAIT: Duration = Duration::from_secs(2);
    const QUIET: Duration = Duration::from_millis(100);

    struct FlushGate {
        bytes: Arc<Mutex<Vec<u8>>>,
        wrote: Option<oneshot::Sender<()>>,
        flush: oneshot::Receiver<()>,
    }

    impl AsyncWrite for FlushGate {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.bytes
                .lock()
                .expect("fixture bytes")
                .extend_from_slice(bytes);
            if let Some(wrote) = self.wrote.take() {
                let _ = wrote.send(());
            }
            Poll::Ready(Ok(bytes.len()))
        }
        fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            std::future::Future::poll(Pin::new(&mut self.flush), context).map(|_| Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_receipt_waits_for_flush_and_does_not_claim_peer_acceptance() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (wrote, written) = oneshot::channel();
        let (release, flush) = oneshot::channel();
        let mut writer = InputWriter::start(Box::new(FlushGate {
            bytes: bytes.clone(),
            wrote: Some(wrote),
            flush,
        }));
        let mut receipt = writer.send_tracked(b"approval frame\n").expect("enqueue");
        timeout(WAIT, written)
            .await
            .expect("write began")
            .expect("write signal");
        assert_eq!(&*bytes.lock().expect("bytes"), b"approval frame\n");
        assert!(
            timeout(QUIET, &mut receipt).await.is_err(),
            "written bytes without flush are not a Written receipt"
        );
        release.send(()).expect("release flush");
        assert_eq!(
            timeout(WAIT, receipt).await.expect("receipt"),
            InputWriteOutcome::Written
        );
        writer.task_abort_and_join().await;
    }

    #[tokio::test]
    async fn dropping_the_receipt_observer_does_not_cancel_accepted_input() {
        let (input, mut peer) = tokio::io::duplex(64);
        let mut writer = InputWriter::start(Box::new(input));
        drop(writer.send_tracked(b"kept input\n").expect("enqueue"));
        let mut bytes = [0; 11];
        timeout(WAIT, peer.read_exact(&mut bytes))
            .await
            .expect("input arrives")
            .expect("read");
        assert_eq!(&bytes, b"kept input\n");
        writer.task_abort_and_join().await;
    }

    #[tokio::test]
    async fn aborting_a_partial_write_retains_unknown_for_active_and_queued_frames() {
        let (input, _peer) = tokio::io::duplex(1);
        let mut writer = InputWriter::start(Box::new(input));
        let mut active = writer.send_tracked(&[b'x'; 1024]).expect("active frame");
        let queued = writer.send_tracked(b"queued").expect("queued frame");
        assert!(timeout(QUIET, &mut active).await.is_err());
        writer.task_abort_and_join().await;
        assert_eq!(
            timeout(WAIT, active).await.expect("active outcome"),
            InputWriteOutcome::Unknown
        );
        assert_eq!(
            timeout(WAIT, queued).await.expect("queued outcome"),
            InputWriteOutcome::Unknown
        );
    }

    #[tokio::test]
    async fn a_broken_pipe_cannot_report_a_written_receipt() {
        let (input, peer) = tokio::io::duplex(1);
        drop(peer);
        let mut writer = InputWriter::start(Box::new(input));
        let receipt = writer
            .send_tracked(b"frame")
            .expect("queue accepts before I/O");
        assert_eq!(
            timeout(WAIT, receipt).await.expect("failed outcome"),
            InputWriteOutcome::Unknown
        );
        assert_eq!(*writer.status.borrow(), InputState::Failed);
        writer.task_abort_and_join().await;
    }
}
