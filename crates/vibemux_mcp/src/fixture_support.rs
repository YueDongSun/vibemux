//! Explicit test-only process behaviors; absent from default builds and MCP CLI.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const GATE_FALLBACK: Duration = Duration::from_secs(2);
const REAP_GATE_FALLBACK: Duration = Duration::from_secs(4);
const GATE_POLL_INTERVAL: Duration = Duration::from_millis(2);

#[derive(Clone, Default)]
pub struct LaunchGate {
    entered: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
}

impl LaunchGate {
    pub fn entered(&self) -> bool {
        self.entered.load(Ordering::Acquire)
    }

    pub fn release(&self) {
        self.released.store(true, Ordering::Release);
    }

    pub(crate) fn wait(&self) {
        self.entered.store(true, Ordering::Release);
        let deadline = Instant::now() + GATE_FALLBACK;
        while !self.released.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(GATE_POLL_INTERVAL);
        }
    }

    pub(crate) async fn wait_before_reap(&self) {
        self.entered.store(true, Ordering::Release);
        let deadline = tokio::time::Instant::now() + REAP_GATE_FALLBACK;
        while !self.released.load(Ordering::Acquire) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(GATE_POLL_INTERVAL).await;
        }
    }
}

#[derive(Clone, Default)]
pub struct LaunchControl {
    pub before_spawn: Option<LaunchGate>,
    pub after_containment: Option<LaunchGate>,
    pub before_reap: Option<LaunchGate>,
    pub terminal_observed: Arc<AtomicBool>,
    pub fail_cleanup: bool,
}

#[derive(Clone, Copy)]
pub enum FixtureMode {
    Healthy,
    Hang,
    Crash,
    Malformed,
    NonObjectResult,
    Oversized,
    StaleGeneration,
    HealthyThenHang,
    IdleCrash,
    Descendant,
    NoOutputPipe,
    RejectContainment,
}

impl FixtureMode {
    pub(crate) const fn argument(self) -> &'static str {
        match self {
            Self::Healthy | Self::NoOutputPipe | Self::RejectContainment => "healthy",
            Self::Hang => "hang",
            Self::Crash => "crash",
            Self::Malformed => "malformed",
            Self::NonObjectResult => "non_object_result",
            Self::Oversized => "oversized",
            Self::StaleGeneration => "stale_generation",
            Self::HealthyThenHang => "healthy_then_hang",
            Self::IdleCrash => "idle_crash",
            Self::Descendant => "descendant_owner",
        }
    }
}
