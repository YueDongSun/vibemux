//! The transport boundary distinguishes a read failure from a verified EOF.
use super::*;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

struct FaultAfterData(bool);
impl AsyncRead for FaultAfterData {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let reader = self.get_mut();
        if !reader.0 {
            reader.0 = true;
            buffer.put_slice(b"fixed diagnostic");
            Poll::Ready(Ok(()))
        } else {
            Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
        }
    }
}

#[tokio::test]
async fn a_stderr_read_fault_is_not_verified_eof() {
    let bytes = Arc::new(AtomicU64::new(0));
    let flags = Arc::new(AtomicU32::new(0));
    let mut task = tokio::spawn(drain_stderr(FaultAfterData(false), bytes.clone(), flags));
    let eof = join_stderr_drain(&mut task).await;
    assert!(!eof, "read failure cannot authorize quiescence");
    assert_eq!(
        bytes.load(Ordering::Relaxed),
        16,
        "bytes read before the fault remain counted"
    );
}

#[tokio::test]
async fn normal_stderr_eof_preserves_bounded_diagnostic_flags() {
    let bytes = Arc::new(AtomicU64::new(0));
    let flags = Arc::new(AtomicU32::new(0));
    let input = std::io::Cursor::new(b"access denied by sandbox".to_vec());
    assert!(drain_stderr(input, bytes.clone(), flags.clone()).await);
    assert_eq!(bytes.load(Ordering::Relaxed), 24);
    assert_eq!(
        flags.load(Ordering::Relaxed),
        STDERR_ACCESS_DENIED | STDERR_SANDBOX
    );
}
