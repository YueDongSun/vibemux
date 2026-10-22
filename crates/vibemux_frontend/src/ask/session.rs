#![forbid(unsafe_code)]
//! Drives one read-only Ask through ADR 029 dispatch: submit once, then read
//! output pages and status until the attempt settles and every record has
//! been delivered. The daemon owns the attempt; this only observes it, and a
//! cancel is a request the daemon commits.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use uuid::Uuid;
use vibemux_harness::dispatch::{DispatchPhase, DispatchRequest, NativeProtocol};
use vibemuxd::{
    control::HarnessDispatchOutputQuery,
    harness_dispatch::{DispatchOutputPage, OutputReassembler},
};

use super::reply_text::ReplyText;

/// How long the GUI waits on one Ask before giving up on it. The daemon's
/// own route deadline (600 s by default) normally ends it first.
pub const ASK_DEADLINE: Duration = Duration::from_secs(15 * 60);
/// Output pages read per poll, so a burst cannot starve status reads.
const MAX_PAGES_PER_TICK: usize = 32;

/// Where an Ask stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskPhase {
    Submitting,
    Running,
    Cancelling,
    Finished,
}

/// How an Ask ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AskEnd {
    Completed,
    /// The vendor stopped without a verified terminal record.
    Unverified,
    Cancelled,
    /// A stable code, such as a dispatch error or `ask_timeout`.
    Failed(String),
}

/// One observation for the GUI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AskUpdate {
    pub request_id: Uuid,
    pub phase: AskPhase,
    pub reply: String,
    pub truncated: bool,
    pub end: Option<AskEnd>,
}

/// The part of an attempt's status the driver needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptStatus {
    pub phase: DispatchPhase,
    pub error_code: Option<String>,
}

/// The daemon operations one Ask uses. Errors are stable codes.
pub(crate) trait DispatchPort {
    async fn submit(&mut self, request: &DispatchRequest) -> Result<NativeProtocol, String>;
    async fn status(&mut self, request_id: Uuid) -> Result<AttemptStatus, String>;
    async fn output(
        &mut self,
        query: HarnessDispatchOutputQuery,
    ) -> Result<DispatchOutputPage, String>;
    async fn cancel(&mut self, request_id: Uuid) -> Result<(), String>;
}

/// The end an attempt in `status` has reached, if any.
#[must_use]
pub fn attempt_end(status: &AttemptStatus) -> Option<AskEnd> {
    match status.phase {
        DispatchPhase::Admitted | DispatchPhase::Running | DispatchPhase::CancelRequested => None,
        DispatchPhase::Completed => Some(AskEnd::Completed),
        DispatchPhase::Unverified => Some(AskEnd::Unverified),
        DispatchPhase::Cancelled => Some(AskEnd::Cancelled),
        DispatchPhase::Failed => Some(AskEnd::Failed(
            status
                .error_code
                .clone()
                .unwrap_or_else(|| "dispatch_failed".to_string()),
        )),
        // The daemon restarted mid-run; an operator must cancel it.
        DispatchPhase::RecoveryPending => {
            Some(AskEnd::Failed("dispatch_recovery_pending".to_string()))
        }
    }
}

