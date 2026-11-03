#![forbid(unsafe_code)]
//! Drives one read-only Ask through ADR 029 dispatch: submit once, then read
//! output pages and status until the attempt settles and every record has
//! been delivered. The daemon owns the attempt; this only observes it, and a
//! cancel is a request the daemon commits.

use std::time::Duration;
use uuid::Uuid;
use vibemux_harness::dispatch::DispatchPhase;

/// How long the GUI waits on one Ask before giving up on it. The daemon's
/// own route deadline (600 s by default) normally ends it first.
pub const ASK_DEADLINE: Duration = Duration::from_secs(15 * 60);

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
        // A recovery reservation remains active until an operator decides.
        DispatchPhase::RecoveryPending => None,
    }
}

/// Legacy presentation adapter over the same canonical observation driver.
#[cfg(test)]
pub(crate) async fn drive<P: super::driver::DispatchPort>(
    port: &mut P,
    request: vibemux_harness::dispatch::DispatchRequest,
    cancel: &std::sync::atomic::AtomicBool,
    poll_interval: Duration,
    deadline: Duration,
    mut sink: impl FnMut(AskUpdate),
) {
    use super::driver::{self, AskOperation, DriverControl, DriverTiming};
    use std::sync::atomic::AtomicBool;
    driver::drive_observed(
        port,
        AskOperation::Submit(request),
        DriverControl {
            cancel,
            stop: &AtomicBool::new(false),
        },
        DriverTiming {
            poll_interval,
            deadline,
        },
        |observation| sink(observation.update),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::super::driver::DispatchPort;
    use super::super::observation::{AskAdmission, AskCanonicalStatus, SubmitError};
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use vibemux_harness::dispatch::{DispatchRequest, NativeProtocol};
    use vibemux_harness::{
        AgentKind,
        dispatch::{ObservationKind, request::DISPATCH_REQUEST_SCHEMA_VERSION},
    };
    use vibemux_types::{RunId, TaskId};
    use vibemuxd::harness_dispatch::{DispatchOutputFragment, OutputCursor};
    use vibemuxd::{control::HarnessDispatchOutputQuery, harness_dispatch::DispatchOutputPage};

    /// Scripted daemon: each output call takes the next page, each status
    /// call the next phase (the last one repeats).
    struct FakePort {
        submit: Result<NativeProtocol, String>,
        /// Errors the first output calls return, in order.
        output_errors: VecDeque<String>,
        pages: VecDeque<DispatchOutputPage>,
        phases: VecDeque<DispatchPhase>,
        cancels: usize,
    }

    impl DispatchPort for FakePort {
        async fn submit(&mut self, request: &DispatchRequest) -> Result<AskAdmission, SubmitError> {
            self.submit
                .clone()
                .map(|protocol| AskAdmission {
                    request_id: request.request_id,
                    task_id: TaskId::from_uuid(Uuid::from_u128(1)).expect("task"),
                    run_id: RunId::from_uuid(Uuid::from_u128(2)).expect("run"),
                    harness: request.harness,
                    protocol,
                    phase: DispatchPhase::Admitted,
                    duplicate: false,
                    sequence: 1,
                })
                .map_err(|code| SubmitError {
                    code,
                    uncertain: false,
                })
        }
        async fn status(&mut self, request_id: Uuid) -> Result<AskCanonicalStatus, String> {
            let phase = if self.phases.len() > 1 {
                self.phases.pop_front().expect("phase")
            } else {
                *self.phases.front().expect("phase")
            };
            let protocol = *self.submit.as_ref().expect("protocol");
            Ok(AskCanonicalStatus {
                request_id,
                task_id: TaskId::from_uuid(Uuid::from_u128(1)).expect("task"),
                run_id: RunId::from_uuid(Uuid::from_u128(2)).expect("run"),
                harness: if protocol == NativeProtocol::ClaudeStreamJson {
                    AgentKind::Claude
                } else {
                    AgentKind::Codex
                },
                protocol,
                version: match phase {
                    DispatchPhase::Admitted => 1,
                    DispatchPhase::Running => 2,
                    DispatchPhase::CancelRequested => 3,
                    _ => 4,
                },
                outcome: None,
                phase,
                error_code: (phase == DispatchPhase::Failed)
                    .then(|| "dispatch_vendor_failed".into()),
            })
        }
        async fn output(
            &mut self,
            query: HarnessDispatchOutputQuery,
        ) -> Result<DispatchOutputPage, String> {
            if let Some(code) = self.output_errors.pop_front() {
                return Err(code);
            }
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
        let flag = AtomicBool::new(false);
        let mut request = request();
        if port
            .submit
            .as_ref()
            .is_ok_and(|protocol| *protocol != NativeProtocol::ClaudeStreamJson)
        {
            request.harness = AgentKind::Codex;
        }
        drive(
            port,
            request,
            &flag,
            Duration::from_millis(1),
            Duration::from_secs(5),
            |update| {
                if cancel && update.phase == AskPhase::Running {
                    flag.store(true, Ordering::Release);
                }
                updates.push(update);
            },
        )
        .await;
        updates
    }

    #[tokio::test]
    async fn a_completed_attempt_reports_its_reply_and_ends() {
        let mut port = FakePort {
            submit: Ok(NativeProtocol::ClaudeStreamJson),
            output_errors: VecDeque::new(),
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
        assert_eq!(
            updates.first().map(|update| update.phase),
            Some(AskPhase::Submitting)
        );
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
            output_errors: VecDeque::new(),
            pages: VecDeque::from([
                page(
                    &[(
                        1,
                        r#"{"method":"item/agentMessage/delta","params":{"itemId":"a","delta":"Part "}}"#,
                    )],
                    false,
                ),
                // Nothing new yet: the cursor stays after record 1.
                empty_page(1),
                page(
                    &[(
                        2,
                        r#"{"method":"item/agentMessage/delta","params":{"itemId":"a","delta":"two"}}"#,
                    )],
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
            output_errors: VecDeque::new(),
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
        assert!(
            updates
                .iter()
                .any(|update| update.phase == AskPhase::Cancelling)
        );
        assert_eq!(
            updates.last().and_then(|update| update.end.clone()),
            Some(AskEnd::Cancelled)
        );
    }

    #[tokio::test]
    async fn submission_and_vendor_failures_end_with_their_code() {
        let mut refused = FakePort {
            submit: Err("dispatch_route_unavailable".into()),
            output_errors: VecDeque::new(),
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
            output_errors: VecDeque::new(),
            pages: VecDeque::new(),
            phases: VecDeque::from([DispatchPhase::Failed]),
            cancels: 0,
        };
        let last = run(&mut failed, false).await.pop().expect("final");
        assert_eq!(
            last.end,
            Some(AskEnd::Failed("dispatch_vendor_failed".into()))
        );
    }

    const UNAVAILABLE: &str = "harness_dispatch_output_unavailable";

    #[tokio::test]
    async fn output_that_is_not_there_yet_is_waited_for() {
        // The daemon starts a transcript only when the attempt launches.
        let mut port = FakePort {
            submit: Ok(NativeProtocol::ClaudeStreamJson),
            output_errors: VecDeque::from([UNAVAILABLE.to_string(), UNAVAILABLE.to_string()]),
            pages: VecDeque::from([page(&[(1, r#"{"type":"result","result":"Ready"}"#)], true)]),
            phases: VecDeque::from([
                DispatchPhase::Admitted,
                DispatchPhase::Running,
                DispatchPhase::Completed,
            ]),
            cancels: 0,
        };
        let last = run(&mut port, false).await.pop().expect("final");
        assert_eq!(last.end, Some(AskEnd::Completed));
        assert_eq!(last.reply, "Ready");
        assert_eq!(port.cancels, 0);
    }

    #[tokio::test]
    async fn a_settled_attempt_without_output_ends_with_its_own_reason() {
        let mut never_launched = FakePort {
            submit: Ok(NativeProtocol::CodexAppServer),
            output_errors: VecDeque::from([UNAVAILABLE.to_string()]),
            pages: VecDeque::new(),
            phases: VecDeque::from([DispatchPhase::Failed]),
            cancels: 0,
        };
        let last = run(&mut never_launched, false).await.pop().expect("final");
        assert_eq!(
            last.end,
            Some(AskEnd::Failed("dispatch_vendor_failed".into()))
        );
        let mut evicted = FakePort {
            submit: Ok(NativeProtocol::CodexAppServer),
            output_errors: VecDeque::from([UNAVAILABLE.to_string()]),
            pages: VecDeque::new(),
            phases: VecDeque::from([DispatchPhase::Completed]),
            cancels: 0,
        };
        let last = run(&mut evicted, false).await.pop().expect("final");
        assert_eq!(last.end, Some(AskEnd::Completed));
    }

    #[tokio::test]
    async fn giving_up_cancels_the_attempt_left_behind() {
        let mut lost = FakePort {
            submit: Ok(NativeProtocol::ClaudeStreamJson),
            output_errors: VecDeque::from(["control_connection_failed".to_string()]),
            pages: VecDeque::new(),
            phases: VecDeque::from([DispatchPhase::Running]),
            cancels: 0,
        };
        let last = run(&mut lost, false).await.pop().expect("final");
        assert_eq!(
            last.end,
            Some(AskEnd::Failed("control_connection_failed".into()))
        );
        assert_eq!(
            lost.cancels, 1,
            "the owning submitter requests best-effort cancellation"
        );

        let mut slow = FakePort {
            submit: Ok(NativeProtocol::ClaudeStreamJson),
            output_errors: VecDeque::new(),
            pages: VecDeque::new(),
            phases: VecDeque::from([DispatchPhase::Running]),
            cancels: 0,
        };
        let mut last = None;
        drive(
            &mut slow,
            request(),
            &AtomicBool::new(false),
            Duration::from_millis(1),
            Duration::ZERO,
            |update| last = Some(update),
        )
        .await;
        assert_eq!(
            last.and_then(|update| update.end),
            Some(AskEnd::Failed("ask_timeout".into()))
        );
        assert_eq!(slow.cancels, 1);
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
        assert_eq!(attempt_end(&status(DispatchPhase::RecoveryPending)), None);
    }
}
