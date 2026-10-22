#![forbid(unsafe_code)]
//! The window's Ask state: what the folder's daemon can do, this window's
//! Asks, the confirmation, the one-click setup, and the requests waiting for
//! the live adapter. At most one Ask runs at a time, matching the daemon's
//! one dispatch per project.

use vibemux_harness::AgentKind;

use crate::ask::{AskEnd, AskPhase, AskUpdate, SetupOutcome};

use super::ask_view::{AskAvailability, AskEntry, ConfirmState, MAX_ASKS, SetupJob};

#[derive(Debug, Default)]
pub struct AskState {
    pub availability: AskAvailability,
    /// Oldest first, at most [`MAX_ASKS`].
    pub asks: Vec<AskEntry>,
    pub confirm: ConfirmState,
    pub setup: SetupJob,
    launch: Option<(AgentKind, String)>,
    cancel: bool,
    setup_request: bool,
    recheck: bool,
}

impl AskState {
    /// Whether an Ask is waiting to start, running, or waiting for consent.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.launch.is_some() || self.confirm.open || self.asks.iter().any(AskEntry::running)
    }

    /// Adds an Ask of the harness named `harness` and queues it for the live
    /// adapter. Hands the prompt back when the name is no harness or another
    /// Ask is in flight.
    pub fn launch(&mut self, harness: String, prompt: String) -> Result<(), String> {
        let Some(kind) = AgentKind::all()
            .into_iter()
            .find(|kind| kind.display_name() == harness)
        else {
            return Err(prompt);
        };
        if self.launch.is_some() || self.asks.iter().any(AskEntry::running) {
            return Err(prompt);
        }
        self.asks.push(AskEntry::submitted(harness, prompt.clone()));
        let excess = self.asks.len().saturating_sub(MAX_ASKS);
        self.asks.drain(..excess);
        self.launch = Some((kind, prompt));
        Ok(())
    }

    /// The Ask to start, once.
    pub fn take_launch(&mut self) -> Option<(AgentKind, String)> {
        self.launch.take()
    }

    /// Folds a worker update into the running Ask.
    pub fn apply_update(&mut self, update: AskUpdate) {
        if let Some(entry) = self.running_entry() {
            entry.apply(update);
        }
    }

    /// Ends the running Ask with `code` when its worker cannot report.
    pub fn fail_running(&mut self, code: &str) {
        if let Some(entry) = self.running_entry() {
            entry.phase = AskPhase::Finished;
            entry.end = Some(AskEnd::Failed(code.to_string()));
        }
    }

    fn running_entry(&mut self) -> Option<&mut AskEntry> {
        self.asks.iter_mut().rev().find(|entry| entry.running())
    }

    pub fn request_cancel(&mut self) {
        self.cancel = self.asks.iter().any(AskEntry::running);
    }

    /// Whether the user asked to cancel the running Ask, once.
    pub fn take_cancel(&mut self) -> bool {
        std::mem::take(&mut self.cancel)
    }

    /// Queues the one-click setup unless it already runs.
    pub fn request_setup(&mut self) {
        if self.setup != SetupJob::Running {
            self.setup = SetupJob::Running;
            self.setup_request = true;
        }
    }

    pub fn take_setup_request(&mut self) -> bool {
        std::mem::take(&mut self.setup_request)
    }

    pub fn finish_setup(&mut self, outcome: SetupOutcome) {
        self.setup = SetupJob::Finished(outcome);
    }

    pub fn dismiss_setup(&mut self) {
        if self.setup != SetupJob::Running {
            self.setup = SetupJob::Idle;
        }
    }

    /// Asks the live adapter to list the daemon's routes again.
    pub fn request_recheck(&mut self) {
        self.recheck = true;
    }

    pub fn take_recheck(&mut self) -> bool {
        std::mem::take(&mut self.recheck)
    }

    pub fn set_availability(&mut self, availability: AskAvailability) {
        self.availability = availability;
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn update(reply: &str, end: Option<AskEnd>) -> AskUpdate {
        AskUpdate {
            request_id: Uuid::nil(),
            phase: if end.is_some() {
                AskPhase::Finished
            } else {
                AskPhase::Running
            },
            reply: reply.to_string(),
            truncated: false,
            end,
        }
    }

    #[test]
    fn one_ask_runs_at_a_time_and_unknown_harnesses_hand_the_prompt_back() {
        let mut state = AskState::default();
        assert_eq!(
            state.launch("Nobody".into(), "Hi".into()),
            Err("Hi".to_string())
        );
        assert_eq!(state.launch("Codex".into(), "First".into()), Ok(()));
        assert!(state.busy());
        assert_eq!(
            state.launch("Claude".into(), "Second".into()),
            Err("Second".to_string())
        );
        assert_eq!(
            state.take_launch(),
            Some((AgentKind::Codex, "First".to_string()))
        );
        assert!(state.busy(), "the started Ask still runs");
        state.apply_update(update("Done", Some(AskEnd::Completed)));
        assert!(!state.busy());
        assert_eq!(state.asks[0].reply, "Done");
        assert_eq!(state.launch("Claude".into(), "Second".into()), Ok(()));
    }

    #[test]
    fn the_transcript_keeps_the_newest_asks() {
        let mut state = AskState::default();
        for index in 0..MAX_ASKS + 3 {
            state
                .launch("Codex".into(), format!("Prompt {index}"))
                .expect("no Ask runs");
            state.take_launch();
            state.apply_update(update("", Some(AskEnd::Cancelled)));
        }
        assert_eq!(state.asks.len(), MAX_ASKS);
        assert_eq!(state.asks[0].prompt, "Prompt 3");
    }

    #[test]
    fn cancel_and_worker_failures_reach_only_the_running_ask() {
        let mut state = AskState::default();
        state.request_cancel();
        assert!(!state.take_cancel(), "nothing runs");
        state.launch("Codex".into(), "Prompt".into()).expect("idle");
        state.request_cancel();
        assert!(state.take_cancel());
        assert!(!state.take_cancel(), "taken once");
        state.fail_running("frontend_worker_stopped");
        assert_eq!(
            state.asks[0].end,
            Some(AskEnd::Failed("frontend_worker_stopped".into()))
        );
    }

    #[test]
    fn setup_is_requested_once_and_dismissed_only_when_finished() {
        let mut state = AskState::default();
        state.request_setup();
        state.request_setup();
        assert!(state.take_setup_request());
        assert!(!state.take_setup_request());
        state.dismiss_setup();
        assert_eq!(state.setup, SetupJob::Running);
        state.finish_setup(SetupOutcome::Failed("no".into()));
        state.dismiss_setup();
        assert_eq!(state.setup, SetupJob::Idle);
    }
}
