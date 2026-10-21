//! One background re-run of the read-only probe for the diagnostic TUI.
//!
//! The probe is the same one the TUI runs at start: launcher `--version`
//! checks, a loopback gateway health read, and the local A2A self-test. It
//! sends no prompt. The worker owns its own runtime so the key loop never
//! blocks on it.
use std::{
    io,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread::JoinHandle,
};

pub use vibemux_probe::ProbeConfig;
use vibemux_probe::ProbeReport;

/// Where a re-probe stands.
#[derive(Debug)]
pub enum ReprobePoll {
    Running,
    Done(Box<ProbeReport>),
    /// The worker ended without a report (its runtime could not start), or
    /// the report was already taken.
    Failed,
}

pub struct Reprobe {
    reports: Receiver<ProbeReport>,
    worker: Option<JoinHandle<()>>,
}

impl Reprobe {
    pub fn start(config: ProbeConfig) -> io::Result<Self> {
        Self::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?;
            Some(runtime.block_on(vibemux_probe::run_probe(&config)))
        })
    }

    /// Runs `job` on the worker thread and delivers its report, if any.
    fn spawn(job: impl FnOnce() -> Option<ProbeReport> + Send + 'static) -> io::Result<Self> {
        // Capacity one: the worker sends at most one report and exits.
        let (sender, reports) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("tui_reprobe".into())
            .spawn(move || {
                if let Some(report) = job() {
                    let _ = sender.send(report);
                }
            })?;
        Ok(Self {
            reports,
            worker: Some(worker),
        })
    }

    pub fn poll(&mut self) -> ReprobePoll {
        match self.reports.try_recv() {
            Ok(report) => ReprobePoll::Done(Box::new(report)),
            Err(TryRecvError::Empty) => ReprobePoll::Running,
            Err(TryRecvError::Disconnected) => ReprobePoll::Failed,
        }
    }
}

impl Drop for Reprobe {
    /// Waits for the worker. The probe's own command and network deadlines
    /// bound the wait, and its child processes are reaped, not orphaned.
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use vibemux_probe::{AgentKind, ProbeState};

    #[test]
    fn a_worker_that_ends_without_a_report_counts_as_finished() {
        // A runtime that cannot be built sends nothing; `r` must work again.
        let mut reprobe = Reprobe::spawn(|| None).expect("worker");
        let started = Instant::now();
        loop {
            match reprobe.poll() {
                ReprobePoll::Running => {}
                ReprobePoll::Failed => break,
                ReprobePoll::Done(_) => panic!("no report was produced"),
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "never finished"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_reprobe_delivers_one_report_without_any_launcher() {
        // No PATH entries and a private home: no vendor CLI can be found or
        // run, and nothing listens on port 1.
        let home = tempfile::tempdir().expect("temporary home");
        let config = ProbeConfig {
            home_dir: home.path().to_path_buf(),
            path_entries: Vec::new(),
            cc_switch_port: 1,
            command_timeout: Duration::from_millis(500),
            network_timeout: Duration::from_millis(200),
            telemetry_window_seconds: 60,
            run_a2a_self_test: false,
        };
        let mut reprobe = Reprobe::start(config).expect("worker");
        let started = Instant::now();
        let report = loop {
            if let ReprobePoll::Done(report) = reprobe.poll() {
                break report;
            }
            assert!(started.elapsed() < Duration::from_secs(20), "no report");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(report.agents.len(), AgentKind::all().len());
        assert!(
            report
                .agents
                .iter()
                .all(|agent| agent.launcher_state != ProbeState::Verified)
        );
        assert!(
            !matches!(reprobe.poll(), ReprobePoll::Done(_)),
            "one report per run"
        );
    }
}
