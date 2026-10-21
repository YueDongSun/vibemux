//! One background re-run of the read-only probe for the diagnostic TUI.
//!
//! The probe is the same one the TUI runs at start: launcher `--version`
//! checks, a loopback gateway health read, and the local A2A self-test. It
//! sends no prompt. The worker owns its own runtime so the key loop never
//! blocks on it.
use std::{
    io,
    sync::mpsc::{self, Receiver},
    thread::JoinHandle,
};

pub use vibemux_probe::ProbeConfig;
use vibemux_probe::ProbeReport;

pub struct Reprobe {
    reports: Receiver<ProbeReport>,
    worker: Option<JoinHandle<()>>,
}

impl Reprobe {
    pub fn start(config: ProbeConfig) -> io::Result<Self> {
        // Capacity one: the worker sends exactly one report and exits.
        let (sender, reports) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("tui_reprobe".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                let report = runtime.block_on(vibemux_probe::run_probe(&config));
                let _ = sender.send(report);
            })?;
        Ok(Self {
            reports,
            worker: Some(worker),
        })
    }

    /// The report, once it has arrived. `None` while the probe runs, and
    /// after the report was taken.
    pub fn try_take(&mut self) -> Option<ProbeReport> {
        self.reports.try_recv().ok()
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
            if let Some(report) = reprobe.try_take() {
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
        assert!(reprobe.try_take().is_none(), "one report per run");
    }
}
