//! Canonical approval outcomes applied by the owning runtime actor.
use super::*;

impl<T: RuntimeTransport> Actor<T> {
    pub(super) async fn approval_outcome(
        &mut self,
        outcome: ApprovalOutcome,
    ) -> Result<(), RuntimeStopReason> {
        use vibemux_types::chat_approval::{ChatApprovalDelivery, ChatApprovalState};
        match outcome {
            ApprovalOutcome::InputWritten {
                request_id,
                turn_id,
                outcome,
            } => {
                if outcome != InputWriteOutcome::Written {
                    return Err(RuntimeStopReason::InputFailed);
                }
                let transition = self
                    .machine
                    .mark_input_written(request_id, turn_id)
                    .map_err(|_| RuntimeStopReason::ProtocolViolation)?;
                self.apply(transition).await.map_err(failure_reason)?;
            }
            ApprovalOutcome::Admitted {
                challenge,
                deadline,
                result,
            } => {
                let record = *result.map_err(failure_reason)?;
                if record.chat_id != self.binding.chat_id
                    || record.generation != self.binding.generation
                    || record.request_id != challenge.request_id
                    || *record.attempt_id.as_uuid() != challenge.turn_id
                    || record.state != ChatApprovalState::Pending
                {
                    return Err(RuntimeStopReason::ProtocolViolation);
                }
                let id = record.approval_id;
                self.approvals.active = Some(ActiveApproval {
                    challenge: *challenge,
                    record: record.clone(),
                    deadline: Some(deadline),
                    human_waiting: true,
                    prepared_frame: None,
                });
                if self.close_reason.is_some()
                    || self
                        .machine
                        .snapshot()
                        .active_turn
                        .as_ref()
                        .is_none_or(|t| t.cancel_requested)
                {
                    self.approvals.active = None;
                    self.approval_updates.send_replace(None);
                } else if Instant::now() >= deadline
                    || self.turn_deadline.is_none_or(|d| Instant::now() >= d)
                {
                    self.approvals
                        .expire(self.sink.clone(), self.binding.clone(), id);
                    self.begin_close(RuntimeStopReason::Deadline)
                        .await
                        .map_err(failure_reason)?;
                } else {
                    self.approval_updates.send_replace(Some(record));
                }
            }
            ApprovalOutcome::Decided {
                request,
                response,
                result,
            } => {
                let mut result = match result {
                    Ok(value) => *value,
                    Err(error) => {
                        let _ = response.send(Err(error));
                        self.approval_updates.send_replace(None);
                        self.begin_close(RuntimeStopReason::Closed)
                            .await
                            .map_err(failure_reason)?;
                        return Ok(());
                    }
                };
                let record = result.receipt.approval.clone();
                if record.approval_id != request.approval_id
                    || record.chat_id != self.binding.chat_id
                    || record.generation != self.binding.generation
                    || record.decision != Some(request.decision)
                {
                    let _ = response.send(Err(RuntimeError::WrongTurn));
                    return Err(RuntimeStopReason::ProtocolViolation);
                }
                let _ = response.send(Ok(record.clone()));
                self.approval_updates.send_replace(Some(record.clone()));
                if let Some(active) = self.approvals.active.as_mut() {
                    active.record = record.clone();
                    active.human_waiting = false;
                }
                let Some(dispatch) = result.dispatch.take() else {
                    self.begin_close(RuntimeStopReason::InputFailed)
                        .await
                        .map_err(failure_reason)?;
                    return Ok(());
                };
                self.approvals
                    .validate(
                        self.sink.clone(),
                        self.binding.clone(),
                        request,
                        record,
                        dispatch,
                    )
                    .map_err(failure_reason)?;
            }
            ApprovalOutcome::Validated {
                request,
                record,
                dispatch,
                result,
            } => {
                let (claim, evidence) = (*dispatch).into_parts();
                let Some(active) = self.approvals.active.as_mut() else {
                    return Err(RuntimeStopReason::ProtocolViolation);
                };
                let challenge_id = active.challenge.challenge_id;
                let matches = evidence.chat_id == self.binding.chat_id
                    && evidence.generation == self.binding.generation
                    && evidence.request_id == active.challenge.request_id
                    && evidence.vendor_session_id == active.challenge.vendor_session_id
                    && evidence.vendor_request_id == active.challenge.vendor_request_id
                    && evidence.tool_use_id == active.challenge.tool_use_id
                    && evidence.tool == active.challenge.tool
                    && evidence.vendor_tool_name == active.challenge.vendor_tool_name
                    && evidence.input == active.challenge.input
                    && record.approval_id == active.record.approval_id;
                active.record = (*record).clone();
                active.human_waiting = false;
                let can_send = result.is_ok()
                    && matches
                    && record.decision == Some(request.decision)
                    && record.request_id == active.challenge.request_id
                    && *record.attempt_id.as_uuid() == active.challenge.turn_id
                    && active.deadline.is_some_and(|d| Instant::now() < d)
                    && self.turn_deadline.is_some_and(|d| Instant::now() < d)
                    && self.close_reason.is_none()
                    && self.closing.borrow().is_none()
                    && self
                        .machine
                        .snapshot()
                        .active_turn
                        .as_ref()
                        .is_some_and(|t| !t.cancel_requested)
                    && matches!(self.transport.root_exited(), Ok(false));
                let prepared = active.prepared_frame.take();
                let frame = if can_send { prepared } else { None };
                let receipt =
                    frame.and_then(|frame| self.input.send_tracked(frame.as_bytes()).ok());
                self.approvals
                    .delivery(claim, challenge_id, receipt)
                    .map_err(failure_reason)?;
                self.approval_updates.send_replace(Some(*record));
            }
            ApprovalOutcome::PipeWritten {
                challenge_id,
                claim,
                outcome,
            } => {
                if let Some(active) = self.approvals.active.as_mut() {
                    active.deadline = None;
                    active.human_waiting = false;
                }
                self.last_activity = Instant::now();
                self.approvals
                    .complete(
                        self.sink.clone(),
                        self.binding.clone(),
                        claim,
                        challenge_id,
                        outcome,
                    )
                    .map_err(failure_reason)?;
            }
            ApprovalOutcome::Delivered {
                challenge_id,
                outcome,
                result,
            } => {
                let record = *result.map_err(failure_reason)?;
                let expected = if outcome == InputWriteOutcome::Written {
                    ChatApprovalDelivery::Written
                } else {
                    ChatApprovalDelivery::Unknown
                };
                if record.delivery != Some(expected) {
                    return Err(RuntimeStopReason::ProtocolViolation);
                }
                self.approval_updates.send_replace(Some(record));
                if outcome != InputWriteOutcome::Written {
                    return Err(RuntimeStopReason::InputFailed);
                }
                self.approvals.active = None;
                if self.close_reason.is_some()
                    || self.turn_deadline.is_none_or(|d| Instant::now() >= d)
                {
                    return Ok(());
                }
                let transition = self
                    .machine
                    .mark_approval_written(challenge_id)
                    .map_err(|_| RuntimeStopReason::ProtocolViolation)?;
                self.apply(transition).await.map_err(failure_reason)?;
            }
            ApprovalOutcome::Expired { result } => {
                if let Ok(record) = result {
                    if record.delivery.is_none() && record.state != ChatApprovalState::Pending {
                        self.approvals.active = None;
                    } else if let Some(active) = self.approvals.active.as_mut() {
                        active.record = (*record).clone();
                    }
                    self.approval_updates.send_replace(Some(*record));
                }
                // A claimed decision may have won expiry; never rewrite it.
                self.begin_close(RuntimeStopReason::Deadline)
                    .await
                    .map_err(failure_reason)?;
            }
            ApprovalOutcome::Cancelled { response, result } => {
                let failed = result.is_err();
                if !failed
                    && !self.approvals.operation_pending()
                    && self
                        .approvals
                        .active
                        .as_ref()
                        .is_some_and(|a| a.record.delivery.is_none())
                {
                    self.approvals.active = None;
                }
                let _ = response.send(result);
                if failed {
                    return Err(RuntimeStopReason::WriterUnavailable);
                }
            }
        }
        self.snapshots.send_replace(self.machine.snapshot());
        Ok(())
    }
}
