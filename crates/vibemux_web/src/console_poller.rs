//! Background reader of the ADR 042 console sections for the web API.
//!
//! It calls only content-free read operations: health, plugin status, the
//! dispatch route catalog, the DSH session status, workflow slots, and the
//! decision list. It never reads a prompt, transcript, or output page, never
//! changes state, and never starts or recovers a daemon.

use std::{sync::Arc, time::Duration};

use tokio::sync::{RwLock, watch};
use vibemuxd::{
    control::{ControlClient, ControlError, DecisionListQuery},
    process::DaemonPaths,
};

use crate::{
    console_model::{
        ConsoleConnection, ConsoleDecision, ConsoleList, ConsoleSection, ConsoleSnapshot,
        MAX_CONSOLE_DECISIONS, merge_decisions, project_decision, project_health, project_plugins,
        project_routes, project_slots, unavailable,
    },
    control_bridge::{
        DEFAULT_POLL_INTERVAL, descriptor_fingerprint, load_validated_client, now_rfc3339,
        project_dsh_status,
    },
};

pub type ConsoleState = Arc<RwLock<ConsoleSnapshot>>;

/// Health is read first and bounds how long a crashed daemon (a descriptor
/// with nothing listening) can keep the sections on their last observation.
pub const HEALTH_DEADLINE: Duration = Duration::from_secs(2);
pub const MAX_DECISION_PAGES_PER_POLL: usize = 4;
pub const DECISION_PAGE_LIMIT: usize = 50;
const REFRESH_DEADLINE: Duration = Duration::from_secs(30);

pub struct ConsolePoller {
    paths: DaemonPaths,
    poll_interval: Duration,
    /// Descriptor fingerprint of the daemon the kept decisions came from.
    generation: Option<String>,
    /// Newest first, at most `MAX_CONSOLE_DECISIONS`.
    decisions: Vec<ConsoleDecision>,
    /// Whether older decisions of this daemon were cut from `decisions`.
    decisions_cut: bool,
}

impl ConsolePoller {
    #[must_use]
    pub fn new(paths: DaemonPaths, poll_interval: Duration) -> Self {
        Self {
            paths,
            poll_interval: if poll_interval.is_zero() {
                DEFAULT_POLL_INTERVAL
            } else {
                poll_interval
            },
            generation: None,
            decisions: Vec::new(),
            decisions_cut: false,
        }
    }

    /// One bounded poll. A failure to reach the daemon makes every section
    /// unavailable rather than keeping an earlier observation.
    pub async fn refresh(&mut self) -> ConsoleSnapshot {
        match tokio::time::timeout(REFRESH_DEADLINE, self.refresh_inner()).await {
            Ok(snapshot) => snapshot,
            Err(_) => ConsoleSnapshot::not_running("bridge_deadline_exceeded", Some(now_rfc3339())),
        }
    }

    pub async fn run(mut self, console_state: ConsoleState, mut stop: watch::Receiver<bool>) {
        let mut interval = tokio::time::interval(self.poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() { break; }
                }
                _ = interval.tick() => {
                    let snapshot = tokio::select! {
                        biased;
                        changed = stop.changed() => {
                            if changed.is_err() || *stop.borrow() { break; }
                            continue;
                        }
                        snapshot = self.refresh() => snapshot,
                    };
                    *console_state.write().await = snapshot;
                }
            }
        }
    }

    async fn refresh_inner(&mut self) -> ConsoleSnapshot {
        let paths = self.paths.clone();
        let connected = tokio::task::spawn_blocking(move || {
            if !paths.descriptor_path().exists() {
                return Err("daemon_not_running");
            }
            load_validated_client(&paths).map_err(|error| error.code())
        })
        .await;
        let (client, generation) = match connected {
            Ok(Ok(connected)) => connected,
            Ok(Err(code)) => return ConsoleSnapshot::not_running(code, Some(now_rfc3339())),
            Err(_) => {
                return ConsoleSnapshot::not_running("daemon_unavailable", Some(now_rfc3339()));
            }
        };
        if self.generation.as_deref() != Some(generation.as_str()) {
            // Decisions of a replaced daemon are not decisions of this one.
            self.decisions.clear();
            self.decisions_cut = false;
            self.generation = Some(generation.clone());
        }
        let health = match client.health_with_deadline(HEALTH_DEADLINE).await {
            Ok(health) => project_health(&health),
            Err(error) => {
                return ConsoleSnapshot::not_running(error.code(), Some(now_rfc3339()));
            }
        };
        let plugins = section(
            client
                .plugin_status()
                .await
                .map(|rows| project_plugins(&rows)),
        );
        let routes = section(
            client
                .harness_dispatch_catalog()
                .await
                .map(|rows| project_routes(&rows)),
        );
        let dsh = section(client.dsh_session_status().await.map(project_dsh_status));
        let slots = section(
            client
                .workflow_slots()
                .await
                .map(|value| project_slots(&value)),
        );
        let decisions = match read_decisions(&client, &self.decisions).await {
            Ok((read, more_pending)) => {
                self.decisions_cut |=
                    merge_decisions(&mut self.decisions, read, MAX_CONSOLE_DECISIONS);
                ConsoleSection::Loaded {
                    data: ConsoleList {
                        rows: self.decisions.clone(),
                        truncated: self.decisions_cut || more_pending,
                    },
                }
            }
            Err(error) => unavailable(error.code()),
        };
        let descriptor_path = self.paths.descriptor_path().to_owned();
        let unchanged =
            tokio::task::spawn_blocking(move || descriptor_fingerprint(&descriptor_path))
                .await
                .ok()
                .and_then(Result::ok)
                .is_some_and(|observed| observed == generation);
        if !unchanged {
            // A restart during the poll: the reads may mix two daemons.
            self.decisions.clear();
            self.decisions_cut = false;
            self.generation = None;
            return ConsoleSnapshot::not_running("daemon_generation_changed", Some(now_rfc3339()));
        }
        ConsoleSnapshot {
            observed_at: Some(now_rfc3339()),
            connection: ConsoleConnection::Connected,
            health: ConsoleSection::Loaded { data: health },
            plugins,
            routes,
            dsh,
            slots,
            decisions,
        }
    }
}

fn section<T>(result: Result<T, ControlError>) -> ConsoleSection<T> {
    match result {
        Ok(data) => ConsoleSection::Loaded { data },
        Err(error) => unavailable(error.code()),
    }
}

/// Re-reads from the oldest kept decision, because a kept decision can be
/// consumed after it was recorded, then continues forward. Bounded to
/// `MAX_DECISION_PAGES_PER_POLL` pages; a longer history catches up over the
/// next polls, since the oldest kept decision moves forward each time.
/// Returns the rows read and whether newer pages are still pending.
async fn read_decisions(
    client: &ControlClient,
    kept: &[ConsoleDecision],
) -> Result<(Vec<ConsoleDecision>, bool), ControlError> {
    let mut after_sequence = kept
        .iter()
        .map(|row| row.created_sequence)
        .min()
        .map_or(0, |oldest| oldest.saturating_sub(1));
    let mut read = Vec::new();
    for _ in 0..MAX_DECISION_PAGES_PER_POLL {
        let page = client
            .decision_list(DecisionListQuery {
                after_sequence,
                limit: DECISION_PAGE_LIMIT,
            })
            .await?;
        read.extend(page.decisions.iter().map(project_decision));
        match page.next_after_sequence {
            Some(next) => after_sequence = next,
            None => return Ok((read, false)),
        }
    }
    Ok((read, true))
}