/// Runs one Ask to its end, reporting every observation to `sink`. Setting
/// `cancel` asks the daemon to cancel; the Ask still ends only when the
/// daemon reports the attempt settled.
pub(crate) async fn drive<P: DispatchPort>(
    port: &mut P,
    request: DispatchRequest,
    cancel: &AtomicBool,
    poll_interval: Duration,
    deadline: Duration,
    mut sink: impl FnMut(AskUpdate),
) {
    let request_id = request.request_id;
    let mut report = |phase, reply: &ReplyText, end: Option<AskEnd>| {
        sink(AskUpdate {
            request_id,
            phase,
            reply: reply.text(),
            truncated: reply.truncated(),
            end,
        });
    };
    let mut reply = ReplyText::default();
    report(AskPhase::Submitting, &reply, None);
    let protocol = match port.submit(&request).await {
        Ok(protocol) => protocol,
        Err(code) => {
            report(AskPhase::Finished, &reply, Some(AskEnd::Failed(code)));
            return;
        }
    };
    let started = Instant::now();
    let mut reassembler = OutputReassembler::starting_after(0);
    let mut cancel_sent = false;
    loop {
        if cancel.load(Ordering::Acquire) && !cancel_sent {
            cancel_sent = true;
            if let Err(code) = port.cancel(request_id).await {
                report(AskPhase::Finished, &reply, Some(AskEnd::Failed(code)));
                return;
            }
        }
        let mut delivered = false;
        for _ in 0..MAX_PAGES_PER_TICK {
            let query = HarnessDispatchOutputQuery {
                request_id,
                cursor: reassembler.cursor(),
            };
            let page = match port.output(query).await {
                Ok(page) => page,
                Err(code) => {
                    report(AskPhase::Finished, &reply, Some(AskEnd::Failed(code)));
                    return;
                }
            };
            let more = !page.fragments.is_empty();
            delivered = page.complete;
            match reassembler.accept(page) {
                Ok(records) => {
                    for record in records {
                        if let Ok(value) = serde_json::from_str(&record.raw_json) {
                            reply.absorb(protocol, &value);
                        }
                    }
                }
                Err(_) => {
                    let end = AskEnd::Failed("dispatch_output_inconsistent".to_string());
                    report(AskPhase::Finished, &reply, Some(end));
                    return;
                }
            }
            if !more || delivered {
                break;
            }
        }
        let status = match port.status(request_id).await {
            Ok(status) => status,
            Err(code) => {
                report(AskPhase::Finished, &reply, Some(AskEnd::Failed(code)));
                return;
            }
        };
        if let Some(end) = attempt_end(&status)
            && delivered
        {
            report(AskPhase::Finished, &reply, Some(end));
            return;
        }
        if started.elapsed() >= deadline {
            if !cancel_sent {
                let _ = port.cancel(request_id).await;
            }
            let end = AskEnd::Failed("ask_timeout".to_string());
            report(AskPhase::Finished, &reply, Some(end));
            return;
        }
        let phase = if cancel_sent {
            AskPhase::Cancelling
        } else {
            AskPhase::Running
        };
        report(phase, &reply, None);
        tokio::time::sleep(poll_interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use vibemux_harness::{
        AgentKind,
        dispatch::{ObservationKind, request::DISPATCH_REQUEST_SCHEMA_VERSION},
    };
    use vibemuxd::harness_dispatch::{DispatchOutputFragment, OutputCursor};

    /// Scripted daemon: each output call takes the next page, each status
    /// call the next phase (the last one repeats).
    struct FakePort {
        submit: Result<NativeProtocol, String>,
        pages: VecDeque<DispatchOutputPage>,
        phases: VecDeque<DispatchPhase>,
        cancels: usize,
    }

    impl DispatchPort for FakePort {
        async fn submit(&mut self, _: &DispatchRequest) -> Result<NativeProtocol, String> {
            self.submit.clone()
        }
        async fn status(&mut self, _: Uuid) -> Result<AttemptStatus, String> {
            let phase = if self.phases.len() > 1 {
                self.phases.pop_front().expect("phase")
            } else {
                *self.phases.front().expect("phase")
            };
            Ok(AttemptStatus {
                phase,
                error_code: (phase == DispatchPhase::Failed).then(|| "dispatch_vendor_failed".into()),
            })
        }
        async fn output(
            &mut self,
            query: HarnessDispatchOutputQuery,
        ) -> Result<DispatchOutputPage, String> {
            Ok(self.pages.pop_front().unwrap_or(DispatchOutputPage {
                fragments: vec![],
                next: query.cursor,
                complete: true,
            }))
        }
        async fn cancel(&mut self, _: Uuid) -> Result<(), String> {
            self.cancels += 1;
            Ok(())
        }
    }

    fn page(records: &[(u64, &str)], complete: bool) -> DispatchOutputPage {
        let fragments: Vec<_> = records
            .iter()
            .map(|(sequence, raw)| DispatchOutputFragment {
                sequence: *sequence,
                kind: ObservationKind::Text,
                total_bytes: raw.len() as u64,
                offset: 0,
                fragment: (*raw).to_string(),
            })
            .collect();
        let after = records.last().map_or(0, |(sequence, _)| *sequence);
        DispatchOutputPage {
            fragments,
            next: OutputCursor {
                after_sequence: after,
                cursor_offset: 0,
            },
            complete,
        }
    }

    fn empty_page(after_sequence: u64) -> DispatchOutputPage {
        DispatchOutputPage {
            fragments: vec![],
            next: OutputCursor {
                after_sequence,
                cursor_offset: 0,
            },
            complete: false,
        }
    }

    fn request() -> DispatchRequest {
        DispatchRequest {
            schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
            request_id: Uuid::new_v4(),
            harness: AgentKind::Claude,
            prompt: "Explain the project".into(),
        }
    }

    async fn run(port: &mut FakePort, cancel: bool) -> Vec<AskUpdate> {
        let mut updates = Vec::new();
        let flag = AtomicBool::new(cancel);
        drive(
            port,
            request(),
            &flag,
            Duration::from_millis(1),
            Duration::from_secs(5),
            |update| updates.push(update),
        )
        .await;
        updates
    }

    #[tokio::test]
    async fn a_completed_attempt_reports_its_reply_and_ends() {
        let mut port = FakePort {
            submit: Ok(NativeProtocol::ClaudeStreamJson),
            pages: VecDeque::from([
                page(
                    &[(
                        1,
                        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}}"#,
                    )],
                    false,
                ),
                page(&[(2, r#"{"type":"result","result":"Hello there"}"#)], true),
            ]),
            phases: VecDeque::from([DispatchPhase::Running, DispatchPhase::Completed]),
            cancels: 0,
        };
        let updates = run(&mut port, false).await;
        assert_eq!(updates.first().map(|update| update.phase), Some(AskPhase::Submitting));
        let last = updates.last().expect("final update");
        assert_eq!(last.end, Some(AskEnd::Completed));
        assert_eq!(last.reply, "Hello there");
        assert_eq!(port.cancels, 0);
    }

    #[tokio::test]
    async fn the_end_waits_until_every_record_is_delivered() {
        // The attempt is already terminal, but its last record arrives on a
        // later page; the Ask must not end before reading it.
        let mut port = FakePort {
            submit: Ok(NativeProtocol::CodexAppServer),
            pages: VecDeque::from([
                page(
                    &[(1, r#"{"method":"item/agentMessage/delta","params":{"itemId":"a","delta":"Part "}}"#)],
                    false,
                ),
                // Nothing new yet: the cursor stays after record 1.
                empty_page(1),
                page(
                    &[(2, r#"{"method":"item/agentMessage/delta","params":{"itemId":"a","delta":"two"}}"#)],
                    true,
                ),
            ]),
            phases: VecDeque::from([DispatchPhase::Completed]),
            cancels: 0,
        };
        let last = run(&mut port, false).await.pop().expect("final");
        assert_eq!(last.end, Some(AskEnd::Completed));
        assert_eq!(last.reply, "Part two");
    }

    #[tokio::test]
    async fn a_cancel_is_sent_once_and_the_daemon_decides_the_end() {
        let mut port = FakePort {
            submit: Ok(NativeProtocol::ClaudeStreamJson),
            pages: VecDeque::new(),
            phases: VecDeque::from([
                DispatchPhase::CancelRequested,
                DispatchPhase::CancelRequested,
                DispatchPhase::Cancelled,
            ]),
            cancels: 0,
        };
        let updates = run(&mut port, true).await;
        assert_eq!(port.cancels, 1);
        assert!(updates.iter().any(|update| update.phase == AskPhase::Cancelling));
        assert_eq!(updates.last().and_then(|update| update.end.clone()), Some(AskEnd::Cancelled));
    }

    #[tokio::test]
    async fn submission_and_vendor_failures_end_with_their_code() {
        let mut refused = FakePort {
            submit: Err("dispatch_route_unavailable".into()),
            pages: VecDeque::new(),
            phases: VecDeque::from([DispatchPhase::Running]),
            cancels: 0,
        };
        let last = run(&mut refused, false).await.pop().expect("final");
        assert_eq!(
            last.end,
            Some(AskEnd::Failed("dispatch_route_unavailable".into()))
        );
        let mut failed = FakePort {
            submit: Ok(NativeProtocol::CodexExec),
            pages: VecDeque::new(),
            phases: VecDeque::from([DispatchPhase::Failed]),
            cancels: 0,
        };
        let last = run(&mut failed, false).await.pop().expect("final");
        assert_eq!(last.end, Some(AskEnd::Failed("dispatch_vendor_failed".into())));
    }

    #[test]
    fn only_settled_phases_end_an_ask() {
        let status = |phase| AttemptStatus {
            phase,
            error_code: None,
        };
        assert_eq!(attempt_end(&status(DispatchPhase::Running)), None);
        assert_eq!(attempt_end(&status(DispatchPhase::CancelRequested)), None);
        assert_eq!(
            attempt_end(&status(DispatchPhase::Unverified)),
            Some(AskEnd::Unverified)
        );
        assert_eq!(
            attempt_end(&status(DispatchPhase::RecoveryPending)),
            Some(AskEnd::Failed("dispatch_recovery_pending".into()))
        );
    }
}
