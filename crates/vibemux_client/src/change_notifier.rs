#![forbid(unsafe_code)]
//! GUI-neutral change notification. `FrontendClient` and the Ask workers
//! hand the backend a `ChangeNotifier` instead of an `egui::Context`, so
//! this crate never depends on egui. The GUI wraps its repaint context in
//! a newtype that implements this trait; headless callers and tests use
//! [`NoopNotifier`].

#[cfg(any(test, feature = "test_support"))]
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

/// How often [`CountingNotifier::wait_for_count`] rereads the count.
#[cfg(any(test, feature = "test_support"))]
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Wakes a frontend when backend state changed. Implementors decide what
/// "wake" means (an egui repaint, a channel send, nothing at all).
///
/// Calling contract: `notify()` is called from worker threads, possibly from
/// inside a Tokio runtime, so it must be cheap and non-blocking — it must not
/// call `blocking_send` or `Handle::block_on`. It may also be called while the
/// owner is dropping its client (`FrontendClient::drop` joins the worker that
/// calls `notify()`), so an implementation must not take any lock the owner
/// holds across that drop.
pub trait ChangeNotifier: Send + Sync + 'static {
    fn notify(&self);
}

/// Does nothing. For headless use and tests that do not observe wakeups.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopNotifier;

impl ChangeNotifier for NoopNotifier {
    fn notify(&self) {}
}

/// Counts every `notify()` call. Test-only: production builds never pull
/// in this scaffolding unless the `test_support` feature is explicitly on.
#[cfg(any(test, feature = "test_support"))]
#[derive(Default)]
pub struct CountingNotifier {
    count: AtomicUsize,
}

#[cfg(any(test, feature = "test_support"))]
impl CountingNotifier {
    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    /// Blocks until at least `minimum` notifications arrived or `timeout`
    /// elapsed, and returns whether the count was reached. Workers publish
    /// their result before calling `notify()`, so a test that has just seen
    /// the result must wait for the notification instead of reading the
    /// count once.
    pub fn wait_for_count(&self, minimum: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.count() < minimum {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(WAIT_POLL_INTERVAL);
        }
        true
    }
}

#[cfg(any(test, feature = "test_support"))]
impl ChangeNotifier for CountingNotifier {
    fn notify(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_notifier_counts_each_notify() {
        let notifier = CountingNotifier::default();
        notifier.notify();
        notifier.notify();
        assert_eq!(notifier.count(), 2);
    }

    #[test]
    fn wait_for_count_reports_whether_the_count_was_reached() {
        let notifier = CountingNotifier::default();
        assert!(!notifier.wait_for_count(1, Duration::from_millis(20)));
        notifier.notify();
        assert!(notifier.wait_for_count(1, Duration::ZERO));
    }
}
