use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutPhase {
    Startup,
    Walkthrough,
}

enum Signal {
    Ready,
    Done,
}

pub struct Watchdog {
    sender: SyncSender<Signal>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    pub fn start(
        startup: Duration,
        walkthrough: Duration,
        on_timeout: impl FnOnce(TimeoutPhase) + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(2);
        let thread = thread::Builder::new()
            .name("native-ui-watchdog".to_owned())
            .spawn(move || {
                if let Some(phase) = wait(&receiver, startup, walkthrough) {
                    on_timeout(phase);
                }
            })?;
        Ok(Self {
            sender,
            thread: Some(thread),
        })
    }

    pub fn ready_signal(&self) -> impl FnOnce() + Send + 'static {
        let sender = self.sender.clone();
        move || {
            let _ = sender.try_send(Signal::Ready);
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.sender.try_send(Signal::Done);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn wait(
    receiver: &Receiver<Signal>,
    startup: Duration,
    walkthrough: Duration,
) -> Option<TimeoutPhase> {
    match receiver.recv_timeout(startup) {
        Ok(Signal::Done) | Err(RecvTimeoutError::Disconnected) => None,
        Err(RecvTimeoutError::Timeout) => Some(TimeoutPhase::Startup),
        Ok(Signal::Ready) => match receiver.recv_timeout(walkthrough) {
            Ok(_) | Err(RecvTimeoutError::Disconnected) => None,
            Err(RecvTimeoutError::Timeout) => Some(TimeoutPhase::Walkthrough),
        },
    }
}

pub struct ExitRequest<F: FnOnce(i32)> {
    request: Option<F>,
    code: i32,
}

impl<F: FnOnce(i32)> ExitRequest<F> {
    pub fn new(request: F) -> Self {
        Self {
            request: Some(request),
            code: 1,
        }
    }

    pub fn code(&mut self, code: i32) {
        self.code = code;
    }
}

impl<F: FnOnce(i32)> Drop for ExitRequest<F> {
    fn drop(&mut self) {
        if let Some(request) = self.request.take() {
            request(self.code);
        }
    }
}

pub fn write_status(writer: &mut impl Write, code: i32) -> io::Result<()> {
    writeln!(
        writer,
        "NATIVE_UI_SMOKE {}",
        if code == 0 { "PASS" } else { "FAIL" }
    )?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[test]
    fn startup_has_a_deadline_before_any_window_exists() {
        let (_sender, receiver) = mpsc::sync_channel(2);
        assert_eq!(
            wait(&receiver, Duration::ZERO, Duration::ZERO),
            Some(TimeoutPhase::Startup)
        );
    }

    #[test]
    fn a_ready_window_still_has_a_walkthrough_deadline() {
        let (sender, receiver) = mpsc::sync_channel(2);
        sender.send(Signal::Ready).expect("ready");
        assert_eq!(
            wait(&receiver, Duration::ZERO, Duration::ZERO),
            Some(TimeoutPhase::Walkthrough)
        );
    }

    #[test]
    fn normal_completion_disarms_both_deadlines() {
        let (sender, receiver) = mpsc::sync_channel(2);
        sender.send(Signal::Ready).expect("ready");
        sender.send(Signal::Done).expect("done");
        assert_eq!(wait(&receiver, Duration::ZERO, Duration::ZERO), None);
    }

    #[test]
    fn an_early_return_disarms_startup_and_joins_the_watchdog() {
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = std::sync::Arc::clone(&fired);
        let watchdog = Watchdog::start(
            Duration::from_secs(10),
            Duration::from_secs(10),
            move |_| {
                observed.store(true, std::sync::atomic::Ordering::Release);
            },
        )
        .expect("watchdog");
        drop(watchdog);
        assert!(!fired.load(std::sync::atomic::Ordering::Acquire));
    }

    struct ClosedStdout;
    impl Write for ClosedStdout {
        fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed stdout"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed stdout"))
        }
    }

    #[test]
    fn closed_stdout_does_not_panic_or_prevent_the_exit_request() {
        let requested = Cell::new(None);
        {
            let mut exit = ExitRequest::new(|code| requested.set(Some(code)));
            exit.code(0);
            assert_eq!(
                write_status(&mut ClosedStdout, 0)
                    .expect_err("broken pipe")
                    .kind(),
                io::ErrorKind::BrokenPipe
            );
        }
        assert_eq!(requested.get(), Some(0));
    }

    #[test]
    fn an_unexpected_worker_panic_requests_failure_shutdown() {
        let requested = Cell::new(None);
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                let _exit = ExitRequest::new(|code| requested.set(Some(code)));
                panic!("synthetic worker failure");
            }))
            .is_err()
        );
        assert_eq!(requested.get(), Some(1));
    }
}
