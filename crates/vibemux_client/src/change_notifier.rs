#![forbid(unsafe_code)]
//! GUI-neutral change notification. `FrontendClient` and the Ask workers
//! hand the backend a `ChangeNotifier` instead of an `egui::Context`, so
//! this crate never depends on egui. The GUI wraps its repaint context in
//! a newtype that implements this trait; headless callers and tests use
//! [`NoopNotifier`].

#[cfg(any(test, feature = "test_support"))]
use std::sync::atomic::{AtomicUsize, Ordering};

/// Wakes a frontend when backend state changed. Implementors decide what
/// "wake" means (an egui repaint, a channel send, nothing at all).
pub trait ChangeNotifier: Send + Sync + 'static {
    fn notify(&self);
}

/// Does nothing. For headless use and tests that do not observe wakeups.
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
}
